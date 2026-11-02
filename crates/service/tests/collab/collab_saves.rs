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

/// The one engram of the second domain, so a sweep over `eng` has a neighbour
/// it must leave alone.
const OAK_NOTE: &str = "---\ntype: engram\ntitle: Oak Note\npermalink: oak-note\ntags:\n  - oak\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Oak Note\n\nA rule about oak.\n";

/// Write `name`'s MANIFEST into `dir`, the routing file every domain needs.
fn write_manifest(dir: &std::path::Path, name: &str) {
    std::fs::write(
        dir.join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
        ),
    )
    .unwrap();
}

/// Two file domains - `eng` holding MANIFEST, alpha and a CRLF engram, and
/// `oak` holding MANIFEST and one note - synced into an in-memory store.
/// Mirrors `collab_session.rs::engine_fixture`; integration test crates share
/// no helpers, so it is copied rather than imported.
async fn engine_fixture() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    fixture_with(false).await
}

/// [`engine_fixture`] with a state directory, which a rename and a removal
/// need for their journals.
async fn engine_fixture_with_state() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    fixture_with(true).await
}

async fn fixture_with(
    state: bool,
) -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    let scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    write_manifest(&dir, "eng");
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    std::fs::write(
        dir.join("crlf.md"),
        "---\r\ntype: engram\r\ntitle: Crlf\r\npermalink: crlf\r\ntags:\r\n  - eng\r\nstatus: stable\r\n---\r\n\r\nwindows body\r\n",
    )
    .unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    let oak = root.join("oak");
    std::fs::create_dir_all(&oak).unwrap();
    write_manifest(&oak, "oak");
    std::fs::write(oak.join("oak-note.md"), OAK_NOTE).unwrap();
    cfg.domains
        .insert("oak".to_string(), DomainEntry::file(oak));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path));
    if state {
        engine = engine.with_state_dir(root.join("state"));
    }
    let engine = Arc::new(engine);
    engine.sync(None).await.unwrap();
    (tmp, engine, scratch)
}

/// An engine over `eng` with Reflection on and one reflection, and a scratch
/// state directory.
async fn reflection_fixture() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    reflection_fixture_with_aliases(&[]).await
}

/// [`reflection_fixture`] with `eng` also known by `alias`.
async fn reflection_fixture_with_alias(
    alias: &str,
) -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    reflection_fixture_with_aliases(&[alias]).await
}

async fn reflection_fixture_with_aliases(
    aliases: &[&str],
) -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    reflection_fixture_at(aliases, false).await
}

/// [`reflection_fixture`] whose engine knows its state directory
/// (`<scratch home>/state`), so a room can keep a text it could not save.
async fn reflection_fixture_keeping() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    reflection_fixture_at(&[], true).await
}

async fn reflection_fixture_at(
    aliases: &[&str],
    with_state_dir: bool,
) -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    let scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("eng");
    std::fs::create_dir_all(dir.join("reflections/guides")).unwrap();
    write_manifest(&dir, "eng");
    let manifest = std::fs::read_to_string(dir.join("MANIFEST.md"))
        .unwrap()
        .replacen("---\n\n", "reflection: on\n---\n\n", 1);
    std::fs::write(dir.join("MANIFEST.md"), manifest).unwrap();
    std::fs::write(
        dir.join("reflections/guides/a.md"),
        "# A\n\n## One\n<!-- r:aaaa1111 -->\n\nFirst.\n\n## Two\n<!-- r:bbbb2222 -->\n\nSecond.\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("reflections/.reflected/guides")).unwrap();
    std::fs::write(
        dir.join("reflections/.reflected/guides/a.md"),
        "## One\n<!-- r:aaaa1111 -->\n\nFirst.\n\n",
    )
    .unwrap();
    let mut cfg = GlobalConfig::default();
    let mut entry = DomainEntry::file(dir);
    entry.aliases = aliases.iter().map(|a| a.to_string()).collect();
    cfg.domains.insert("eng".into(), entry);
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, None);
    if with_state_dir {
        engine = engine.with_state_dir(scratch.home().join("state"));
    }
    let engine = Arc::new(engine);
    engine.sync(None).await.unwrap();
    (tmp, engine, scratch)
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

/// The next `Closed` control on this receiver, skipping every other one (a
/// room's last save announces itself before the close).
async fn next_closed(rx: &mut tokio::sync::broadcast::Receiver<Frame>) -> Control {
    loop {
        let control = next_control(rx).await;
        if matches!(control, Control::Closed { .. }) {
            return control;
        }
    }
}

#[tokio::test]
async fn a_pause_lands_the_save_as_lf() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "crlf", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "hello from the session").await;

    // The debounce window elapses (synthetic now; no sleeping).
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;

    // The whole file lands as LF, the CRLF lines the session never touched
    // included: Crystalline writes LF only.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/crlf.md")).unwrap();
    assert!(
        on_disk.ends_with("windows body\nhello from the session\n"),
        "saved as LF: {on_disk:?}"
    );
    assert!(!on_disk.contains('\r'), "no CR is left: {on_disk:?}");
    let saved = next_control(&mut joined.rx).await;
    assert!(matches!(saved, Control::Saved { .. }));

    // The editor is the surface most human authoring comes through, and it
    // reaches the engine directly rather than through the REST write handler,
    // so the domain owes a consolidation sweep from here too.
    assert!(
        crystalline_service::maintenance::load()
            .pending_domains
            .contains(&"eng".to_string()),
        "a landed co-editing save marks its domain pending"
    );
}

/// The room's save is a feed point: one `modified` per landed save, and
/// nothing for a tick that had nothing to save.
#[tokio::test]
async fn a_room_save_announces_once() {
    let (_tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    let mut rx = engine.changes().subscribe();
    append_line(&joined, &doc, "typed in the room").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    let saved = next_control(&mut joined.rx).await;
    assert!(matches!(saved, Control::Saved { .. }));
    let first = rx.try_recv().expect("the save announced");
    match &first.change {
        crystalline_service::changes::Change::Engram(change) => {
            assert_eq!(change.permalink, "alpha");
            assert_eq!(
                change.kind,
                crystalline_service::changes::ChangeKind::Modified
            );
            assert_eq!(
                change.actor, None,
                "a room saves as the machine owner, who has no label"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(rx.try_recv().is_err(), "once");
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    assert!(rx.try_recv().is_err(), "a quiet tick announces nothing");
}

/// A file that mixes CRLF and LF hosts a room like any other, and the
/// room's save writes the whole file as LF.
#[tokio::test]
async fn a_mixed_endings_file_hosts_a_room_and_saves_as_lf() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    std::fs::write(
        tmp.path().join("eng/mixed.md"),
        "---\r\ntitle: Mixed\r\npermalink: mixed\r\ntags:\r\n  - eng\r\nstatus: stable\r\ntype: engram\r\n---\r\n\r\na CRLF file\nwith a lone LF\r\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "mixed", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed in the room").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    let saved = next_control(&mut joined.rx).await;
    assert!(matches!(saved, Control::Saved { .. }), "{saved:?}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/mixed.md")).unwrap();
    assert!(!on_disk.contains('\r'), "{on_disk:?}");
    assert!(
        on_disk.ends_with("a CRLF file\nwith a lone LF\ntyped in the room\n"),
        "{on_disk:?}"
    );
}

/// A lone `\r` is line content, not a line ending: it rides through a room
/// over a CRLF file and its save untouched, while every CRLF pair turns LF.
#[tokio::test]
async fn a_lone_cr_survives_a_room_and_its_save() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    std::fs::write(
        tmp.path().join("eng/cr.md"),
        "---\r\ntitle: Cr\r\npermalink: cr\r\ntags:\r\n  - eng\r\nstatus: stable\r\ntype: engram\r\n---\r\n\r\nbefore\rafter\r\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "cr", None).await.unwrap();
    let doc = sync_client(&joined).await;
    let synced = doc
        .get_or_insert_text("content")
        .get_string(&doc.transact());
    assert!(synced.contains("before\rafter\n"), "{synced:?}");
    append_line(&joined, &doc, "typed").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/cr.md")).unwrap();
    assert!(on_disk.ends_with("before\rafter\ntyped\n"), "{on_disk:?}");
    assert!(!on_disk.contains("\r\n"), "{on_disk:?}");
}

#[tokio::test]
async fn an_untouched_session_never_writes() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let before = std::fs::read(tmp.path().join("eng/crlf.md")).unwrap();
    let maintenance_before = crystalline_service::maintenance::load();
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "crlf", None).await.unwrap();
    let _doc = sync_client(&joined).await;
    // Open, sync, leave: the byte-fidelity property for a no-op session.
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    sessions.dispose_if_empty(&joined.session).await;
    let after = std::fs::read(tmp.path().join("eng/crlf.md")).unwrap();
    assert_eq!(before, after, "open-then-close is byte-identical");
    assert_eq!(
        crystalline_service::maintenance::load(),
        maintenance_before,
        "a session that wrote nothing owes no sweep"
    );
}

#[tokio::test]
async fn a_joining_client_alone_never_arms_a_save() {
    // A joining provider answers the greeting's SyncStep1 with a SyncStep2 of
    // its own, which marks the session dirty while carrying no edit. The
    // debounce gates on the text comparison, not on that flag, so the elapsed
    // window still writes nothing and the file keeps its mtime.
    let (tmp, engine, _scratch) = engine_fixture().await;
    let path = tmp.path().join("eng/alpha.md");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
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
async fn a_joiner_into_a_blocked_room_is_greeted_with_the_standing_refusal() {
    // The refusal was broadcast before this tab had a socket, and the session
    // never repeats a detail it has already announced: the greeting is the
    // only place a joiner can learn that the room cannot save. Without it the
    // second author reads "Saved" over an engram nothing has written since.
    let (_tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut first = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&first).await;
    replace_all(&first, &doc, "no frontmatter at all").await;
    first
        .session
        .handle_frame(first.conn, &control::encode(&Control::Flush))
        .await;
    first.session.tick_save(Instant::now()).await;
    let Control::SaveFailed { detail } = next_control(&mut first.rx).await else {
        panic!("the refusal is broadcast to the room that was there")
    };

    let second = sessions.join("eng", "alpha", None).await.unwrap();
    let Message::Custom(tag, payload) = &messages_of(&second.greeting)[0] else {
        panic!("the greeting opens with the hello control")
    };
    assert_eq!(*tag, control::CONTROL_TAG);
    let Some(Control::Hello {
        save_state,
        detail: greeted,
        ..
    }) = control::decode(payload)
    else {
        panic!("hello parses")
    };
    assert_eq!(save_state, "failed");
    assert_eq!(
        greeted.as_deref(),
        Some(detail.as_str()),
        "the joiner is told the same reason the room was"
    );
}

#[tokio::test]
async fn a_frontmatter_rename_moves_the_session_and_the_receipt_says_so() {
    let (_tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (_tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    rename_to_beta(&joined, &doc).await;

    let rejoined = sessions.join("eng", "beta", None).await.unwrap();
    assert!(
        Arc::ptr_eq(&joined.session, &rejoined.session),
        "the rename followed the engram, so beta is the live room"
    );
    assert_eq!(sessions.session_count().await, 1, "one room, one file");
}

/// A permalink change under an open room closes the room instead of being
/// refused: the room saves its typing first, every socket is told `moved`
/// with the new address, and the moved engram holds the typed line. A
/// path-only move keeps the room (`a_path_only_move_of_an_open_engram_keeps_the_rooms_typing`).
#[tokio::test]
async fn a_permalink_move_saves_the_room_and_closes_it_with_moved() {
    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed just before the move").await;

    let p = crystalline_service::params::MoveParams {
        identifier: "alpha".into(),
        domain: "eng".into(),
        destination: "alpha.md".into(),
        destination_domain: None,
        permalink: Some("renamed".into()),
        update_links: None,
    };
    engine
        .move_engram(&p, &crystalline_service::Scope::Unrestricted)
        .await
        .expect("an open room does not refuse the move");
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: Some("alpha.md".into()),
            permalink: Some("renamed".into()),
        }
    );
    assert!(joined.session.is_disposed(), "the room is over");
    assert_eq!(sessions.session_count().await, 0, "and out of the registry");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("typed just before the move") && on_disk.contains("permalink: renamed"),
        "the typing was saved first and moved with the engram: {on_disk}"
    );
    // The new address opens a fresh room on the moved text.
    let rejoined = sessions.join("eng", "renamed", None).await.unwrap();
    let (live, _) = rejoined.session.snapshot().await;
    assert!(live.contains("typed just before the move"), "{live}");
}

