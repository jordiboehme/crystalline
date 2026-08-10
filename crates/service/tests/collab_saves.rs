//! The session save path, driven WITHOUT a socket and WITHOUT sleeping: the
//! saver pass takes `now`, so a debounce window is a value rather than a wait.
//! Every assertion is about what reaches the file and what the room is told.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::collab::control::{self, Control};
use crystalline_service::collab::session::{CollabSessions, Frame, Joined, SAVE_DEBOUNCE_MS};
use tokio::sync::Mutex;
use yrs::sync::{Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Options, ReadTxn, Text, Transact, Update};

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// A file domain `eng` holding MANIFEST, alpha and a CRLF engram, synced into
/// an in-memory store. Mirrors `collab_session.rs::engine_fixture`; integration
/// test crates share no helpers, so it is copied rather than imported.
async fn engine_fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
    )
    .unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    std::fs::write(
        dir.join("crlf.md"),
        "---\r\ntype: engram\r\ntitle: Crlf\r\npermalink: crlf\r\ntags:\r\n  - eng\r\nstatus: stable\r\n---\r\n\r\nwindows body\r\n",
    )
    .unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// Frame one client update the way the provider sends it.
fn frame_update(update: Vec<u8>) -> Vec<u8> {
    Message::Sync(SyncMessage::Update(update)).encode_v1()
}

/// Split a greeting/reply buffer into its protocol messages.
fn messages_of(bytes: &[u8]) -> Vec<Message> {
    let mut decoder = DecoderV1::from(bytes);
    MessageReader::new(&mut decoder)
        .collect::<Result<_, _>>()
        .unwrap()
}

/// A JS-shaped client doc, synced against the session: SyncStep1 out,
/// SyncStep2 back, applied.
async fn sync_client(joined: &Joined) -> Doc {
    let doc = Doc::with_options(Options {
        offset_kind: yrs::OffsetKind::Utf16,
        ..Options::default()
    });
    let sv = doc.transact().state_vector();
    let replies = joined
        .session
        .handle_frame(
            joined.conn,
            &Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1(),
        )
        .await;
    let Message::Sync(SyncMessage::SyncStep2(update)) = &messages_of(&replies[0])[0] else {
        panic!("step1 is answered with step2");
    };
    doc.transact_mut()
        .apply_update(Update::decode_v1(update).unwrap())
        .unwrap();
    doc
}

/// Append `line` plus a newline at the DOCUMENT END and send it. The end, not
/// position 0: text inserted ahead of the frontmatter would leave the document
/// without one, and the engine's parse gate would rightly refuse the save.
async fn append_line(joined: &Joined, doc: &Doc, line: &str) {
    let text = doc.get_or_insert_text("content");
    let update = {
        let mut txn = doc.transact_mut();
        let end = text.get_string(&txn).encode_utf16().count() as u32;
        text.insert(&mut txn, end, &format!("{line}\n"));
        txn.encode_update_v1()
    };
    joined
        .session
        .handle_frame(joined.conn, &frame_update(update))
        .await;
}

/// Clear the text and refill it with `content`, in one transaction.
async fn replace_all(joined: &Joined, doc: &Doc, content: &str) {
    let text = doc.get_or_insert_text("content");
    let update = {
        let mut txn = doc.transact_mut();
        let len = text.get_string(&txn).encode_utf16().count() as u32;
        text.remove_range(&mut txn, 0, len);
        text.insert(&mut txn, 0, content);
        txn.encode_update_v1()
    };
    joined
        .session
        .handle_frame(joined.conn, &frame_update(update))
        .await;
}

/// The next control on this receiver, skipping sync and awareness frames;
/// panics after ten seconds so a missing broadcast fails loudly.
async fn next_control(rx: &mut tokio::sync::broadcast::Receiver<Frame>) -> Control {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a control frame arrives in time")
            .expect("the broadcast channel stays open");
        let mut decoder = DecoderV1::from(frame.bytes.as_ref());
        for message in MessageReader::new(&mut decoder).flatten() {
            if let Message::Custom(control::CONTROL_TAG, payload) = message
                && let Some(found) = control::decode(&payload)
            {
                return found;
            }
        }
    }
}

#[tokio::test]
async fn a_pause_lands_the_save_with_the_separator_reapplied() {
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "crlf").await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "hello from the session").await;

    // The debounce window elapses (synthetic now; no sleeping).
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/crlf.md")).unwrap();
    assert!(
        on_disk.ends_with("windows body\r\nhello from the session\r\n"),
        "saved with CRLF back: {on_disk:?}"
    );
    assert!(
        !on_disk.replace("\r\n", "").contains('\n'),
        "no stray LF was minted"
    );
    let saved = next_control(&mut joined.rx).await;
    assert!(matches!(saved, Control::Saved { .. }));
}

