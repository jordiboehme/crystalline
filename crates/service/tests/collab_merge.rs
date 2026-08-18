//! External changes: an agent, git or the CLI writes the file while a session
//! holds it open. A clean three-way merge flows into the live room; a genuine
//! collision suspends saving until the room resolves it; a deleted file is its
//! own conflict kind. Driven without sockets and without sleeping, like
//! collab_saves.rs.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::collab::control::{self, Control};
use crystalline_service::collab::session::{CollabSessions, Frame, IDLE_CHECK_MS, Joined};
use tokio::sync::Mutex;
use yrs::sync::{Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Options, ReadTxn, Text, Transact, Update};

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// A file domain `eng` holding MANIFEST and alpha, synced into an in-memory
/// store. Mirrors `collab_saves.rs::engine_fixture`; integration test crates
/// share no helpers, so it is copied rather than imported.
async fn engine_fixture() -> (tempfile::TempDir, Arc<Engine>, support::ScratchStateDir) {
    let scratch = support::ScratchStateDir::acquire();
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

/// A JS-shaped client doc, synced against the session.
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

/// Append `line` plus a newline at the DOCUMENT END and send it.
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

/// [`next_control`], applying every document update that arrives ahead of it
/// to `doc`. What a real client does: a server-side merge reaches the editor
/// before the author's next keystroke, so the keystroke is composed against
/// the merged text rather than concurrently with it.
async fn next_control_syncing(
    rx: &mut tokio::sync::broadcast::Receiver<Frame>,
    doc: &Doc,
) -> Control {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a control frame arrives in time")
            .expect("the broadcast channel stays open");
        let mut decoder = DecoderV1::from(frame.bytes.as_ref());
        for message in MessageReader::new(&mut decoder).flatten() {
            match message {
                Message::Custom(control::CONTROL_TAG, payload) => {
                    if let Some(found) = control::decode(&payload) {
                        return found;
                    }
                }
                Message::Sync(SyncMessage::Update(update)) => {
                    if let Ok(decoded) = Update::decode_v1(&update) {
                        let _ = doc.transact_mut().apply_update(decoded);
                    }
                }
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn a_clean_external_edit_merges_into_the_room_and_the_file() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    // Mine: refine the heading. Theirs: an external append at the end,
    // written behind the session's back. The two hunks have the untouched
    // body line between them, so the three-way merge is clean (an edit that
    // TOUCHED the appended-to line would collide instead; see merge.rs).
    replace_all(&joined, &doc, &ALPHA.replace("# Alpha", "# Alpha, refined")).await;
    let path = tmp.path().join("eng/alpha.md");
    let external = std::fs::read_to_string(&path).unwrap() + "\ntheirs: appended line\n";
    std::fs::write(&path, &external).unwrap();

    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;

    // The room hears about the merge, then the save that landed it.
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Merged
    ));
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.contains("# Alpha, refined"), "my edit survived");
    assert!(
        on_disk.contains("theirs: appended line"),
        "their edit survived"
    );
    // And the live doc converged onto the same text.
    let (file_now, dirty) = joined.session.snapshot().await;
    assert_eq!(file_now, on_disk);
    assert!(!dirty);
}

#[tokio::test]
async fn an_idle_session_pulls_external_edits_in_without_writing() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let _doc = sync_client(&joined).await;
    let path = tmp.path().join("eng/alpha.md");
    let external = std::fs::read_to_string(&path)
        .unwrap()
        .replace("A rule", "An external rule");
    std::fs::write(&path, &external).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(IDLE_CHECK_MS + 100))
        .await;

    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Merged
    ));
    let (file_now, dirty) = joined.session.snapshot().await;
    assert_eq!(file_now, external, "the room converged onto theirs");
    assert!(!dirty);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before,
        "nothing was written back: the file already held this text"
    );
}

#[tokio::test]
async fn colliding_edits_suspend_saving_until_the_room_resolves_mine() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    let path = tmp.path().join("eng/alpha.md");
    // Both sides rewrite the same body line.
    replace_all(&joined, &doc, &ALPHA.replace("A rule about alpha.", "MINE")).await;
    std::fs::write(&path, ALPHA.replace("A rule about alpha.", "THEIRS")).unwrap();

    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let Control::Conflict {
        conflict_kind,
        theirs,
        ..
    } = next_control(&mut joined.rx).await
    else {
        panic!("the room is told");
    };
    assert_eq!(conflict_kind, "edit");
    assert!(theirs.unwrap().contains("THEIRS"));
    // Saving is suspended: further ticks write nothing.
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    assert!(std::fs::read_to_string(&path).unwrap().contains("THEIRS"));

    // The room picks mine: the session text lands over theirs.
    joined
        .session
        .handle_frame(
            joined.conn,
            &control::encode(&Control::Resolve {
                choice: "mine".into(),
            }),
        )
        .await;
    joined.session.tick_save(Instant::now()).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    assert!(std::fs::read_to_string(&path).unwrap().contains("MINE"));
}

#[tokio::test]
async fn resolving_theirs_replaces_the_live_text_and_touches_nothing() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    let path = tmp.path().join("eng/alpha.md");
    let theirs = ALPHA.replace("A rule about alpha.", "THEIRS");
    replace_all(&joined, &doc, &ALPHA.replace("A rule about alpha.", "MINE")).await;
    std::fs::write(&path, &theirs).unwrap();
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Conflict { .. }
    ));
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    joined
        .session
        .handle_frame(
            joined.conn,
            &control::encode(&Control::Resolve {
                choice: "theirs".into(),
            }),
        )
        .await;
    // The room hears the convergence; the live doc now holds theirs, and this
    // client adopts the same edit before it types again.
    assert!(matches!(
        next_control_syncing(&mut joined.rx, &doc).await,
        Control::Merged
    ));
    let (file_now, dirty) = joined.session.snapshot().await;
    assert_eq!(file_now, theirs);
    assert!(!dirty);
    // Nothing was written: the file already held this text, and saving is no
    // longer suspended.
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
    // A fresh edit afterward saves normally over the adopted checksum.
    append_line(&joined, &doc, "after the storm").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .ends_with("after the storm\n")
    );
}