/// A move into another domain names that domain in the close.
#[tokio::test]
async fn a_move_into_another_domain_names_it_in_the_close() {
    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before it left eng").await;
    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".into(),
                domain: "eng".into(),
                destination: "notes/alpha.md".into(),
                destination_domain: Some("oak".into()),
                permalink: None,
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let Control::Closed {
        reason,
        domain,
        path,
        permalink,
    } = next_closed(&mut joined.rx).await
    else {
        unreachable!()
    };
    assert_eq!(reason, "moved");
    assert_eq!(domain.as_deref(), Some("oak"));
    assert_eq!(path.as_deref(), Some("notes/alpha.md"));
    let permalink = permalink.expect("an engram's close names its permalink");
    let moved = std::fs::read_to_string(tmp.path().join("oak/notes/alpha.md")).unwrap();
    assert!(moved.contains("typed before it left eng"), "{moved}");
    assert!(
        moved.contains(&format!("permalink: {permalink}")),
        "{moved}"
    );
}

/// The `Closed` control addressed to `conn` (or to everybody) on this
/// receiver, skipping every other frame.
async fn closed_for(rx: &mut tokio::sync::broadcast::Receiver<Frame>, conn: u64) -> Control {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a frame arrives in time")
            .expect("the broadcast channel stays open");
        if frame.to.is_some_and(|only| only != conn) {
            continue;
        }
        let mut decoder = DecoderV1::from(frame.bytes.as_ref());
        for message in MessageReader::new(&mut decoder).flatten() {
            if let Message::Custom(control::CONTROL_TAG, payload) = message
                && let Some(found @ Control::Closed { .. }) = control::decode(&payload)
            {
                return found;
            }
        }
    }
}

/// A move into a private domain names that domain only to the people who
/// may see it. Bob, who is not a member of `hr`, is told the engram moved
/// and nothing else, in the room and on a later join; Ann, a member, gets
/// the whole address.
#[tokio::test]
async fn a_move_into_a_private_domain_names_it_only_to_its_members() {
    use crystalline_service::rest::{AuthStore, MemberLevel, Role};
    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "bob", "ann"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    // `oak` plays hr: private, owned by keeper, with ann as a member.
    auth.set_domain_visibility("oak", true, "keeper")
        .await
        .unwrap();
    auth.upsert_domain_member("oak", "ann", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(crystalline_service::DomainAccess::new(auth)));
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let user = |account: &str| crystalline_service::Scope::User {
        account: account.into(),
        admin: false,
    };
    let mut bob = sessions
        .join_as("eng", "alpha", None, user("bob"))
        .await
        .unwrap();
    let mut ann = sessions
        .join_as("eng", "alpha", None, user("ann"))
        .await
        .unwrap();
    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".into(),
                domain: "eng".into(),
                destination: "alpha.md".into(),
                destination_domain: Some("oak".into()),
                permalink: Some("salaries".into()),
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        closed_for(&mut bob.rx, bob.conn).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: None,
            permalink: None,
        },
        "bob learns nothing about where it went"
    );
    assert_eq!(
        closed_for(&mut ann.rx, ann.conn).await,
        Control::Closed {
            reason: "moved".into(),
            domain: Some("oak".into()),
            path: Some("alpha.md".into()),
            permalink: Some("salaries".into()),
        }
    );
    // A later join at the old address: the registry knows where it went,
    // and only a caller who may see `oak` is told.
    let to = match sessions.join("eng", "alpha", None).await {
        Err(crystalline_service::collab::session::JoinError::Moved(to)) => to,
        Err(other) => panic!("the move, not {other}"),
        Ok(_) => panic!("no room opens at the old address"),
    };
    assert!(!sessions.may_see_move(&to, &user("bob")).await);
    assert!(sessions.may_see_move(&to, &user("ann")).await);
}

/// A connection the registry knows nobody for (joined with no viewer) is
/// told no address when the engram moves into another domain: the close
/// fails closed. The machine owner's connection, joined through `join`, is
/// told the whole address.
#[tokio::test]
async fn a_moved_close_names_no_address_to_a_connection_of_nobody_known() {
    let (_tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut unknown = sessions
        .join_doc(
            "eng",
            crystalline_service::collab::session::RoomDoc::Engram("alpha".into()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let mut owner = sessions.join("eng", "alpha", None).await.unwrap();
    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".into(),
                domain: "eng".into(),
                destination: "alpha.md".into(),
                destination_domain: Some("oak".into()),
                permalink: None,
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        closed_for(&mut unknown.rx, unknown.conn).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: None,
            permalink: None,
        }
    );
    let Control::Closed { domain, path, .. } = closed_for(&mut owner.rx, owner.conn).await else {
        unreachable!()
    };
    assert_eq!(domain.as_deref(), Some("oak"));
    assert_eq!(path.as_deref(), Some("alpha.md"));
}

/// R81 for an engram: a room cannot open over the old address between the
/// move's close and its commit. The join is refused while the move holds
/// the address, and the moved engram opens at its new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_cannot_open_over_an_engram_inside_its_move_window() {
    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (entered, release) = engine.hold_next_room_screen_until();
    let moving = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .move_engram(
                    &crystalline_service::params::MoveParams {
                        identifier: "alpha".into(),
                        domain: "eng".into(),
                        destination: "alpha.md".into(),
                        destination_domain: None,
                        permalink: Some("renamed".into()),
                        update_links: None,
                    },
                    &crystalline_service::Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the move reaches the gate");
    let refused = match sessions.join("eng", "alpha", None).await {
        Err(refused) => refused.to_string(),
        Ok(_) => panic!("a room opened inside the move window"),
    };
    assert!(refused.contains("is being moved"), "{refused}");
    release.notify_one();
    moving.await.unwrap().unwrap();
    // The fence is gone with the move: the new address opens a room.
    assert!(sessions.join("eng", "renamed", None).await.is_ok());
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.contains("permalink: renamed"), "{on_disk}");
}

/// A room whose last save cannot land (its text no longer parses) keeps that
/// text on the server when a move closes it, and the move carries the text
/// that was saved: nothing typed is lost.
#[tokio::test]
async fn a_move_over_a_room_that_cannot_save_keeps_its_text() {
    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    replace_all(&joined, &doc, "no frontmatter, typed and never saved\n").await;
    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".into(),
                domain: "eng".into(),
                destination: "alpha.md".into(),
                destination_domain: None,
                permalink: Some("renamed".into()),
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let Control::Closed { reason, .. } = next_closed(&mut joined.rx).await else {
        unreachable!()
    };
    assert_eq!(reason, "moved");
    let kept_dir = tmp.path().join("state/kept/eng/engram");
    let kept: Vec<_> = std::fs::read_dir(&kept_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    let held = std::fs::read_to_string(&kept[0]).unwrap();
    assert!(held.contains("typed and never saved"), "{held}");
    let moved = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        moved.contains("A rule about alpha.") && moved.contains("permalink: renamed"),
        "the saved text moved: {moved}"
    );
}

#[tokio::test]
async fn the_old_permalink_stops_resolving_after_a_rename() {
    // The stale key is gone rather than left pointing at the renamed room: a
    // client that asks for alpha gets the engine's own miss and falls back to
    // solo editing, instead of adopting a room whose content is now beta.
    let (_tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    rename_to_beta(&joined, &doc).await;

    let refused = sessions.join("eng", "alpha", None).await.unwrap_err();
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
    let (_tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
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
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "never lands").await;

    joined.session.poison().await;
    let closed = next_control(&mut joined.rx).await;
    let Control::Closed { reason, .. } = closed else {
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
    let again = sessions.join("eng", "alpha", None).await.unwrap();
    assert_ne!(again.session.epoch(), joined.session.epoch());
    assert_eq!(sessions.session_count().await, 1, "the corpse was replaced");
}

/// Unregistering a domain closes its rooms: the sweep saves each room's
/// text first (the files stay on disk, so in-flight edits must land),
/// then poisons so clients get a Closed control. Other domains' rooms
/// are untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispose_domain_saves_then_closes_only_that_domains_rooms() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined_eng = sessions.join("eng", "alpha", None).await.unwrap();
    let joined_oak = sessions.join("oak", "oak-note", None).await.unwrap();
    assert_eq!(sessions.session_count().await, 2);

    let doc = sync_client(&joined_eng).await;
    append_line(&joined_eng, &doc, "A swept edit").await;

    let closed = sessions.dispose_domain("eng").await;
    assert_eq!(closed, 1);
    assert_eq!(sessions.session_count().await, 1, "oak's room lives on");
    assert!(joined_eng.session.is_disposed());
    assert!(!joined_oak.session.is_disposed(), "oak was not swept");
    // The final save landed in the file that stays on disk.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.contains("A swept edit"), "{on_disk}");

    // The room heard the save land and only then heard it close: the order is
    // what keeps an in-flight edit out of the bin.
    assert!(matches!(
        next_control(&mut joined_eng.rx).await,
        Control::Saved { .. }
    ));
    let Control::Closed { reason, .. } = next_control(&mut joined_eng.rx).await else {
        panic!("the swept room is closed")
    };
    assert_eq!(reason, "internal");

    // A domain with no rooms is a quiet zero, not an error.
    assert_eq!(sessions.dispose_domain("ghost").await, 0);
}

#[tokio::test]
async fn a_second_refusal_with_a_new_reason_reaches_the_room() {
    // The room's alert must name the reason that applies NOW: a save-blocked
    // session that breaks a different way has to re-broadcast, or the author
    // keeps reading the refusal before last.
    let (_tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;

    // First: a document with no frontmatter at all.
    replace_all(&joined, &doc, "no frontmatter at all").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let Control::SaveFailed { detail: first } = next_control(&mut joined.rx).await else {
        panic!("the first refusal")
    };
    assert!(first.contains("frontmatter"), "{first}");

    // Then: frontmatter that is there but does not parse. Still refused, for
    // a different reason, and the flush skips the backoff.
    replace_all(&joined, &doc, "---\ntitle: [unclosed\n---\n\n# Alpha\n").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    let Control::SaveFailed { detail: second } = next_control(&mut joined.rx).await else {
        panic!("the second refusal is broadcast too")
    };
    assert_ne!(first, second, "the room is told the NEW reason");

    // A repeat of the same refusal, though, says nothing new: the next
    // control the room hears is the save that finally lands.
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(2 * SAVE_DEBOUNCE_MS))
        .await;
    replace_all(&joined, &doc, ALPHA).await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(3 * SAVE_DEBOUNCE_MS))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
}

// --- the room's text in a reviewing domain -----------------------------------

/// A room over the document a direct domain keeps - `overlay: None` - opens on
/// the text the team reviewed even where the domain reviews changes, whoever
/// else is drafting.
///
/// The other half of the key, said from the side nobody is drafting on: the
/// owner component is what makes a room somebody's draft, and a room opened
/// without one reads and writes exactly what every room read and wrote before
/// drafts existed. What a room over an OVERLAY document opens on, and where
/// its save lands, is
/// `an_overlay_sessions_save_lands_in_the_overlay_row_and_never_the_tree`
/// below.
#[tokio::test]
async fn a_room_in_a_reviewing_domain_still_opens_on_the_reviewed_text() {
    let scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("team");
    std::fs::create_dir_all(&dir).unwrap();
    write_manifest(&dir, "team");
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    let mut entry = DomainEntry::file(dir.clone());
    entry.review = Some(crystalline_core::config::ReviewMode::Overlay);
    cfg.domains.insert("team".to_string(), entry);
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();

    // The machine owner rewrites the engram, which in review mode is a draft
    // standing over the file rather than the file.
    let checksum = engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("team".to_string()),
                share_link: None,
                include_images: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap()["checksum"]
        .as_str()
        .unwrap()
        .to_string();
    engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "alpha".to_string(),
                content: ALPHA.replace("A rule about alpha.", "A draft about alpha."),
                expected_checksum: checksum,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("alpha.md")).unwrap(),
        ALPHA,
        "the folder still says what the team reviewed"
    );

    let sessions = CollabSessions::new(engine.clone());
    let joined = sessions.join("team", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    let text = doc.get_or_insert_text("content");
    let opened = text.get_string(&doc.transact());
    assert_eq!(
        opened, ALPHA,
        "and the room opens on it, not on anybody's draft"
    );
    drop(scratch);
}