#[tokio::test]
async fn an_untouched_session_never_writes() {
    let (tmp, engine) = engine_fixture().await;
    let before = std::fs::read(tmp.path().join("eng/crlf.md")).unwrap();
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "crlf").await.unwrap();
    let _doc = sync_client(&joined).await;
    // Open, sync, leave: the byte-fidelity property for a no-op session.
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    sessions.dispose_if_empty(&joined.session).await;
    let after = std::fs::read(tmp.path().join("eng/crlf.md")).unwrap();
    assert_eq!(before, after, "open-then-close is byte-identical");
}

#[tokio::test]
async fn a_joining_client_alone_never_arms_a_save() {
    // A joining provider answers the greeting's SyncStep1 with a SyncStep2 of
    // its own, which marks the session dirty while carrying no edit. The
    // debounce gates on the text comparison, not on that flag, so the elapsed
    // window still writes nothing and the file keeps its mtime.
    let (tmp, engine) = engine_fixture().await;
    let path = tmp.path().join("eng/alpha.md");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    // The client's own state, echoed back at the server: no edit inside.
    let echo = doc
        .transact()
        .encode_state_as_update_v1(&Default::default());
    joined
        .session
        .handle_frame(
            joined.conn,
            &Message::Sync(SyncMessage::SyncStep2(echo)).encode_v1(),
        )
        .await;

    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before,
        "an unedited join never touches the file"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), ALPHA);
}

#[tokio::test]
async fn the_last_leave_lands_the_final_save() {
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "a final thought").await;
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    sessions.dispose_if_empty(&joined.session).await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.ends_with("a final thought\n"));
    assert_eq!(sessions.session_count().await, 0);
}

#[tokio::test]
async fn a_flush_request_saves_now_not_after_the_debounce() {
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "save this now").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    // The very next tick, with NO debounce elapsed, lands it.
    joined.session.tick_save(Instant::now()).await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.ends_with("save this now\n"));
}

#[tokio::test]
async fn a_tick_inside_the_debounce_window_holds_the_save_back() {
    // Typing is not a save: the pass that runs while the window is still open
    // writes nothing, and the one past it writes everything.
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "still typing").await;

    joined.session.tick_save(Instant::now()).await;
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA,
        "the debounce window is still open"
    );

    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.ends_with("still typing\n"), "{on_disk:?}");
}

#[tokio::test]
async fn a_refused_save_blocks_saving_not_editing_and_recovers() {
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;

    // Wreck the document: delete everything, so it has no frontmatter and the
    // engine's one hard gate refuses the save.
    let text = doc.get_or_insert_text("content");
    let len = {
        let txn = doc.transact();
        text.get_string(&txn).encode_utf16().count() as u32
    };
    let edit = {
        let mut txn = doc.transact_mut();
        text.remove_range(&mut txn, 0, len);
        text.insert(&mut txn, 0, "no frontmatter at all");
        txn.encode_update_v1()
    };
    joined
        .session
        .handle_frame(joined.conn, &frame_update(edit))
        .await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;

    let failed = next_control(&mut joined.rx).await;
    let Control::SaveFailed { detail } = failed else {
        panic!("save-failed broadcast")
    };
    assert!(
        detail.contains("frontmatter"),
        "the engine's own words: {detail}"
    );
    let untouched = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(untouched.starts_with("---"), "nothing was written");

    // The author repairs the text; the next flush saves and the state heals.
    let repair = {
        let mut txn = doc.transact_mut();
        let len = "no frontmatter at all".encode_utf16().count() as u32;
        text.remove_range(&mut txn, 0, len);
        text.insert(&mut txn, 0, ALPHA);
        txn.encode_update_v1()
    };
    joined
        .session
        .handle_frame(joined.conn, &frame_update(repair))
        .await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let saved = next_control(&mut joined.rx).await;
    assert!(matches!(saved, Control::Saved { .. }));
}

#[tokio::test]
async fn a_frontmatter_rename_moves_the_session_and_the_receipt_says_so() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    // Replace the whole text with a version whose permalink line says beta.
    replace_all(
        &joined,
        &doc,
        &ALPHA.replace("permalink: alpha", "permalink: beta"),
    )
    .await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let Control::Saved { permalink, .. } = next_control(&mut joined.rx).await else {
        panic!("saved");
    };
    assert_eq!(permalink, "beta");
    // The NEXT save uses the new identifier: edit the body again and flush.
    append_line(&joined, &doc, "again").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let Control::Saved { permalink, .. } = next_control(&mut joined.rx).await else {
        panic!("saved after rename");
    };
    assert_eq!(permalink, "beta");
}

/// A full frontmatter rename: an author who renames an engram moves its title
/// with its permalink, so the old identifier stops resolving entirely.
fn renamed_alpha() -> String {
    ALPHA
        .replace("permalink: alpha", "permalink: beta")
        .replace("title: Alpha", "title: Beta")
}

