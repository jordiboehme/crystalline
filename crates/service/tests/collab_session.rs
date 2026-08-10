//! The collab session core, driven WITHOUT a socket: raw yrs docs play the
//! clients and exchange encoded protocol frames through handle_frame, so
//! every assertion is about session semantics rather than transport.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::collab::control::{self, Control};
use crystalline_service::collab::session::{CollabSessions, Frame, MAX_PARTICIPANTS};
use tokio::sync::Mutex;
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, GetString, Options, ReadTxn, Text, Transact, Update};

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// An engram whose body is full of characters that are one UTF-16 unit, two
/// UTF-16 units and several UTF-8 bytes each: the shapes a byte-offset
/// document would misplace.
const WIDE: &str = "---\ntype: engram\ntitle: Wide\npermalink: wide\ntags:\n  - eng\nstatus: stable\n---\n\na 𝄞 clef and an emoji 🎉 in one line\nünnötige Umlaute\n";

/// A file domain `eng` holding MANIFEST, alpha, a CRLF engram and a
/// mixed-endings one, synced into an in-memory store. Mirrors
/// `engine_writes.rs::engine_fixture`; integration test crates share no
/// helpers, so it is copied rather than imported.
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
    std::fs::write(
        dir.join("mixed.md"),
        "---\r\ntitle: Mixed\r\npermalink: mixed\r\ntags:\r\n  - eng\r\nstatus: stable\r\ntype: engram\r\n---\r\n\r\na CRLF file\nwith a lone LF\r\n",
    )
    .unwrap();
    std::fs::write(dir.join("wide.md"), WIDE).unwrap();
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

/// A JS-shaped client: a plain yjs-compatible doc mirroring the session.
fn client_doc() -> Doc {
    Doc::with_options(Options {
        offset_kind: yrs::OffsetKind::Utf16,
        ..Options::default()
    })
}

/// Split a greeting/broadcast buffer into its protocol messages.
fn messages_of(bytes: &[u8]) -> Vec<Message> {
    let mut decoder = yrs::updates::decoder::DecoderV1::from(bytes);
    yrs::sync::MessageReader::new(&mut decoder)
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Sync a fresh client doc against the session and return it, the way a
/// provider does: SyncStep1 out, SyncStep2 back, applied.
async fn synced_client(
    session: &crystalline_service::collab::session::CollabSession,
    conn: u64,
) -> Doc {
    let doc = client_doc();
    let sv = doc.transact().state_vector();
    let replies = session
        .handle_frame(conn, &Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1())
        .await;
    let Message::Sync(SyncMessage::SyncStep2(update)) = &messages_of(&replies[0])[0] else {
        panic!("step1 is answered with step2");
    };
    doc.transact_mut()
        .apply_update(Update::decode_v1(update).unwrap())
        .unwrap();
    doc
}

#[tokio::test]
async fn the_first_join_loads_the_file_and_greets_with_hello_and_step1() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "crlf").await.unwrap();

    let messages = messages_of(&joined.greeting);
    // Message 1: the hello control carrying the recorded separator.
    let Message::Custom(tag, payload) = &messages[0] else {
        panic!("greeting opens with the hello control");
    };
    assert_eq!(*tag, control::CONTROL_TAG);
    let Some(Control::Hello {
        separator,
        permalink,
        save_state,
        epoch,
        ..
    }) = control::decode(payload)
    else {
        panic!("hello parses");
    };
    assert_eq!(separator, "\r\n");
    assert_eq!(permalink, "crlf");
    assert_eq!(save_state, "ok");
    assert_eq!(epoch, joined.session.epoch());
    // Message 2: SyncStep1 with the server's state vector.
    assert!(matches!(
        messages[1],
        Message::Sync(SyncMessage::SyncStep1(_))
    ));
}

#[tokio::test]
async fn a_client_syncs_and_reads_the_lf_session_text() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "crlf").await.unwrap();

    // The client answers the greeting's SyncStep1 with its own SyncStep1 and
    // receives SyncStep2 carrying everything it is missing.
    let doc = synced_client(&joined.session, joined.conn).await;
    let text = doc.get_or_insert_text("content");
    let synced = text.get_string(&doc.transact());
    assert!(!synced.contains("\r\n"), "the shared text is LF space");
    assert!(synced.contains("windows body"));

    // And the session hands it back in file space, byte for byte.
    let (file_now, dirty) = joined.session.snapshot().await;
    assert!(file_now.contains("windows body\r\n"), "{file_now:?}");
    assert!(!dirty, "an untouched session is not dirty");
}