/// A reviewing domain's room over one actor's overlay document: its text is
/// that actor's draft, and its save is a draft of theirs.
async fn review_fixture() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    let scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("team");
    std::fs::create_dir_all(&dir).unwrap();
    write_manifest(&dir, "team");
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    let mut entry = DomainEntry::file(dir);
    entry.review = Some(crystalline_core::config::ReviewMode::Overlay);
    cfg.domains.insert("team".to_string(), entry);
    // A second domain, taking changes directly, so a sweep over `team` has a
    // neighbour it must leave alone.
    let oak = root.join("oak");
    std::fs::create_dir_all(&oak).unwrap();
    write_manifest(&oak, "oak");
    std::fs::write(oak.join("oak-note.md"), OAK_NOTE).unwrap();
    cfg.domains
        .insert("oak".to_string(), DomainEntry::file(oak));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine, scratch)
}

/// What a room over an overlay document writes: the owner's draft row, and
/// never the folder the team reviewed.
///
/// The seam Task 13 moves, said as the two facts that matter to whoever is
/// typing: their work is theirs, and the page the team holds is untouched
/// until somebody folds it in.
#[tokio::test]
async fn an_overlay_sessions_save_lands_in_the_overlay_row_and_never_the_tree() {
    let (tmp, engine, _scratch) = review_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("team", "alpha", Some("alice")).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "alice typed this").await;

    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 1))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));

    let draft = engine
        .overlay_draft_at("team", "alice", "alpha.md")
        .await
        .unwrap()
        .expect("her draft is where the room's text landed");
    assert!(draft.content.contains("alice typed this"), "{draft:?}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("team/alpha.md")).unwrap(),
        ALPHA,
        "and the folder still says what the team reviewed"
    );
    assert!(
        engine
            .overlay_draft_at("team", "owner", "alpha.md")
            .await
            .unwrap()
            .is_none(),
        "nothing landed in the machine owner's draft, which is where it used to go"
    );
}

/// A sweep is about a domain, whatever the rooms in it are rooms over: the
/// owner component of the key must not hide a room from the verb that closes
/// them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispose_domain_sweeps_every_overlay_owners_room() {
    let (_tmp, engine, _scratch) = review_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let alice = sessions.join("team", "alpha", Some("alice")).await.unwrap();
    let bob = sessions.join("team", "alpha", Some("bob")).await.unwrap();
    let oak = sessions.join("oak", "oak-note", None).await.unwrap();
    assert_eq!(sessions.session_count().await, 3);

    assert_eq!(
        sessions.dispose_domain("team").await,
        2,
        "both owners' rooms are the domain's rooms"
    );
    assert!(alice.session.is_disposed());
    assert!(bob.session.is_disposed());
    assert!(!oak.session.is_disposed(), "and oak was not swept");
    assert_eq!(sessions.session_count().await, 1);
}

// ---------------------------------------------------------------------------
// domain name normalization: the Fluid editor's save path is the
// same funnel write_engram and edit_engram pass their final text through, not
// a route around it.
// ---------------------------------------------------------------------------

/// A domain registered locally as `eng-knowledge`, whose MANIFEST declares
/// `domain_name: eng` with the machine-local alias `engineering`, holding
/// `alpha`, synced. Mirrors `engine_writes.rs::normalize_fixture`, trimmed to
/// what the room save path needs; integration test crates share no helpers,
/// so it is written fresh here rather than imported.
async fn normalize_fixture() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    let scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("eng-knowledge");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        crystalline_core::manifest_template("eng", "2026-01-01"),
    )
    .unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    let mut entry = DomainEntry::file(dir);
    entry.aliases = vec!["engineering".to_string()];
    cfg.domains.insert("eng-knowledge".to_string(), entry);
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
    (tmp, engine, scratch)
}

/// The room's own saver (`Engine::save_engram`, the same verb the Fluid
/// editor's PUT reaches) normalizes a cross-domain link the same way
/// `write_engram` and `edit_engram` do: `[[engineering:runbook]]`, spelled
/// with the machine-local alias, lands as `[[eng:runbook]]`, the canonical
/// name its MANIFEST declares. The rewrite is CONVERGED into the room's own
/// document before the save (`Control::Merged` arrives first), not left for
/// the engine's own pass to rewrite silently underneath the person looking at
/// the page.
///
/// Normalization means the receipt's checksum is of bytes the room never
/// itself computed (the room's own text still reads `engineering` until the
/// converge lands): a second edit and save has to still succeed on the CAS
/// the first save's receipt handed back, not fall into the external-change
/// path over a mismatch this change introduced. Both saves are asserted
/// `Saved` here for exactly that reason; the second carries no `Merged` of
/// its own, since by then the document already reads canonical and the
/// normalizing pass finds nothing left to do.
#[tokio::test]
async fn a_room_save_normalizes_a_cross_domain_link_to_the_canonical_name() {
    let (tmp, engine, _scratch) = normalize_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng-knowledge", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "See [[engineering:runbook]] for more.").await;

    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(
        matches!(next_control(&mut joined.rx).await, Control::Merged),
        "the normalizing rewrite converges into the document first"
    );
    assert!(
        matches!(next_control(&mut joined.rx).await, Control::Saved { .. }),
        "then the first save lands"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng-knowledge/alpha.md")).unwrap();
    assert!(
        on_disk.contains("[[eng:runbook]]"),
        "the room's save normalizes the same way write_engram and edit_engram do: {on_disk}"
    );

    // A second edit and save: the room's next CAS token is the receipt's own
    // checksum (of the normalized bytes actually on disk), not a hash the
    // room computed over its own unnormalized text, so this still succeeds
    // rather than reading as an external change.
    append_line(&joined, &doc, "and a second line").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(2 * SAVE_DEBOUNCE_MS + 200))
        .await;
    assert!(
        matches!(next_control(&mut joined.rx).await, Control::Saved { .. }),
        "the second save also lands, not a conflict, and carries no Merged of its own"
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng-knowledge/alpha.md")).unwrap();
    assert!(on_disk.contains("[[eng:runbook]]"), "{on_disk}");
    assert!(on_disk.contains("and a second line"), "{on_disk}");
}

/// Three things the converge step promises once a normalizing save has
/// landed: the room's own document (not only the file) reads the canonical
/// spelling and is clean (`snapshot`'s `dirty` is false); a further tick with
/// nothing new typed writes nothing at all, since `file == last_saved_text`
/// holds once both sides hold the same normalized text (setting
/// `last_saved_text` without converging the document would leave that compare
/// false forever, and the room would save on every debounce window for
/// good); and an external edit elsewhere in the body, racing the room's own
/// unsaved edit, still three-way merges cleanly - the extra converge step
/// this fix adds is not itself an edit the room's own change detection ever
/// sees as new, so it never collides with a legitimate external merge.
#[tokio::test]
async fn a_converged_room_settles_and_still_merges_an_external_edit_cleanly() {
    let (tmp, engine, _scratch) = normalize_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng-knowledge", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "See [[engineering:runbook]] for more.").await;

    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Merged
    ));
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));

    let (snapshot_text, dirty) = joined.session.snapshot().await;
    assert!(
        snapshot_text.contains("[[eng:runbook]]"),
        "the document itself agrees with the canonical spelling, not only the \
         file: {snapshot_text}"
    );
    assert!(!dirty, "and settles clean: {snapshot_text}");

    let path = tmp.path().join("eng-knowledge/alpha.md");
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(2 * SAVE_DEBOUNCE_MS + 200))
        .await;
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before,
        "nothing new was typed, so the already-converged document never re-saves"
    );

    // The room holds an unsaved edit of its own (a new line at the very end)
    // while an external write lands elsewhere - the frontmatter, well clear
    // of the body: a clean three-way merge, not a conflict.
    append_line(&joined, &doc, "a further thought").await;
    let external = std::fs::read_to_string(&path)
        .unwrap()
        .replace("status: stable", "status: draft");
    std::fs::write(&path, &external).unwrap();
    engine.sync(Some("eng-knowledge")).await.unwrap();

    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(3 * SAVE_DEBOUNCE_MS + 300))
        .await;
    let first = next_control(&mut joined.rx).await;
    assert!(
        matches!(first, Control::Merged),
        "the external change merges cleanly rather than raising a conflict: {first:?}"
    );
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.contains("status: draft"), "{on_disk}");
    assert!(
        on_disk.contains("[[eng:runbook]]"),
        "the canonical spelling this fix introduced survives the merge: {on_disk}"
    );
    assert!(on_disk.contains("a further thought"), "{on_disk}");
}

/// A captured audience with a room open: a rename and a removal each close the rooms of
/// the domain first, and the save a closing room lands goes out under the old
/// name. It must carry the audience captured before the room closed, never
/// the ordinary check, which after the records move would no longer hide the
/// name. Catches a capture taken after `close_editors` or `dispose_domain`.
#[tokio::test]
async fn a_room_saved_by_a_rename_or_a_removal_carries_the_captured_audience() {
    use crystalline_service::changes::{Change, DomainAudience};
    use crystalline_service::rest::{AuthStore, MemberLevel, Role};
    use std::collections::HashSet;

    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for (name, role) in [("keeper", Role::Editor), ("mem", Role::Viewer)] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    for domain in ["eng", "oak"] {
        auth.set_domain_visibility(domain, true, "keeper")
            .await
            .unwrap();
    }
    auth.upsert_domain_member("eng", "mem", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(crystalline_service::DomainAccess::new(auth)));
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let readers = |names: &[&str]| {
        DomainAudience::Accounts(names.iter().map(|n| n.to_string()).collect::<HashSet<_>>())
    };
    let room_save = |heard: &[Change], domain: &str, path: &str| {
        heard
            .iter()
            .find(|c| matches!(c, Change::Engram(e) if e.domain == domain && e.path == path))
            .cloned()
            .unwrap_or_else(|| panic!("no room save for {domain}/{path}: {heard:?}"))
    };

    // A rename closes the room over eng's alpha with a typed line unsaved.
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before the rename").await;
    let mut rx = engine.changes().subscribe();
    engine
        .rename_domain(
            "eng",
            "elm",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let mut heard = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        heard.push(envelope.change);
    }
    let saved = room_save(&heard, "eng", "alpha.md");
    assert_eq!(saved.audience(), Some(&readers(&["keeper", "mem"])));
    drop(joined);

    // A removal closes the room over oak's note the same way.
    let joined = sessions.join("oak", "oak-note", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before the removal").await;
    engine
        .unregister_domain("oak", &crystalline_service::Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    let mut heard = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        heard.push(envelope.change);
    }
    let saved = room_save(&heard, "oak", "oak-note.md");
    assert_eq!(saved.audience(), Some(&readers(&["keeper"])));
}

/// A domain rename moves every document in it: each room saves first and is
/// told `moved` with the new domain, the same path and the same permalink.
#[tokio::test]
async fn a_domain_rename_tells_its_rooms_the_new_name() {
    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before the rename").await;
    engine
        .rename_domain(
            "eng",
            "elm",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: Some("elm".into()),
            path: Some("alpha.md".into()),
            permalink: Some("alpha".into()),
        }
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.contains("typed before the rename"), "{on_disk}");
}

/// A full rename (the MANIFEST takes the new name too) tells its rooms the
/// new name the same way, once it is done.
#[tokio::test]
async fn a_full_domain_rename_tells_its_rooms_the_new_name() {
    let (tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before the full rename").await;
    engine
        .rename_domain(
            "eng",
            "elm",
            false,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: Some("elm".into()),
            path: Some("alpha.md".into()),
            permalink: Some("alpha".into()),
        }
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("typed before the full rename"),
        "{on_disk}"
    );
}

/// A full rename that stops after its MANIFEST and link steps (a write in
/// the domain did not finish in time) is not done: the old name still
/// serves, so its rooms are told `internal` and no new name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_rename_that_stops_does_not_tell_its_rooms_a_new_name() {
    let (_tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let _doc = sync_client(&joined).await;
    engine.set_rename_drain_wait(Some(Duration::from_millis(50)));
    let write_hold = engine.hold_next_write();
    let writer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .write_engram(&crystalline_service::params::WriteParams {
                    path: None,
                    base64: None,
                    domain: "eng".into(),
                    title: "Beta".into(),
                    content: Some("written while the rename waits".into()),
                    folder: None,
                    engram_type: None,
                    tags: Vec::new(),
                    status: None,
                    metadata: None,
                    overwrite: false,
                    share_link: None,
                    model: None,
                })
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), write_hold.reached())
        .await
        .expect("the write is counted and held");
    let stopped = engine
        .rename_domain(
            "eng",
            "elm",
            false,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(stopped.contains("busy"), "{stopped}");
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::closed("internal")
    );
    write_hold.release();
    writer.await.unwrap().unwrap();
}