#[tokio::test]
async fn an_external_delete_is_its_own_conflict_and_mine_restores_the_file() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    replace_all(&joined, &doc, &ALPHA.replace("A rule", "An unsaved rule")).await;
    std::fs::remove_file(tmp.path().join("eng/alpha.md")).unwrap();

    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let Control::Conflict {
        conflict_kind,
        theirs,
        ..
    } = next_control(&mut joined.rx).await
    else {
        panic!("deleted conflict");
    };
    assert_eq!(conflict_kind, "deleted");
    assert!(theirs.is_none());

    joined
        .session
        .handle_frame(
            joined.conn,
            &control::encode(&Control::Resolve {
                choice: "mine".into(),
            }),
        )
        .await;
    joined.session.tick_save(Instant::now()).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    let restored = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        restored.contains("An unsaved rule"),
        "the session text came back"
    );
}

#[tokio::test]
async fn a_deleted_conflict_never_restores_over_a_file_that_came_back() {
    // "Deleted" means the engram is not there to SAVE, which an external
    // rename produces while the file itself sits on disk holding the other
    // author's work. A restore has no CAS to stop it, so the resolve reads the
    // path first: their bytes survive and the room is asked again.
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    replace_all(&joined, &doc, &ALPHA.replace("A rule", "An unsaved rule")).await;

    // The external author renames the engram whole and rewrites its body; the
    // reindex takes 'alpha' out of the index while alpha.md stays on disk.
    let path = tmp.path().join("eng/alpha.md");
    let external = ALPHA
        .replace("permalink: alpha", "permalink: beta")
        .replace("title: Alpha", "title: Beta")
        .replace("A rule about alpha.", "THEIR OWN WORK");
    std::fs::write(&path, &external).unwrap();
    engine.sync(None).await.unwrap();

    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let Control::Conflict { conflict_kind, .. } = next_control(&mut joined.rx).await else {
        panic!("the engram is gone from the index");
    };
    assert_eq!(conflict_kind, "deleted");

    joined
        .session
        .handle_frame(
            joined.conn,
            &control::encode(&Control::Resolve {
                choice: "mine".into(),
            }),
        )
        .await;
    let Control::Conflict {
        conflict_kind,
        theirs,
        ..
    } = next_control(&mut joined.rx).await
    else {
        panic!("the room is asked again, with their text in view");
    };
    assert_eq!(conflict_kind, "edit");
    assert!(theirs.unwrap().contains("THEIR OWN WORK"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        external,
        "their bytes were never overwritten"
    );
    // And saving stays suspended rather than landing behind their back.
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
}

#[tokio::test]
async fn resolving_mine_adopts_their_text_as_the_base_so_the_choice_lands() {
    // A theirs the session cannot merge at all: mixed line endings. Nobody in
    // the room typed, so "mine" is the base text - and a resolve that adopted
    // only their CHECKSUM would leave the save comparing my text against the
    // stale base, finding nothing to write, and telling the room its choice
    // landed when the file still held theirs.
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let _doc = sync_client(&joined).await;
    let path = tmp.path().join("eng/alpha.md");
    let external = ALPHA
        .replace("A rule about alpha.", "THEIRS")
        .replacen("---\n", "---\r\n", 1);
    std::fs::write(&path, &external).unwrap();

    // The idle probe finds it, and the merge refuses to rewrite the endings.
    joined
        .session
        .tick_save(Instant::now() + Duration::from_millis(IDLE_CHECK_MS + 100))
        .await;
    let Control::Conflict { conflict_kind, .. } = next_control(&mut joined.rx).await else {
        panic!("a mixed-endings theirs is the room's decision");
    };
    assert_eq!(conflict_kind, "edit");

    joined
        .session
        .handle_frame(
            joined.conn,
            &control::encode(&Control::Resolve {
                choice: "mine".into(),
            }),
        )
        .await;
    joined.session.tick_save(Instant::now()).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Saved { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        ALPHA,
        "the room's own text landed over theirs"
    );
}

#[tokio::test]
async fn accepting_an_external_delete_closes_the_session() {
    let (tmp, engine, _scratch) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let mut joined = sessions.join("eng", "alpha").await.unwrap();
    let doc = sync_client(&joined).await;
    replace_all(&joined, &doc, &ALPHA.replace("A rule", "A doomed rule")).await;
    let path = tmp.path().join("eng/alpha.md");
    std::fs::remove_file(&path).unwrap();
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    assert!(matches!(
        next_control(&mut joined.rx).await,
        Control::Conflict { .. }
    ));

    joined
        .session
        .handle_frame(
            joined.conn,
            &control::encode(&Control::Resolve {
                choice: "theirs".into(),
            }),
        )
        .await;
    let Control::Closed { reason } = next_control(&mut joined.rx).await else {
        panic!("the room is told the session is over");
    };
    assert_eq!(reason, "deleted");
    // The deletion stands: no final save resurrects the file.
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    sessions.dispose_if_empty(&joined.session).await;
    assert!(
        !path.exists(),
        "accepting the deletion never writes the file back"
    );
    assert_eq!(sessions.session_count().await, 0);
}