#[tokio::test]
async fn a_non_ascii_engram_round_trips_byte_for_byte() {
    // The session doc counts UTF-16 units, the way the JS client does; a
    // document full of multi-byte characters must still come back out of the
    // session exactly as it went in, or the first save would rewrite it.
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "wide").await.unwrap();

    let doc = synced_client(&joined.session, joined.conn).await;
    let text = doc.get_or_insert_text("content");
    let synced = text.get_string(&doc.transact());
    assert!(synced.contains("a 𝄞 clef and an emoji 🎉 in one line"));

    let (file_now, dirty) = joined.session.snapshot().await;
    assert_eq!(file_now, WIDE, "byte fidelity for an untouched session");
    assert!(!dirty);
}

#[tokio::test]
async fn an_update_from_one_conn_fans_out_tagged_with_its_origin() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let alice = sessions.join("eng", "alpha").await.unwrap();
    let mut bob = sessions.join("eng", "alpha").await.unwrap();

    // Alice edits: a client-side doc synced first, then an insert, sent as an
    // Update frame the way the provider would send it.
    let doc = synced_client(&alice.session, alice.conn).await;
    let text = doc.get_or_insert_text("content");
    let edit = {
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, "hi ");
        txn.encode_update_v1()
    };
    alice
        .session
        .handle_frame(
            alice.conn,
            &Message::Sync(SyncMessage::Update(edit)).encode_v1(),
        )
        .await;

    let frame: Frame = bob.rx.recv().await.unwrap();
    assert_eq!(
        frame.from,
        Some(alice.conn),
        "tagged so the sender can be skipped"
    );
    let Message::Sync(SyncMessage::Update(_)) = &messages_of(&frame.bytes)[0] else {
        panic!("the fan-out frame is an update");
    };
    let (file_now, dirty) = alice.session.snapshot().await;
    assert!(dirty, "an applied update marks the session dirty");
    assert!(
        file_now.starts_with("hi "),
        "the session text took the edit"
    );
}

#[tokio::test]
async fn awareness_states_fan_out_and_null_on_disconnect() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let alice = sessions.join("eng", "alpha").await.unwrap();
    let mut bob = sessions.join("eng", "alpha").await.unwrap();

    // Alice announces presence: client 7 with a user state, hand-framed the
    // way y-protocols encodes it (same reference shape as collab_wire.rs).
    let update = yrs::sync::AwarenessUpdate {
        clients: std::collections::HashMap::from([(
            ClientID::new(7),
            yrs::sync::awareness::AwarenessUpdateEntry {
                clock: 1,
                json: r#"{"user":{"name":"Alice"}}"#.into(),
            },
        )]),
    };
    alice
        .session
        .handle_frame(alice.conn, &Message::Awareness(update).encode_v1())
        .await;
    let seen: Frame = bob.rx.recv().await.unwrap();
    assert_eq!(seen.from, Some(alice.conn));
    let Message::Awareness(relayed) = &messages_of(&seen.bytes)[0] else {
        panic!("awareness relays");
    };
    assert!(relayed.clients.contains_key(&ClientID::new(7)));

    // Alice's socket dies: her client 7 is nulled for everyone else.
    let last = alice.session.remove_conn(alice.conn).await;
    assert!(!last, "bob is still here");
    let removal: Frame = bob.rx.recv().await.unwrap();
    let Message::Awareness(nulled) = &messages_of(&removal.bytes)[0] else {
        panic!("removal is an awareness message");
    };
    let entry = nulled.clients.get(&ClientID::new(7)).unwrap();
    assert_eq!(&*entry.json, "null");
    assert_eq!(entry.clock, 2, "the null carries a bumped clock so it wins");
}