/// A rename refused after it swept the rooms (a write in the domain did not
/// finish in time) moved nothing, so its rooms are told `internal` and no
/// new name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_rename_does_not_tell_its_rooms_a_new_name() {
    let (_tmp, engine, _scratch) = engine_fixture_with_state().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let _doc = sync_client(&joined).await;
    engine.set_rename_drain_wait(Some(Duration::from_millis(50)));
    let write_hold = engine.hold_next_write();
    let writer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .write_engram(&crystalline_service::params::WriteParams {
                    path: None,
                    base64: None,
                    domain: "eng".into(),
                    title: "Beta".into(),
                    content: Some("written while the rename waits".into()),
                    folder: None,
                    engram_type: None,
                    tags: Vec::new(),
                    status: None,
                    metadata: None,
                    overwrite: false,
                    share_link: None,
                    model: None,
                })
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), write_hold.reached())
        .await
        .expect("the write is counted and held");
    let refused = engine
        .rename_domain(
            "eng",
            "elm",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("busy"), "{refused}");
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::closed("internal")
    );
    write_hold.release();
    writer.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_reflection_room_saves_straight_to_the_file_and_assigns_ids_on_close() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "\n## Three\n\nTyped in the room.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("## Three\n\nTyped in the room."), "{saved}");
    assert!(
        !saved.contains("## Three\n<!-- r:"),
        "no id under the cursor while the room is open"
    );
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    let closed = std::fs::read_to_string(&path).unwrap();
    assert!(
        closed.contains("## Three\n<!-- r:"),
        "the id comes with the close: {closed}"
    );
}

/// A room's save is announced as a `changed` reflection event naming its
/// one typist.
#[tokio::test]
async fn a_reflection_room_save_is_announced_with_its_typist() {
    use crystalline_service::changes::{Change, ReflectionChangeKind};
    let (_tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut rx = engine.changes().subscribe();
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed in the room.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let mut heard = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let Change::Reflection(change) = envelope.change {
            heard.push((change.kind, change.path, change.actor));
        }
    }
    assert_eq!(
        heard,
        vec![(
            ReflectionChangeKind::Changed,
            "guides/a.md".to_string(),
            Some("ada".to_string())
        )]
    );
}

/// A room closed without typing writes nothing, so it announces nothing.
/// The fixture's page has no provenance block yet, which the first close
/// writes; the second room finds nothing left to write.
#[tokio::test]
async fn a_room_close_that_writes_nothing_announces_nothing() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let close_untouched = || async {
        let joined = sessions
            .join_reflection("eng", "guides/a.md", "ada")
            .await
            .unwrap();
        let _doc = sync_client(&joined).await;
        assert!(joined.session.remove_conn(joined.conn).await);
        joined.session.final_save().await;
        sessions.dispose_if_empty(&joined.session).await;
    };
    close_untouched().await;
    let before = std::fs::read(&path).unwrap();
    let mut rx = engine.changes().subscribe();
    close_untouched().await;
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing was written");
    let mut heard = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let crystalline_service::changes::Change::Reflection(change) = envelope.change {
            heard.push(change);
        }
    }
    assert!(heard.is_empty(), "{heard:?}");
}

#[tokio::test]
async fn two_people_in_one_reflection_room_merge_and_the_baseline_is_untouched() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let baseline = tmp.path().join("eng/reflections/.reflected/guides/a.md");
    let before = std::fs::read(&baseline).unwrap();
    let sessions = CollabSessions::new(engine.clone());
    let mut first = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let second = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    assert!(
        Arc::ptr_eq(&first.session, &second.session),
        "one room per reflection"
    );
    let doc_a = sync_client(&first).await;
    let doc_b = sync_client(&second).await;
    append_line(&first, &doc_a, "Added by the first person.").await;
    append_line(&second, &doc_b, "Added by the second person.").await;
    first
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut first.rx).await,
        Control::Saved { .. }
    ));
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(
        saved.contains("Added by the first person.")
            && saved.contains("Added by the second person."),
        "{saved}"
    );
    assert_eq!(
        std::fs::read(&baseline).unwrap(),
        before,
        "people's edits never move the baseline"
    );
}

#[tokio::test]
async fn an_agent_write_composes_into_an_open_reflection_room_and_a_move_closes_it() {
    let (_tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let _doc = sync_client(&joined).await;
    let current = engine
        .read_reflection(
            "eng",
            "guides/a.md",
            &crystalline_service::ReflectionCaller::owner(),
        )
        .await
        .unwrap();
    let markdown = current["markdown"]
        .as_str()
        .unwrap()
        .replace("Second.", "Second, revised by an agent.");
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        markdown: Some(markdown),
        ..Default::default()
    };
    let caller = crystalline_service::ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    );
    let written = engine.write_reflection(&p, &caller).await.unwrap();
    assert_eq!(written["landed"], "live");
    let (live, _) = joined.session.snapshot().await;
    assert!(live.contains("Second, revised by an agent."));
    let mut joined = joined;
    assert!(
        matches!(next_control(&mut joined.rx).await, Control::Merged),
        "the room is told the agent's text is in"
    );
    let file = engine
        .reflection_room_text("eng", "guides/a.md")
        .await
        .unwrap();
    assert_eq!(
        joined.session.live().await.checksum,
        file.checksum,
        "the room took the file in and holds its checksum"
    );
    let err = engine
        .delete_reflection("eng", "guides/a.md", &caller)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("somebody is editing"), "a delete waits: {err}");
    // A move does not wait: the room saves, is told where the page went and
    // closes, and the moved page holds the agent's text.
    engine
        .move_reflection("eng", "guides/a.md", "b.md", &caller)
        .await
        .unwrap();
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: Some("b.md".into()),
            permalink: None,
        }
    );
    let moved = engine.reflection_room_text("eng", "b.md").await.unwrap();
    assert!(moved.content.contains("Second, revised by an agent."));
}

/// A rewrite never races a room: a file or a page cannot move while a page
/// that links it is open, and a refused move changes nothing.
#[tokio::test]
async fn a_file_or_page_move_waits_while_a_page_that_links_it_is_open() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let dir = tmp.path().join("eng/reflections");
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/a.png"), "png").unwrap();
    std::fs::write(
        dir.join("b.md"),
        "# B\n\n![a](assets/a.png) and [page](guides/a.md)\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions
        .join_reflection("eng", "b.md", "ada")
        .await
        .unwrap();
    let _doc = sync_client(&joined).await;
    let owner = crystalline_service::ReflectionCaller::owner();
    let err = engine
        .move_reflection_asset("eng", "assets/a.png", "assets/c.png", &owner)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("somebody is editing 'b.md'"), "{err}");
    assert!(
        dir.join("assets/a.png").is_file(),
        "a refused move changes nothing"
    );
    let err = engine
        .move_reflection("eng", "guides/a.md", "c.md", &owner)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("somebody is editing 'b.md'"), "{err}");
    assert!(dir.join("guides/a.md").is_file());
}

/// A page whose only frontmatter key is the bookkeeping block naming an
/// engram: the riskiest round trip, since cutting the block drops the fences.
const WITH_SOURCES: &str = "---\nreflection:\n  aaaa1111:\n    sources:\n      - engram: runbooks/deploy\n        checksum: 00ff\n    reflected: 11ee\n---\n\n# A\n\n## One\n<!-- r:aaaa1111 -->\n\nFirst.\n\n## Two\n<!-- r:bbbb2222 -->\n\nSecond.\n";

#[tokio::test]
async fn a_reflection_room_never_carries_the_provenance_block() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    std::fs::write(&path, WITH_SOURCES).unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();

    // What every socket receives: the page without the block.
    let doc = sync_client(&joined).await;
    let seen = doc
        .get_or_insert_text("content")
        .get_string(&doc.transact());
    assert!(
        !seen.contains("runbooks/deploy") && !seen.contains("reflection:"),
        "{seen}"
    );
    assert!(
        seen.starts_with("# A\n"),
        "the fences go with their only key: {seen}"
    );

    // A person's save keeps the stored block in the file.
    append_line(&joined, &doc, "Typed in the room.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("engram: runbooks/deploy") && saved.contains("Typed in the room."),
        "{saved}"
    );

    // An agent's write lands in the room without the block.
    let markdown = saved.replace("Second.", "Second, from the agent.");
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        markdown: Some(markdown),
        ..Default::default()
    };
    let caller = crystalline_service::ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    );
    assert_eq!(
        engine.write_reflection(&p, &caller).await.unwrap()["landed"],
        "live"
    );
    let (live, _) = joined.session.snapshot().await;
    assert!(
        live.contains("Second, from the agent.") && !live.contains("runbooks/deploy"),
        "{live}"
    );
    assert!(
        matches!(next_control(&mut joined.rx).await, Control::Merged),
        "the room is told the agent's text is in"
    );

    // The room took the file in, and its next save is not stale: the
    // checksum it holds is the file's, not the stripped text's.
    let file = engine
        .reflection_room_text("eng", "guides/a.md")
        .await
        .unwrap();
    assert_eq!(joined.session.live().await.checksum, file.checksum);
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed after the agent.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 200))
        .await;
    assert!(
        matches!(next_control(&mut joined.rx).await, Control::Saved { .. }),
        "the next save is not stale"
    );
    let last = std::fs::read_to_string(&path).unwrap();
    assert!(
        last.contains("engram: runbooks/deploy") && last.contains("Typed after the agent."),
        "{last}"
    );
    assert_eq!(
        last.matches("reflection:").count(),
        1,
        "the block is re-attached once, not stacked: {last}"
    );
}

/// A reflection room is its own kind of key. An engram lookup under
/// the same name never finds it, whatever the name.
#[tokio::test]
async fn a_reflection_room_never_answers_an_engram_lookup() {
    use crystalline_service::collab::session::RoomDoc;
    let (_tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let _joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    assert!(
        sessions
            .has_live_doc("eng", &RoomDoc::Reflection("guides/a.md".into()), None)
            .await
    );
    assert!(
        !sessions.has_live_room("eng", "guides/a.md", None).await,
        "no engram room by that name"
    );
    assert!(
        !sessions
            .has_live_doc("eng", &RoomDoc::Engram("guides/a.md".into()), None)
            .await
    );
}

/// The read-only freeze reaches the rooms: no reflection
/// room opens on a read-only domain, and the file is untouched.
#[tokio::test]
async fn a_read_only_domain_refuses_a_reflection_room() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let manifest_path = tmp.path().join("eng/MANIFEST.md");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap().replacen(
        "reflection: on\n",
        "reflection: on\nread_only: on\n",
        1,
    );
    std::fs::write(&manifest_path, manifest).unwrap();
    engine.sync(None).await.unwrap();
    let before = std::fs::read(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    let sessions = CollabSessions::new(engine.clone());
    assert!(
        sessions
            .join_reflection("eng", "guides/a.md", "ada")
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(tmp.path().join("eng/reflections/guides/a.md")).unwrap(),
        before
    );
}

/// Open point 2 (approved): a room's save spells other domains by their
/// canonical name, and the room converges to the spelling that landed.
#[tokio::test]
async fn a_reflection_room_save_spells_other_domains_by_their_canonical_name() {
    let (tmp, engine, _scratch) = reflection_fixture_with_alias("engineering").await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "See [[engineering:runbooks/deploy]].").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    // The respelling reaches the room before the save (as in an engram
    // room), then the save lands.
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Merged
    ));
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(saved.contains("[[eng:runbooks/deploy]]"), "{saved}");
    let (live, _) = joined.session.snapshot().await;
    assert!(
        live.contains("[[eng:runbooks/deploy]]"),
        "the room shows what landed: {live}"
    );
}

/// A page deleted under an open room is the room's deletion conflict, as an
/// engram's is: the room's next save never quietly puts it back.
#[tokio::test]
async fn a_reflection_deleted_under_its_room_is_a_conflict_not_a_resurrection() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed before the delete.").await;
    std::fs::remove_file(&path).unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    match next_control(&mut joined.rx).await {
        Control::Conflict { conflict_kind, .. } => assert_eq!(conflict_kind, "deleted"),
        other => panic!("a deletion conflict, not {other:?}"),
    }
    assert!(!path.exists(), "the save did not bring the page back");
}