/// Rename `alpha` to `beta` through a flushed save on `joined`.
async fn rename_to_beta(joined: &Joined, doc: &Doc) {
    replace_all(joined, doc, &renamed_alpha()).await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
}

#[tokio::test]
async fn a_rename_moves_the_registry_key_so_the_new_permalink_finds_the_same_room() {
    // Without the re-key a client that follows the Saved { permalink }
    // broadcast would open a SECOND room over the same file, and the two would
    // fight over one CAS token.
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    rename_to_beta(&joined, &doc).await;

    let rejoined = sessions.join("eng", "beta").await.unwrap();
    assert!(
        Arc::ptr_eq(&joined.session, &rejoined.session),
        "the rename followed the engram, so beta is the live room"
    );
    assert_eq!(sessions.session_count().await, 1, "one room, one file");
}

#[tokio::test]
async fn the_old_permalink_stops_resolving_after_a_rename() {
    // The stale key is gone rather than left pointing at the renamed room: a
    // client that asks for alpha gets the engine's own miss and falls back to
    // solo editing, instead of adopting a room whose content is now beta.
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    rename_to_beta(&joined, &doc).await;

    let refused = sessions.join("eng", "alpha").await.unwrap_err();
    assert!(
        matches!(
            refused,
            crystalline_service::collab::session::JoinError::Engine(_)
        ),
        "alpha is nobody's engram now: {refused:?}"
    );
    assert_eq!(sessions.session_count().await, 1, "no second room opened");
}

#[tokio::test]
async fn a_renamed_session_still_disposes_cleanly() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    rename_to_beta(&joined, &doc).await;

    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    // Disposal goes by the session, so it finds the room under its NEW key.
    sessions.dispose_if_empty(&joined.session).await;
    assert_eq!(
        sessions.session_count().await,
        0,
        "no entry leaks under either permalink"
    );
}

#[tokio::test]
async fn a_blocked_session_backs_off_instead_of_hammering_the_engine() {
    // The edit timers stay armed while a document is unsaveable, so an
    // unguarded retry would call the engine on every 250ms tick for as long as
    // the author leaves the frontmatter broken.
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;

    // The failing attempt happens at a synthetic instant far enough ahead that
    // every later tick in this test has the debounce behind it.
    let failed_at = Instant::now() + Duration::from_secs(10);
    replace_all(&joined, &doc, "no frontmatter at all").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(failed_at).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::SaveFailed { .. }
    ));

    // The document becomes saveable again, so any attempt at all would write.
    let repaired = format!("{ALPHA}a repaired body\n");
    replace_all(&joined, &doc, &repaired).await;
    joined
        .session
        .tick_save(failed_at + Duration::from_millis(1))
        .await;
    joined
        .session
        .tick_save(failed_at + Duration::from_millis(2))
        .await;
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA,
        "the two ticks inside the backoff window attempted nothing"
    );

    // One backoff window after the refusal, the retry runs on its own.
    joined
        .session
        .tick_save(failed_at + Duration::from_millis(SAVE_DEBOUNCE_MS + 1))
        .await;
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        repaired,
        "the backoff expires rather than blocking retries forever"
    );
}

#[tokio::test]
async fn a_flush_retries_a_blocked_save_immediately() {
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;

    let failed_at = Instant::now() + Duration::from_secs(10);
    replace_all(&joined, &doc, "no frontmatter at all").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(failed_at).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::SaveFailed { .. }
    ));

    // The Save button, pressed the moment the alert appears: no waiting.
    let repaired = format!("{ALPHA}repaired and flushed\n");
    replace_all(&joined, &doc, &repaired).await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined
        .session
        .tick_save(failed_at + Duration::from_millis(1))
        .await;
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        repaired,
        "an explicit flush skips the backoff"
    );
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
}

#[tokio::test]
async fn a_poisoned_session_closes_the_room_and_never_saves_again() {
    // The containment a panicked saver pass buys: the room is told, the
    // session stops writing, and no later join can adopt the dead room.
    let (tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "never lands").await;

    joined.session.poison().await;
    let closed = next_control(&mut joined.rx).await;
    let Control::Closed { reason } = closed else {
        panic!("the room is closed")
    };
    assert_eq!(reason, "internal");

    // Every save path is a no-op afterwards, the explicit flush included.
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    joined.session.final_save().await;
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA,
        "a poisoned session never writes"
    );

    // And a fresh join gets a fresh room rather than the saver-less corpse.
    let again = sessions.join("eng", "alpha").await.unwrap();
    assert_ne!(again.session.epoch(), joined.session.epoch());
    assert_eq!(sessions.session_count().await, 1, "the corpse was replaced");
}