#[tokio::test]
async fn a_late_joiner_is_greeted_with_the_presence_already_in_the_room() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let alice = sessions.join("eng", "alpha").await.unwrap();
    let update = yrs::sync::AwarenessUpdate {
        clients: std::collections::HashMap::from([(
            ClientID::new(7),
            yrs::sync::awareness::AwarenessUpdateEntry {
                clock: 1,
                json: r#"{"user":{"name":"Alice"}}"#.into(),
            },
        )]),
    };
    alice
        .session
        .handle_frame(alice.conn, &Message::Awareness(update).encode_v1())
        .await;

    let bob = sessions.join("eng", "alpha").await.unwrap();
    let messages = messages_of(&bob.greeting);
    let Some(Message::Awareness(full)) = messages.get(2) else {
        panic!("the greeting closes with the awareness picture: {messages:?}");
    };
    assert!(full.clients.contains_key(&ClientID::new(7)));
}

#[tokio::test]
async fn an_awareness_query_is_answered_directly_not_broadcast() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let alice = sessions.join("eng", "alpha").await.unwrap();
    let update = yrs::sync::AwarenessUpdate {
        clients: std::collections::HashMap::from([(
            ClientID::new(9),
            yrs::sync::awareness::AwarenessUpdateEntry {
                clock: 1,
                json: r#"{"user":{"name":"Alice"}}"#.into(),
            },
        )]),
    };
    alice
        .session
        .handle_frame(alice.conn, &Message::Awareness(update).encode_v1())
        .await;

    let replies = alice
        .session
        .handle_frame(alice.conn, &Message::AwarenessQuery.encode_v1())
        .await;
    let Message::Awareness(full) = &messages_of(&replies[0])[0] else {
        panic!("a query is answered with the full picture");
    };
    assert!(full.clients.contains_key(&ClientID::new(9)));
}

#[tokio::test]
async fn a_malformed_frame_is_dropped_without_killing_the_session() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let joined = sessions.join("eng", "alpha").await.unwrap();

    let replies = joined
        .session
        .handle_frame(joined.conn, &[0xff, 0xff, 0xff])
        .await;
    assert!(replies.is_empty(), "garbage earns no reply");
    // The session still serves: a sync round trip works after the garbage.
    let doc = synced_client(&joined.session, joined.conn).await;
    let text = doc.get_or_insert_text("content");
    assert!(
        text.get_string(&doc.transact())
            .contains("A rule about alpha.")
    );
}

#[tokio::test]
async fn mixed_endings_and_full_rooms_are_refused() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let refused = sessions.join("eng", "mixed").await.unwrap_err();
    assert!(matches!(
        refused,
        crystalline_service::collab::session::JoinError::MixedEndings
    ));

    let mut joined = Vec::new();
    for _ in 0..MAX_PARTICIPANTS {
        joined.push(sessions.join("eng", "alpha").await.unwrap());
    }
    let over = sessions.join("eng", "alpha").await.unwrap_err();
    assert!(matches!(
        over,
        crystalline_service::collab::session::JoinError::SessionFull
    ));
}

#[tokio::test]
async fn an_unknown_engram_is_an_engine_error_not_a_session() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let refused = sessions.join("eng", "ghost").await.unwrap_err();
    assert!(matches!(
        refused,
        crystalline_service::collab::session::JoinError::Engine(_)
    ));
    assert_eq!(
        sessions.session_count().await,
        0,
        "a failed open leaves no registry entry behind"
    );
}

#[tokio::test]
async fn the_last_leave_disposes_the_session() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    let joined = sessions.join("eng", "alpha").await.unwrap();
    assert_eq!(sessions.session_count().await, 1);
    let last = joined.session.remove_conn(joined.conn).await;
    assert!(last);
    sessions.dispose_if_empty("eng", "alpha").await;
    assert_eq!(sessions.session_count().await, 0);
    // A fresh join is a fresh epoch: the restart-detection signal.
    let again = sessions.join("eng", "alpha").await.unwrap();
    assert_ne!(again.session.epoch(), joined.session.epoch());
}

#[tokio::test]
async fn a_populated_session_survives_dispose_if_empty() {
    let (_tmp, engine) = engine_fixture().await;
    let sessions = CollabSessions::new(engine);
    let alice = sessions.join("eng", "alpha").await.unwrap();
    let bob = sessions.join("eng", "alpha").await.unwrap();
    assert!(
        Arc::ptr_eq(&alice.session, &bob.session),
        "the second join shares the document"
    );

    let last = alice.session.remove_conn(alice.conn).await;
    assert!(!last, "bob is still connected");
    sessions.dispose_if_empty("eng", "alpha").await;
    assert_eq!(sessions.session_count().await, 1, "bob keeps it alive");
}