/// A room opened and closed with nothing typed leaves a page Crystalline
/// wrote as it was.
#[tokio::test]
async fn a_reflection_room_closed_untouched_writes_nothing() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        markdown: Some(std::fs::read_to_string(&path).unwrap()),
        ..Default::default()
    };
    engine
        .write_reflection(&p, &crystalline_service::ReflectionCaller::owner())
        .await
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    // An old modification time, so a rewrite of the same bytes shows.
    let stamp = std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    let sessions = CollabSessions::new(engine.clone());
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let _doc = sync_client(&joined).await;
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        stamp,
        "the file was not written at all"
    );
}

/// A room saved on close and kept open by somebody still in it mints no id
/// under the next cursor: the final pass ends with that save.
#[tokio::test]
async fn a_reflection_room_kept_open_after_its_close_save_mints_no_ids() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    joined.session.final_save().await;
    append_line(&joined, &doc, "\n## Four\n\nLater.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    loop {
        if let Control::Saved { .. } = next_control(&mut joined.rx).await {
            break;
        }
    }
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("## Four\n\nLater."), "{saved}");
    assert!(
        !saved.contains("## Four\n<!-- r:"),
        "no id under the cursor of a room still open: {saved}"
    );
}

/// Read controls until the room reports a landed save.
async fn until_saved(rx: &mut tokio::sync::broadcast::Receiver<Frame>) {
    loop {
        match next_control(rx).await {
            Control::Saved { .. } => return,
            Control::Merged => {}
            other => panic!("a save, not {other:?}"),
        }
    }
}

/// An agent's section write into a page somebody is typing in: the file
/// is merged into the room's own text, so the unsaved typing stays and
/// lands beside the agent's change.
#[tokio::test]
async fn an_agent_write_into_an_open_room_keeps_a_persons_unsaved_typing() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed by a person.").await;
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## One".into()),
        content: Some("First, from the agent.\n".into()),
        ..Default::default()
    };
    let caller = crystalline_service::ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    );
    assert_eq!(
        engine.write_reflection(&p, &caller).await.unwrap()["landed"],
        "live"
    );
    let (live, _) = joined.session.snapshot().await;
    assert!(
        live.contains("First, from the agent.") && live.contains("Typed by a person."),
        "both in the room: {live}"
    );
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    until_saved(&mut joined.rx).await;
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(
        saved.contains("First, from the agent.") && saved.contains("Typed by a person."),
        "both on disk: {saved}"
    );
}

/// The room saves between an agent's file write and the room taking it in
/// (the write drops the page lock before it reaches the room): the room's
/// save already merged the agent's change, so taking it in again changes
/// nothing, and neither the typing nor the agent's text is lost.
#[tokio::test]
async fn a_room_save_between_an_agent_write_and_its_merge_loses_nothing() {
    use crystalline_service::collab::session::RoomDoc;
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    // The rooms are not handed to the engine yet, so the write lands in the
    // file alone, as it does in the window before its live arm runs.
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed by a person.").await;
    let markdown = std::fs::read_to_string(&path)
        .unwrap()
        .replace("First.", "First, from the agent.");
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        markdown: Some(markdown),
        ..Default::default()
    };
    let caller = crystalline_service::ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    );
    engine.write_reflection(&p, &caller).await.unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    until_saved(&mut joined.rx).await;
    // The live arm's step, after the room's save.
    sessions
        .absorb_doc_change("eng", &RoomDoc::Reflection("guides/a.md".into()), None)
        .await
        .unwrap();
    let (live, dirty) = joined.session.snapshot().await;
    assert!(
        live.contains("First, from the agent.") && live.contains("Typed by a person."),
        "{live}"
    );
    assert!(!dirty, "nothing left to save");
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(
        saved.contains("First, from the agent.") && saved.contains("Typed by a person."),
        "{saved}"
    );
}

/// Departure 4: a room save respells a section nobody touched as a link
/// rewrite, so that section records nobody, and the baseline follows.
#[tokio::test]
async fn a_room_save_respells_an_untouched_section_without_recording_anybody() {
    let (tmp, engine, _scratch) = reflection_fixture_with_alias("engineering").await;
    let page = tmp.path().join("eng/reflections/guides/a.md");
    let baseline = tmp.path().join("eng/reflections/.reflected/guides/a.md");
    std::fs::write(
        &page,
        "# A\n\n## One\n<!-- r:aaaa1111 -->\n\nSee [[engineering:runbooks/deploy]].\n\n## Two\n<!-- r:bbbb2222 -->\n\nSecond.\n",
    )
    .unwrap();
    std::fs::write(
        &baseline,
        "## One\n<!-- r:aaaa1111 -->\n\nSee [[engineering:runbooks/deploy]].\n\n",
    )
    .unwrap();
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed in Two.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    until_saved(&mut joined.rx).await;
    let saved = std::fs::read_to_string(&page).unwrap();
    assert!(saved.contains("See [[eng:runbooks/deploy]]."), "{saved}");
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    assert!(
        prov.get("aaaa1111")
            .is_none_or(|record| record.changed_by.is_empty()),
        "the untouched section records nobody: {saved}"
    );
    assert!(
        !prov["bbbb2222"].changed_by.is_empty(),
        "the typed section records its author: {saved}"
    );
    assert!(
        std::fs::read_to_string(&baseline)
            .unwrap()
            .contains("See [[eng:runbooks/deploy]]."),
        "the baseline gets the same respelling"
    );
}

/// A settle rewrites only the bookkeeping block while a room is open; the
/// room's next save keeps what the settle stamped.
#[tokio::test]
async fn a_settle_under_an_open_room_survives_its_next_save() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    engine
        .settle_reflection(
            "eng",
            "guides/a.md",
            &["bbbb2222".to_string()],
            "kept",
            &crystalline_service::ReflectionCaller::owner(),
        )
        .await
        .unwrap();
    let settled = crystalline_core::reflection::provenance::read_provenance(
        &std::fs::read_to_string(&path).unwrap(),
    )["bbbb2222"]
        .reflected
        .clone();
    assert!(settled.is_some());
    append_line(&joined, &doc, "Typed after the settle.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    until_saved(&mut joined.rx).await;
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("Typed after the settle."), "{saved}");
    assert_eq!(
        crystalline_core::reflection::provenance::read_provenance(&saved)["bbbb2222"].reflected,
        settled,
        "the settle's stamp stays: {saved}"
    );
}

/// The engine refuses a room over a path that names no reflection, whoever
/// asks for it.
#[tokio::test]
async fn a_reflection_room_takes_only_a_reflection_path() {
    let (_tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    for path in ["../MANIFEST.md", ".reflected/guides/a.md", "guides/a.txt"] {
        assert!(
            sessions.join_reflection("eng", path, "ada").await.is_err(),
            "{path}"
        );
    }
}

/// The agent's section write collides with a person's unsaved change of
/// the same line: the room raises its edit conflict with the agent's text
/// as theirs, keeps the person's text, and the receipt says the write is
/// on disk and the room is choosing, not that it landed live.
#[tokio::test]
async fn an_agent_write_that_collides_with_unsaved_typing_raises_the_rooms_conflict() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    let mine = doc
        .get_or_insert_text("content")
        .get_string(&doc.transact())
        .replace("First.", "First, by a person.");
    replace_all(&joined, &doc, &mine).await;
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## One".into()),
        content: Some("First, from the agent.\n".into()),
        ..Default::default()
    };
    let caller = crystalline_service::ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    );
    let receipt = engine.write_reflection(&p, &caller).await.unwrap();
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "conflict", "{receipt}");
    assert!(receipt["room_note"].as_str().unwrap().contains("choosing"));
    match next_control(&mut joined.rx).await {
        Control::Conflict {
            conflict_kind,
            theirs,
            ..
        } => {
            assert_eq!(conflict_kind, "edit");
            assert!(theirs.unwrap().contains("First, from the agent."));
        }
        other => panic!("the room's edit conflict, not {other:?}"),
    }
    let (live, _) = joined.session.snapshot().await;
    assert!(live.contains("First, by a person."), "not erased: {live}");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("First, from the agent."),
        "the agent's write is on disk"
    );
}

/// A room with a conflict of its own takes nothing in: the agent's write is
/// on disk, and the receipt says the room meets it when its conflict is
/// resolved.
#[tokio::test]
async fn an_agent_write_into_a_room_in_conflict_is_reported_as_waiting() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    // A change outside Crystalline collides with unsaved typing on the same
    // line: the room's save raises its edit conflict.
    let mine = doc
        .get_or_insert_text("content")
        .get_string(&doc.transact())
        .replace("First.", "First, by a person.");
    replace_all(&joined, &doc, &mine).await;
    let outside = std::fs::read_to_string(&path)
        .unwrap()
        .replace("First.", "First, from outside.");
    std::fs::write(&path, outside).unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Conflict { .. }
    ));
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## Two".into()),
        content: Some("Second, from the agent.\n".into()),
        ..Default::default()
    };
    let caller = crystalline_service::ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    );
    let receipt = engine.write_reflection(&p, &caller).await.unwrap();
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    let (live, _) = joined.session.snapshot().await;
    assert!(
        live.contains("First, by a person.") && !live.contains("Second, from the agent."),
        "the room took nothing in: {live}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Second, from the agent.")
    );
}

/// The agent caller the room-path tests below write and move as.
fn room_agent() -> crystalline_service::ReflectionCaller {
    crystalline_service::ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    )
}

/// An agent's write into a room whose own save failed lands in the file only:
/// the room is `Failed`, takes nothing in, and the receipt says it is waiting.
#[tokio::test]
async fn an_agent_write_into_a_room_with_a_failed_save_is_deferred() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed by a person.").await;
    engine.fail_next_reflection_index();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(
        matches!(
            next_control(&mut joined.rx).await,
            Control::SaveFailed { .. }
        ),
        "the room's save fails"
    );
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## Two".into()),
        content: Some("Second, from the agent.\n".into()),
        ..Default::default()
    };
    let receipt = engine.write_reflection(&p, &room_agent()).await.unwrap();
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    assert_eq!(
        receipt["room_note"],
        crystalline_service::engine::ROOM_WAITING_NOTE,
        "{receipt}"
    );
    let (live, _) = joined.session.snapshot().await;
    assert!(
        live.contains("Typed by a person.") && !live.contains("Second, from the agent."),
        "the room took nothing in: {live}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Second, from the agent.")
    );
}

/// A room whose page is already gone defers an absorb instead of failing it,
/// and its next save raises the deletion conflict.
#[tokio::test]
async fn a_room_whose_page_is_gone_defers_the_change() {
    use crystalline_service::collab::session::{Absorbed, RoomDoc};
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed before the delete.").await;
    std::fs::remove_file(&path).unwrap();
    let outcome = sessions
        .absorb_doc_change("eng", &RoomDoc::Reflection("guides/a.md".into()), None)
        .await
        .unwrap();
    assert_eq!(outcome.absorbed, Absorbed::Deferred);
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    match next_control(&mut joined.rx).await {
        Control::Conflict { conflict_kind, .. } => assert_eq!(conflict_kind, "deleted"),
        other => panic!("a deletion conflict, not {other:?}"),
    }
    assert!(!path.exists(), "no save brought the page back");
}

/// R81: a room that opens after a delete's screen and before its page lock is
/// not refused; it meets the deletion as a conflict and keeps its text.
#[tokio::test]
async fn a_room_opened_inside_the_delete_window_meets_the_deletion() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (entered, release) = engine.hold_next_room_screen_until();
    let deleting = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .delete_reflection("eng", "guides/a.md", &room_agent())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the verb reaches the gate");
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed inside the window.").await;
    release.notify_one();
    deleting.await.unwrap().unwrap();
    assert!(!path.exists(), "the delete went through");
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    match next_control(&mut joined.rx).await {
        Control::Conflict { conflict_kind, .. } => assert_eq!(conflict_kind, "deleted"),
        other => panic!("a deletion conflict, not {other:?}"),
    }
    assert!(!path.exists(), "the save did not bring the page back");
    let (live, _) = joined.session.snapshot().await;
    assert!(live.contains("Typed inside the window."), "{live}");
}

/// R81 for a move: the window between the move's close and its page lock is
/// closed by the move's fence. A join inside it is refused, the moved page
/// holds the text that moved, and the new path opens a room on it.
#[tokio::test]
async fn a_room_cannot_open_inside_the_move_window() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let old = tmp.path().join("eng/reflections/guides/a.md");
    let new = tmp.path().join("eng/reflections/guides/b.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (entered, release) = engine.hold_next_room_screen_until();
    let moving = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the verb reaches the gate");
    let refused = match sessions.join_reflection("eng", "guides/a.md", "ada").await {
        Err(refused) => refused.to_string(),
        Ok(_) => panic!("a room opened inside the move window"),
    };
    assert!(
        refused.contains("'guides/a.md' is being moved"),
        "{refused}"
    );
    release.notify_one();
    moving.await.unwrap().unwrap();
    assert!(!old.exists(), "the old path stays empty");
    assert!(std::fs::read_to_string(&new).unwrap().contains("First."));
    // The fence is gone: the old path answers its own miss, not the move.
    let gone = match sessions.join_reflection("eng", "guides/a.md", "ada").await {
        Err(gone) => gone.to_string(),
        Ok(_) => panic!("the old path holds no page"),
    };
    assert!(!gone.contains("being moved"), "{gone}");
    let joined = sessions
        .join_reflection("eng", "guides/b.md", "ada")
        .await
        .unwrap();
    let (live, _) = joined.session.snapshot().await;
    assert!(live.contains("First."), "{live}");
}

/// A room open over a page when it moves saves its typing first and is told
/// the new path; the old path is gone and nothing typed is lost.
#[tokio::test]
async fn a_moved_pages_room_saves_first_and_is_told_the_new_path() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let old = tmp.path().join("eng/reflections/guides/a.md");
    let new = tmp.path().join("eng/reflections/guides/b.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed just before the move.").await;
    engine
        .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
        .await
        .unwrap();
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: Some("guides/b.md".into()),
            permalink: None,
        }
    );
    assert!(joined.session.is_disposed());
    assert!(!old.exists());
    let moved = std::fs::read_to_string(&new).unwrap();
    assert!(moved.contains("Typed just before the move."), "{moved}");
}

/// Whether a `Closed` control is waiting on this receiver right now, without
/// waiting for one.
fn closed_waiting(rx: &mut tokio::sync::broadcast::Receiver<Frame>) -> bool {
    while let Ok(frame) = rx.try_recv() {
        let mut decoder = DecoderV1::from(frame.bytes.as_ref());
        for message in MessageReader::new(&mut decoder).flatten() {
            if let Message::Custom(control::CONTROL_TAG, payload) = message
                && matches!(control::decode(&payload), Some(Control::Closed { .. }))
            {
                return true;
            }
        }
    }
    false
}

/// A page move onto a path that already holds a page is refused by the
/// screen, before the room is touched: no close is sent and the room goes on
/// saving.
#[tokio::test]
async fn a_page_move_onto_a_taken_path_leaves_the_room_open() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let taken = tmp.path().join("eng/reflections/guides/b.md");
    std::fs::write(&taken, "# B\n\nSomebody else's page.\n").unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed before the refused move.").await;
    let err = engine
        .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("already holds a reflection"), "{err}");
    assert!(!closed_waiting(&mut joined.rx), "no close was sent");
    assert!(!joined.session.is_disposed(), "the room stays open");
    assert_eq!(
        std::fs::read_to_string(&taken).unwrap(),
        "# B\n\nSomebody else's page.\n"
    );
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(saved.contains("Typed before the refused move."), "{saved}");
}

/// A move that fails after it closed the room (here the target is taken
/// between the screen and the page lock) did not move the page: the room is
/// told `internal`, never `moved`, and its text was saved at the old path.
#[tokio::test]
async fn a_move_that_fails_after_the_close_tells_the_room_internal() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let old = tmp.path().join("eng/reflections/guides/a.md");
    let taken = tmp.path().join("eng/reflections/guides/b.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed before the failed move.").await;
    let (entered, release) = engine.hold_next_room_screen_until();
    let moving = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the verb reaches the gate");
    std::fs::write(&taken, "# B\n\nWritten in the window.\n").unwrap();
    release.notify_one();
    let err = moving.await.unwrap().unwrap_err().to_string();
    assert!(err.contains("already holds a reflection"), "{err}");
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::closed("internal")
    );
    assert!(
        std::fs::read_to_string(&old)
            .unwrap()
            .contains("Typed before the failed move."),
        "the room saved at the old path"
    );
    // The fence is gone with the failed move: the page opens again.
    assert!(
        sessions
            .join_reflection("eng", "guides/a.md", "ada")
            .await
            .is_ok()
    );
}

/// A join that comes after the move, at the old address, is told where the
/// page went, as an open room was; one at an address nothing moved from is
/// the ordinary miss.
#[tokio::test]
async fn a_join_after_the_move_is_told_where_the_page_went() {
    let (_tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    engine
        .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
        .await
        .unwrap();
    match sessions.join_reflection("eng", "guides/a.md", "ada").await {
        Err(crystalline_service::collab::session::JoinError::Moved(to)) => {
            assert_eq!(to.path, "guides/b.md");
            assert_eq!(to.domain, None);
        }
        Err(other) => panic!("the move, not {other}"),
        Ok(_) => panic!("no room opens at the old path"),
    }
    assert!(matches!(
        sessions.join_reflection("eng", "guides/c.md", "ada").await,
        Err(crystalline_service::collab::session::JoinError::Engine(_))
    ));
}

/// A move whose future is dropped while the room's last save runs (a caller
/// that went away) leaves no room half closed: the save lands, and the room
/// is told `internal`, since the page did not move.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_move_still_saves_and_closes_the_room() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let old = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed before the cancelled move.").await;
    // The page's lock is held, so the room's last save waits on it.
    let lock = engine.file_write_lock(&old);
    let held = lock.clone().lock_owned().await;
    let moving = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    moving.abort();
    let _ = moving.await;
    // The fence stays while the save the cancelled move started runs.
    let refused = match sessions.join_reflection("eng", "guides/a.md", "ada").await {
        Err(refused) => refused.to_string(),
        Ok(_) => panic!("a room opened while the save was still running"),
    };
    assert!(refused.contains("is being moved"), "{refused}");
    drop(held);
    assert_eq!(
        next_closed(&mut joined.rx).await,
        Control::closed("internal")
    );
    assert!(
        std::fs::read_to_string(&old)
            .unwrap()
            .contains("Typed before the cancelled move."),
        "the save landed"
    );
    assert!(
        !tmp.path().join("eng/reflections/guides/b.md").exists(),
        "nothing moved"
    );
}

/// A save that is running when the move comes is waited for: whichever of the
/// two gets the page first, the typed line ends up in the moved page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_move_waits_for_a_save_that_is_running() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let old = tmp.path().join("eng/reflections/guides/a.md");
    let new = tmp.path().join("eng/reflections/guides/b.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed while the page was saving.").await;
    // The page's lock is held, so the room's save waits on it with the
    // room's state taken, and the move's close waits on that state.
    let lock = engine.file_write_lock(&old);
    let held = lock.clone().lock_owned().await;
    let saving = {
        let session = joined.session.clone();
        tokio::spawn(async move {
            session
                .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
                .await
        })
    };
    let moving = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!moving.is_finished(), "the move waits for the page");
    drop(held);
    saving.await.unwrap();
    moving.await.unwrap().unwrap();
    assert!(!old.exists(), "no save brought the old path back");
    let moved = std::fs::read_to_string(&new).unwrap();
    assert!(
        moved.contains("Typed while the page was saving."),
        "{moved}"
    );
}

/// A room in an edit conflict when its page moves cannot save, so its side of
/// the conflict is kept on the server; the page moves with the text on disk.
#[tokio::test]
async fn a_moved_pages_room_in_conflict_keeps_its_text() {
    let (tmp, engine, scratch) = reflection_fixture_keeping().await;
    let new = tmp.path().join("eng/reflections/guides/b.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    let mine = doc
        .get_or_insert_text("content")
        .get_string(&doc.transact())
        .replace("First.", "First, by a person.");
    replace_all(&joined, &doc, &mine).await;
    let p = crystalline_service::params::WriteReflectionParams {
        domain: "eng".into(),
        path: "guides/a.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## One".into()),
        content: Some("First, from the agent.\n".into()),
        ..Default::default()
    };
    let receipt = engine.write_reflection(&p, &room_agent()).await.unwrap();
    assert_eq!(receipt["room"], "conflict", "{receipt}");
    engine
        .move_reflection("eng", "guides/a.md", "guides/b.md", &room_agent())
        .await
        .unwrap();
    let Control::Closed { reason, path, .. } = next_closed(&mut joined.rx).await else {
        unreachable!()
    };
    assert_eq!(
        (reason.as_str(), path.as_deref()),
        ("moved", Some("guides/b.md"))
    );
    let moved = std::fs::read_to_string(&new).unwrap();
    assert!(moved.contains("First, from the agent."), "{moved}");
    let kept_dir = scratch.home().join("state/kept/eng/reflection/guides");
    let kept: Vec<_> = std::fs::read_dir(&kept_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    let held = std::fs::read_to_string(&kept[0]).unwrap();
    assert!(held.contains("First, by a person."), "{held}");
}

/// Publish a display name in awareness, as a browser's provider does.
async fn publish_name(joined: &Joined, doc: &Doc, name: &str) {
    let mut awareness = yrs::sync::Awareness::new(doc.clone());
    awareness.set_local_state_raw(format!("{{\"user\":{{\"name\":\"{name}\"}}}}"));
    let update = awareness.update().unwrap();
    joined
        .session
        .handle_frame(joined.conn, &Message::Awareness(update).encode_v1())
        .await;
}

/// A reflection read says who is in the room over the page (R77), nested as
/// the contract and Fluid read it, and says nothing while no room is open.
#[tokio::test]
async fn a_reflection_read_says_who_is_editing_while_a_room_is_open() {
    let (_tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let caller = crystalline_service::ReflectionCaller::owner();
    let closed = engine
        .read_reflection("eng", "guides/a.md", &caller)
        .await
        .unwrap();
    assert!(closed.get("live").is_none(), "no room, no member: {closed}");
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    publish_name(&joined, &doc, "Ada").await;
    let open = engine
        .read_reflection("eng", "guides/a.md", &caller)
        .await
        .unwrap();
    assert_eq!(
        open["live"]["present"],
        serde_json::json!(["Ada"]),
        "{open}"
    );
    // The page read answers the file, not the room's text.
    assert_eq!(open["checksum"], closed["checksum"]);
}

/// A room save names the signed-in account of each connection that sent an
/// edit: a name a client puts into awareness is not recorded, and a
/// participant who is there and types nothing is not either, even when its
/// provider sends the room's whole state back.
#[tokio::test]
async fn a_reflection_room_names_the_accounts_that_typed_and_nobody_else() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut typist = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let silent = sessions
        .join_reflection("eng", "guides/a.md", "bob")
        .await
        .unwrap();
    let doc = sync_client(&typist).await;
    let _silent_doc = sync_client(&silent).await;
    publish_name(&typist, &doc, "keeper").await;
    publish_name(&silent, &_silent_doc, "bob").await;
    // ada deletes a word as well, so the echo below carries a delete set.
    let text = doc.get_or_insert_text("content");
    let removal = {
        let mut txn = doc.transact_mut();
        let current = text.get_string(&txn);
        let at = current.find("Second.").unwrap() as u32;
        text.remove_range(&mut txn, at, "Second.".len() as u32);
        txn.encode_update_v1()
    };
    typist
        .session
        .handle_frame(typist.conn, &frame_update(removal))
        .await;
    append_line(&typist, &doc, "Typed by ada.").await;
    // bob's provider answers with everything it holds, ada's edit and her
    // deletion included: it applies, and it is no edit of bob's.
    let echo = doc
        .transact()
        .encode_state_as_update_v1(&Default::default());
    silent
        .session
        .handle_frame(
            silent.conn,
            &Message::Sync(SyncMessage::SyncStep2(echo)).encode_v1(),
        )
        .await;
    typist
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    until_saved(&mut typist.rx).await;
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    assert_eq!(
        prov["bbbb2222"].changed_by,
        vec!["ada".to_string()],
        "{saved}"
    );
}

/// The person who typed and left before the room's last save is still the
/// one it names: the account is kept past the disconnect.
#[tokio::test]
async fn the_final_save_names_the_typist_who_already_left() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed before leaving.").await;
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(saved.contains("Typed before leaving."), "{saved}");
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    assert_eq!(
        prov["bbbb2222"].changed_by,
        vec!["ada".to_string()],
        "{saved}"
    );
}

/// A heading typed in a room gets its id only when the room closes, and by
/// then the typist's save has landed and nobody is connected. The room
/// remembers who typed the heading, so the new section names that person
/// and not "someone".
#[tokio::test]
async fn a_heading_typed_in_a_room_names_its_typist_when_it_gets_its_id() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "\n## Three\n\nThird.").await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    until_saved(&mut joined.rx).await;
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    let sections = crystalline_core::reflection::split_sections(&saved);
    let three = sections.iter().find(|s| s.heading == "Three").unwrap();
    let id = three.id.clone().expect("the close gave it an id");
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    assert_eq!(prov[&id].changed_by, vec!["ada".to_string()], "{saved}");
}

/// A heading typed and taken out again leaves no name behind: a later
/// heading of the same name that somebody else typed names only that
/// person when it gets its id.
#[tokio::test]
async fn a_heading_taken_out_again_carries_no_name_onto_a_later_one() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut ada = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let ada_doc = sync_client(&ada).await;
    let original = ada_doc
        .get_or_insert_text("content")
        .get_string(&ada_doc.transact());
    let later = || Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100);
    append_line(&ada, &ada_doc, "\n## Three\n\nAda's.").await;
    ada.session.tick_save(later()).await;
    until_saved(&mut ada.rx).await;
    replace_all(&ada, &ada_doc, &original).await;
    ada.session.tick_save(later()).await;
    until_saved(&mut ada.rx).await;
    let bob = sessions
        .join_reflection("eng", "guides/a.md", "bob")
        .await
        .unwrap();
    let bob_doc = sync_client(&bob).await;
    append_line(&bob, &bob_doc, "\n## Three\n\nBob's.").await;
    bob.session.tick_save(later()).await;
    until_saved(&mut ada.rx).await;
    assert!(!ada.session.remove_conn(ada.conn).await);
    assert!(bob.session.remove_conn(bob.conn).await);
    bob.session.final_save().await;
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    let sections = crystalline_core::reflection::split_sections(&saved);
    let three = sections.iter().find(|s| s.heading == "Three").unwrap();
    let id = three.id.clone().expect("the close gave it an id");
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    assert_eq!(prov[&id].changed_by, vec!["bob".to_string()], "{saved}");
}

/// A room that puts its deleted page back carries who typed a new heading
/// the same way a save does: the close names the typist, not "someone".
#[tokio::test]
async fn a_heading_typed_before_a_restore_names_its_typist_when_it_gets_its_id() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "\n## Three\n\nThird.").await;
    std::fs::remove_file(&path).unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    match next_control(&mut joined.rx).await {
        Control::Conflict { conflict_kind, .. } => assert_eq!(conflict_kind, "deleted"),
        other => panic!("a deletion conflict, not {other:?}"),
    }
    joined
        .session
        .handle_frame(
            joined.conn,
            &control::encode(&Control::Resolve {
                choice: "mine".into(),
            }),
        )
        .await;
    until_saved(&mut joined.rx).await;
    assert!(path.exists(), "the room put the page back");
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    let saved = std::fs::read_to_string(&path).unwrap();
    let sections = crystalline_core::reflection::split_sections(&saved);
    let three = sections.iter().find(|s| s.heading == "Three").unwrap();
    let id = three.id.clone().expect("the close gave it an id");
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    assert_eq!(prov[&id].changed_by, vec!["ada".to_string()], "{saved}");
}

/// Two new sections under the same heading, typed by two people, each name
/// their own typist when the close gives them ids.
#[tokio::test]
async fn two_new_sections_under_one_heading_each_name_their_own_typist() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut ada = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let ada_doc = sync_client(&ada).await;
    let later = || Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100);
    append_line(&ada, &ada_doc, "\n## Notes\n\nA.").await;
    ada.session.tick_save(later()).await;
    until_saved(&mut ada.rx).await;
    let bob = sessions
        .join_reflection("eng", "guides/a.md", "bob")
        .await
        .unwrap();
    let bob_doc = sync_client(&bob).await;
    append_line(&bob, &bob_doc, "\n## Notes\n\nB.").await;
    bob.session.tick_save(later()).await;
    until_saved(&mut ada.rx).await;
    assert!(!ada.session.remove_conn(ada.conn).await);
    assert!(bob.session.remove_conn(bob.conn).await);
    bob.session.final_save().await;
    let saved = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    let notes: Vec<Vec<String>> = crystalline_core::reflection::split_sections(&saved)
        .into_iter()
        .filter(|s| s.heading == "Notes")
        .map(|s| {
            prov[&s.id.expect("the close gave it an id")]
                .changed_by
                .clone()
        })
        .collect();
    assert_eq!(
        notes,
        vec![vec!["ada".to_string()], vec!["bob".to_string()]],
        "{saved}"
    );
}

/// A room open across a switch-off writes nothing into what is engram
/// content now: the save is refused, the file stays as it was and the
/// room keeps its text.
#[tokio::test]
async fn a_room_open_across_a_switch_off_saves_nothing() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed while it was switched off.").await;
    engine
        .set_manifest_policies(
            "eng",
            &[("reflection".to_string(), "off".to_string())],
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    let Control::SaveFailed { detail } = next_control(&mut joined.rx).await else {
        panic!("the save is refused");
    };
    assert!(detail.contains("not on here"), "{detail}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing written");
    let (live, dirty) = joined.session.snapshot().await;
    assert!(live.contains("Typed while it was switched off."), "{live}");
    assert!(dirty, "the room keeps its text");
    // Its close writes nothing either.
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the close writes nothing"
    );
}

/// A switch-off through Crystalline saves the domain's reflection rooms
/// first, while the Reflection is still on, and then closes them, so no
/// typed text is left in a room that can no longer save.
#[tokio::test]
async fn a_switch_off_saves_and_closes_the_reflection_rooms_first() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed just before the switch.").await;
    engine
        .set_manifest_policies(
            "eng",
            &[("reflection".to_string(), "off".to_string())],
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Typed just before the switch.")
    );
    let mut closed = None;
    while closed.is_none() {
        if let Control::Closed { reason, .. } = next_control(&mut joined.rx).await {
            closed = Some(reason);
        }
    }
    assert_eq!(closed.as_deref(), Some("switched_off"));
    assert_eq!(sessions.session_count().await, 0);
}

/// A reflection room over `guides/a.md` in a registry the engine knows, with
/// one line typed and not saved yet: what every MANIFEST writer below finds
/// open when it switches the Reflection off.
async fn a_typed_room_before_a_switch() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
    Arc<CollabSessions>,
    Joined,
) {
    let (tmp, engine, scratch) = reflection_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed just before the switch.").await;
    (tmp, engine, scratch, sessions, joined)
}

/// The MANIFEST of `eng` as it is, and as it reads with the switch off.
fn manifest_and_off(tmp: &tempfile::TempDir) -> (String, String) {
    let now = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    let off = now.replacen("reflection: on\n", "reflection: off\n", 1);
    assert_ne!(now, off);
    (now, off)
}

/// The room's line is in the page and the room was closed as switched off.
async fn assert_saved_then_closed(
    tmp: &tempfile::TempDir,
    sessions: &CollabSessions,
    joined: &mut Joined,
) {
    let page = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(page.contains("Typed just before the switch."), "{page}");
    let mut closed = None;
    while closed.is_none() {
        if let Control::Closed { reason, .. } = next_control(&mut joined.rx).await {
            closed = Some(reason);
        }
    }
    assert_eq!(closed.as_deref(), Some("switched_off"));
    assert_eq!(sessions.session_count().await, 0);
}

/// `edit_engram` on the MANIFEST closes the reflection rooms first.
#[tokio::test]
async fn an_edit_that_switches_off_saves_and_closes_the_rooms_first() {
    let (tmp, engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    engine
        .edit_engram(&crystalline_service::params::EditParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            operation: "find_replace".into(),
            find_text: Some("reflection: on".into()),
            content: Some("reflection: off".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_saved_then_closed(&tmp, &sessions, &mut joined).await;
}

/// **Task 41b.** The MANIFEST is open in a room whose save is in conflict,
/// so an edit that switches the Reflection off does not compose into that
/// room: it lands in the file. The reflection rooms are then saved and
/// closed first, once, as they are for an edit with no room over the
/// MANIFEST, and the receipt says the MANIFEST's room is waiting.
#[tokio::test]
async fn an_edit_that_switches_off_past_a_manifest_room_in_conflict_closes_the_rooms_once() {
    let (tmp, engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    let manifest = tmp.path().join("eng/MANIFEST.md");
    let room = sessions.join("eng", "manifest", None).await.unwrap();
    let doc = sync_client(&room).await;
    append_line(&room, &doc, "- typed in the manifest room").await;
    let on_disk = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, format!("{on_disk}- written on disk\n")).unwrap();
    room.session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert!(
        !sessions
            .live_text("eng", "manifest", None)
            .await
            .unwrap()
            .saving,
        "the manifest's room is in conflict"
    );

    let receipt = engine
        .edit_engram(&crystalline_service::params::EditParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            operation: "find_replace".into(),
            find_text: Some("reflection: on".into()),
            content: Some("reflection: off".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    let after = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        after.contains("reflection: off") && after.contains("- written on disk"),
        "the switch-off is in the file: {after}"
    );
    assert!(!after.contains("typed in the manifest room"), "{after}");

    let page = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(page.contains("Typed just before the switch."), "{page}");
    let mut closed = None;
    while closed.is_none() {
        if let Control::Closed { reason, .. } = next_control(&mut joined.rx).await {
            closed = Some(reason);
        }
    }
    assert_eq!(closed.as_deref(), Some("switched_off"));
    while let Ok(frame) = joined.rx.try_recv() {
        for message in messages_of(&frame.bytes) {
            if let Message::Custom(control::CONTROL_TAG, payload) = message {
                assert!(
                    !matches!(control::decode(&payload), Some(Control::Closed { .. })),
                    "the reflection room is closed once"
                );
            }
        }
    }
    assert!(
        !sessions
            .has_live_doc(
                "eng",
                &crystalline_service::collab::session::RoomDoc::Reflection("guides/a.md".into()),
                None,
            )
            .await,
        "the reflection room is gone"
    );
}

/// An edit with a token of a MANIFEST stored with CRLF line endings closes
/// the reflection rooms first too: the token is the stored bytes' checksum.
#[tokio::test]
async fn an_edit_with_a_token_of_a_crlf_manifest_saves_and_closes_the_rooms_first() {
    let (tmp, engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    let manifest = tmp.path().join("eng/MANIFEST.md");
    let crlf = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace('\n', "\r\n");
    std::fs::write(&manifest, &crlf).unwrap();
    engine.sync(None).await.unwrap();
    engine
        .edit_engram(&crystalline_service::params::EditParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            operation: "find_replace".into(),
            find_text: Some("reflection: on".into()),
            content: Some("reflection: off".into()),
            expected_checksum: Some(crate::support::sha256_hex(crlf.as_bytes())),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::Off
    );
    assert_saved_then_closed(&tmp, &sessions, &mut joined).await;
}

/// **Task 41b review.** The close ahead of a switch-off works the edit out
/// on the LF form of a CRLF MANIFEST, as the write does: a find text that
/// spans a line break matches, so the rooms are saved and closed first.
#[tokio::test]
async fn a_multi_line_edit_of_a_crlf_manifest_closes_the_rooms_first() {
    let (tmp, engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    let manifest = tmp.path().join("eng/MANIFEST.md");
    let lf = std::fs::read_to_string(&manifest).unwrap();
    assert!(lf.contains("reflection: on\n---\n"), "{lf}");
    std::fs::write(&manifest, lf.replace('\n', "\r\n")).unwrap();
    engine.sync(None).await.unwrap();
    engine
        .edit_engram(&crystalline_service::params::EditParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            operation: "find_replace".into(),
            find_text: Some("reflection: on\n---".into()),
            content: Some("reflection: off\n---".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::Off
    );
    assert_saved_then_closed(&tmp, &sessions, &mut joined).await;
}

/// The MANIFEST editor's save closes the reflection rooms first.
#[tokio::test]
async fn a_manifest_editor_save_that_switches_off_saves_and_closes_the_rooms_first() {
    let (tmp, engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    let (now, off) = manifest_and_off(&tmp);
    engine
        .save_manifest(
            "eng",
            &off,
            &crate::support::sha256_hex(now.as_bytes()),
            &crystalline_service::Scope::Unrestricted,
            false,
        )
        .await
        .unwrap();
    assert_saved_then_closed(&tmp, &sessions, &mut joined).await;
}

/// `save_engram` of the MANIFEST closes the reflection rooms first.
#[tokio::test]
async fn an_engram_save_that_switches_off_saves_and_closes_the_rooms_first() {
    let (tmp, engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    let (now, off) = manifest_and_off(&tmp);
    engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "eng".into(),
                identifier: "manifest".into(),
                content: off,
                expected_checksum: crate::support::sha256_hex(now.as_bytes()),
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_saved_then_closed(&tmp, &sessions, &mut joined).await;
}

/// A stale `save_engram` of the MANIFEST is refused, and it closes nobody's
/// room: the rooms close only for a save that will land.
#[tokio::test]
async fn a_stale_engram_save_that_would_switch_off_closes_no_room() {
    let (tmp, engine, _scratch, sessions, joined) = a_typed_room_before_a_switch().await;
    let (_, off) = manifest_and_off(&tmp);
    let refused = engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "eng".into(),
                identifier: "manifest".into(),
                content: off,
                expected_checksum: "0".repeat(64),
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(sessions.session_count().await, 1);
    assert!(!joined.session.is_disposed(), "the room stays open");
}

/// A stale MANIFEST editor save is refused, and it closes nobody's room.
#[tokio::test]
async fn a_stale_manifest_editor_save_that_would_switch_off_closes_no_room() {
    let (tmp, engine, _scratch, sessions, joined) = a_typed_room_before_a_switch().await;
    let (_, off) = manifest_and_off(&tmp);
    let refused = engine
        .save_manifest(
            "eng",
            &off,
            &"0".repeat(64),
            &crystalline_service::Scope::Unrestricted,
            false,
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(sessions.session_count().await, 1);
    assert!(!joined.session.is_disposed(), "the room stays open");
}

/// A restored MANIFEST closes the reflection rooms first.
#[tokio::test]
async fn a_restore_that_switches_off_saves_and_closes_the_rooms_first() {
    let (tmp, engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    let (_, off) = manifest_and_off(&tmp);
    engine
        .restore_engram(
            "eng",
            "MANIFEST.md",
            &off,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_saved_then_closed(&tmp, &sessions, &mut joined).await;
}

/// A move of a file domain's MANIFEST is refused (Jordi, 2026-10-15): the
/// domain needs it. Nothing moves, nothing switches, and no room is closed.
#[tokio::test]
async fn a_move_of_the_manifest_is_refused_and_closes_no_room() {
    let (tmp, engine, _scratch, sessions, joined) = a_typed_room_before_a_switch().await;
    let mut changes = engine.changes().subscribe();
    let err = engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "manifest".into(),
                domain: "eng".into(),
                destination: "old-manifest.md".into(),
                destination_domain: None,
                permalink: None,
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !tmp.path().join("eng/old-manifest.md").exists(),
        "nothing moved"
    );
    assert_eq!(err, crystalline_service::EngineError::Invalid(
        "MANIFEST.md is required for a domain. Edit it instead, or remove the domain to drop it.".into()
    ).to_string());
    assert!(
        tmp.path().join("eng/MANIFEST.md").exists(),
        "the MANIFEST stays"
    );
    assert_eq!(
        engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::On,
        "nothing switched"
    );
    assert_eq!(sessions.session_count().await, 1, "no room is closed");
    assert!(!joined.session.is_disposed(), "the room stays open");
    while let Ok(envelope) = changes.try_recv() {
        if let crystalline_service::changes::Change::Reflection(change) = envelope.change {
            panic!("nothing is announced: {change:?}");
        }
    }
}

/// A delete of a file domain's MANIFEST is refused (Jordi, 2026-10-15): the
/// domain needs it. Nothing is deleted, nothing switches, and no room is
/// closed.
#[tokio::test]
async fn a_delete_of_the_manifest_is_refused_and_closes_no_room() {
    let (tmp, engine, _scratch, sessions, joined) = a_typed_room_before_a_switch().await;
    let mut changes = engine.changes().subscribe();
    let err = engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "manifest".into(),
            domain: "eng".into(),
            expected_checksum: None,
        })
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err, crystalline_service::EngineError::Invalid(
        "MANIFEST.md is required for a domain. Edit it instead, or remove the domain to drop it.".into()
    ).to_string());
    assert!(
        tmp.path().join("eng/MANIFEST.md").exists(),
        "the MANIFEST stays"
    );
    assert_eq!(
        engine.reflection_switch("eng"),
        crystalline_core::ReflectionSwitch::On,
        "nothing switched"
    );
    assert_eq!(sessions.session_count().await, 1, "no room is closed");
    assert!(!joined.session.is_disposed(), "the room stays open");
    while let Ok(envelope) = changes.try_recv() {
        if let crystalline_service::changes::Change::Reflection(change) = envelope.change {
            panic!("nothing is announced: {change:?}");
        }
    }
}

/// A MANIFEST room's own switch-off save, with joins to that room racing
/// it, never deadlocks: the save closes the reflection rooms with no room
/// guard held, so a join that holds the registry while it waits for the
/// room's guard is never waited on in turn. Several rounds, so the race is
/// actually run; a hang fails the bounded wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_racing_a_manifest_rooms_switch_off_save_does_not_deadlock() {
    for _ in 0..5 {
        let (tmp, _engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
        let (_, off) = manifest_and_off(&tmp);
        let manifest_room = sessions.join("eng", "manifest", None).await.unwrap();
        let doc = sync_client(&manifest_room).await;
        replace_all(&manifest_room, &doc, &off).await;
        let room = manifest_room.session.clone();
        let race = async {
            let save = tokio::spawn(async move {
                room.tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
                    .await;
            });
            let joins: Vec<_> = (0..8)
                .map(|_| {
                    let sessions = sessions.clone();
                    tokio::spawn(async move { sessions.join("eng", "manifest", None).await })
                })
                .collect();
            save.await.unwrap();
            for join in joins {
                let _ = join.await.unwrap();
            }
        };
        tokio::time::timeout(Duration::from_secs(10), race)
            .await
            .expect("the save and the joins all finish");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
            off
        );
        let mut closed = None;
        while closed.is_none() {
            if let Control::Closed { reason, .. } = next_control(&mut joined.rx).await {
                closed = Some(reason);
            }
        }
        assert_eq!(closed.as_deref(), Some("switched_off"));
    }
}

/// A room over the MANIFEST whose own save switches the Reflection off
/// closes the reflection rooms first, from inside its save, and does not
/// hang doing it.
#[tokio::test]
async fn a_manifest_room_save_that_switches_off_saves_and_closes_the_rooms_first() {
    let (tmp, _engine, _scratch, sessions, mut joined) = a_typed_room_before_a_switch().await;
    let (_, off) = manifest_and_off(&tmp);
    let manifest_room = sessions.join("eng", "manifest", None).await.unwrap();
    let doc = sync_client(&manifest_room).await;
    replace_all(&manifest_room, &doc, &off).await;
    tokio::time::timeout(
        Duration::from_secs(10),
        manifest_room
            .session
            .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100)),
    )
    .await
    .expect("the MANIFEST room's save does not hang");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
        off
    );
    let page = std::fs::read_to_string(tmp.path().join("eng/reflections/guides/a.md")).unwrap();
    assert!(page.contains("Typed just before the switch."), "{page}");
    let mut closed = None;
    while closed.is_none() {
        if let Control::Closed { reason, .. } = next_control(&mut joined.rx).await {
            closed = Some(reason);
        }
    }
    assert_eq!(closed.as_deref(), Some("switched_off"));
    assert_eq!(
        sessions.session_count().await,
        1,
        "only the MANIFEST room is left"
    );
}

/// A switch-off that is refused closes nobody's room: the rooms close only
/// once every check that can still refuse the edit has passed.
#[tokio::test]
async fn a_refused_switch_off_closes_no_room() {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user(
        "ed",
        "ed",
        None,
        crystalline_service::rest::Role::Editor,
        "pw12345678",
    )
    .await
    .unwrap();
    auth.set_write_list("ed", &["agents-*".to_string()])
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(crystalline_service::DomainAccess::new(auth)));
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed while ed tried the switch.").await;
    let ed = crystalline_service::Scope::User {
        account: "ed".into(),
        admin: false,
    };
    let refused = engine
        .set_manifest_policies("eng", &[("reflection".to_string(), "off".to_string())], &ed)
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(sessions.session_count().await, 1);
    assert!(!joined.session.is_disposed(), "the room stays open");
}

/// A room whose last save is refused (the switch went off outside
/// Crystalline, so nothing closed it first) keeps its text on the server:
/// one file under `<state>/kept/` holds it, and the page is not touched.
#[tokio::test]
async fn a_refused_close_keeps_the_rooms_text_on_the_server() {
    let (tmp, engine, scratch) = reflection_fixture_keeping().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "Typed while it was switched off.").await;
    // A plain editor switches it off: no Crystalline writer runs.
    let manifest = tmp.path().join("eng/MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap().replacen(
        "reflection: on\n",
        "reflection: off\n",
        1,
    );
    std::fs::write(&manifest, text).unwrap();
    let before = std::fs::read(&path).unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    let Control::SaveFailed { detail } = next_control(&mut joined.rx).await else {
        panic!("the save is refused");
    };
    assert!(
        detail.contains("not on here") && detail.contains("the server keeps one too"),
        "{detail}"
    );
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the close writes nothing"
    );
    let kept_dir = scratch.home().join("state/kept/eng/reflection/guides");
    let kept: Vec<_> = std::fs::read_dir(&kept_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    let name = kept[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.starts_with("a.md.") && name.ends_with("Z.md"),
        "{name}"
    );
    let held = std::fs::read_to_string(&kept[0]).unwrap();
    assert!(held.contains("Typed while it was switched off."), "{held}");
    // What the room kept is what the kept-text listing finds, under the
    // page's own path and with its text.
    let listed = engine
        .kept_texts("eng", &crystalline_service::Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].path, "guides/a.md");
    assert_eq!(listed[0].kind, crystalline_core::kept::KeptKind::Reflection);
    let (_, text) = engine
        .kept_text(
            "eng",
            &listed[0].id,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(text, held);
}

/// A room nobody typed in loses nothing when its last save is refused (a
/// reflection room's last pass runs anyway, for its ids), so it keeps no
/// file either.
#[tokio::test]
async fn an_untouched_room_refused_on_close_keeps_nothing() {
    let (tmp, engine, scratch) = reflection_fixture_keeping().await;
    let sessions = CollabSessions::new(engine.clone());
    let joined = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let _doc = sync_client(&joined).await;
    let manifest = tmp.path().join("eng/MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap().replacen(
        "reflection: on\n",
        "reflection: off\n",
        1,
    );
    std::fs::write(&manifest, text).unwrap();
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    assert!(
        !scratch.home().join("state/kept").exists(),
        "nothing to keep"
    );
}

/// A change typed and taken back again before the save names nobody: the
/// next save, of somebody else's edit, records only that person.
#[tokio::test]
async fn an_undone_edit_names_nobody_on_the_next_save() {
    undone_edit_names_nobody(false).await;
}

/// [`an_undone_edit_names_nobody_on_the_next_save`] with the Save button
/// pressed over the undone edit: the flush reaches the save itself, which
/// finds nothing to write.
#[tokio::test]
async fn an_undone_edit_flushed_names_nobody_on_the_next_save() {
    undone_edit_names_nobody(true).await;
}

async fn undone_edit_names_nobody(flush: bool) {
    let (tmp, engine, _scratch) = reflection_fixture().await;
    let path = tmp.path().join("eng/reflections/guides/a.md");
    let sessions = CollabSessions::new(engine.clone());
    let ada = sessions
        .join_reflection("eng", "guides/a.md", "ada")
        .await
        .unwrap();
    let doc_a = sync_client(&ada).await;
    let original = {
        let text = doc_a.get_or_insert_text("content");
        let txn = doc_a.transact();
        text.get_string(&txn)
    };
    append_line(&ada, &doc_a, "Typed and taken back.").await;
    replace_all(&ada, &doc_a, &original).await;
    if flush {
        ada.session
            .handle_frame(ada.conn, &control::encode(&Control::Flush))
            .await;
    }
    let before = std::fs::read(&path).unwrap();
    ada.session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing written");
    let mut bob = sessions
        .join_reflection("eng", "guides/a.md", "bob")
        .await
        .unwrap();
    let doc_b = sync_client(&bob).await;
    append_line(&bob, &doc_b, "Typed by bob.").await;
    bob.session
        .tick_save(Instant::now() + Duration::from_millis(SAVE_DEBOUNCE_MS + 100))
        .await;
    until_saved(&mut bob.rx).await;
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("Typed by bob."), "{saved}");
    let prov = crystalline_core::reflection::provenance::read_provenance(&saved);
    assert_eq!(
        prov["bbbb2222"].changed_by,
        vec!["bob".to_string()],
        "{saved}"
    );
}
