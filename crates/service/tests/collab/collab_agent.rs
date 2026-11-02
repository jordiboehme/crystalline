//! The live seam: what an agent's write meets when a co-editing room over the
//! same document is open, and what an agent's read is answered with while
//! somebody is still typing.
//!
//! **The rule these assert is one sentence: while a room is open, the live
//! document IS the engram.** An agent that edited the file or the row under an
//! open room would write into a copy the room is about to overwrite, and a
//! reader that read the file would be reading what the room has already moved
//! past. So the edit composes into the document and the read is answered from
//! it, and the room's own saver is what makes either durable.
//!
//! Driven WITHOUT a socket and WITHOUT sleeping, exactly as `collab_saves.rs`
//! is: the saver pass takes `now`, so a debounce window is a value rather than
//! a wait. Integration test crates share no helpers, so the fixtures here are
//! copied from that file rather than imported.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::support::mcp_session::{ServedSession, chooses_lifecycle};
use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::{Store, TursoStore};
use crystalline_service::Engine;
use crystalline_service::collab::session::{
    AgentPeer, CollabSessions, Frame, Joined, MAX_PARTICIPANTS,
};
use crystalline_service::mcp::McpServer;
use crystalline_service::params::{EditParams, ReadParams, WriteParams};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, broadcast};
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{Awareness, AwarenessUpdate, Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, GetString, Options, ReadTxn, Text, Transact, Update};

/// Run a body against Turso (always) and Postgres (when configured), each with a
/// fresh, isolated store handed to the engine as a trait object.
macro_rules! both_backends {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
                $body(store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(scratch) = crate::support::scratch_schema("ctv") {
                    let pg = crystalline_index::PostgresStore::open_in_schema(
                        scratch.url(),
                        scratch.name(),
                    )
                    .await
                    .expect("open the postgres test schema");
                    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(pg));
                    $body(store).await;
                }
            }
        }
    };
}

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

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

/// One file domain, `eng`, holding MANIFEST and alpha, synced into an
/// in-memory store. `review` puts it in review mode, which is where an
/// agent's write meets its own overlay document rather than the folder's.
async fn engine_fixture(
    review: bool,
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
    let mut entry = DomainEntry::file(dir);
    if review {
        entry.review = Some(crystalline_core::config::ReviewMode::Overlay);
    }
    cfg.domains.insert("eng".to_string(), entry);
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

/// Append `line` plus a newline at the DOCUMENT END and send it, the way a
/// person typing at the bottom of the page does.
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

/// Pull the client doc back up to the session's text, the way the provider
/// does when the server broadcasts an update.
async fn resync(joined: &Joined, doc: &Doc) {
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
}

/// The client's view of the document right now.
fn client_text(doc: &Doc) -> String {
    let text = doc.get_or_insert_text("content");
    text.get_string(&doc.transact())
}

/// Publish a display name in awareness, the way a browser's provider does on
/// its first frame. Without it a room has connections but no names, and
/// `present` is empty however many people are in there.
async fn publish_name(joined: &Joined, doc: &Doc, name: &str) {
    let mut awareness = Awareness::new(doc.clone());
    awareness.set_local_state_raw(format!("{{\"user\":{{\"name\":\"{name}\"}}}}"));
    let update = awareness.update().unwrap();
    joined
        .session
        .handle_frame(joined.conn, &Message::Awareness(update).encode_v1())
        .await;
}

/// Throw away whatever the room has broadcast so far, so the next read is
/// about what one action did.
fn drain(rx: &mut broadcast::Receiver<Frame>) {
    while rx.try_recv().is_ok() {}
}

/// Every awareness state the room has broadcast since the last drain, as the
/// client id it is about and the JSON it carries ("null" for a state that was
/// taken away).
fn awareness_states(rx: &mut broadcast::Receiver<Frame>) -> Vec<(ClientID, String)> {
    let mut states = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        for message in messages_of(&frame.bytes) {
            if let Message::Awareness(update) = message {
                for (id, entry) in update.clients {
                    states.push((id, entry.json.to_string()));
                }
            }
        }
    }
    states
}

/// Who the room says is in it right now.
async fn joined_names(joined: &Joined) -> Vec<String> {
    joined.session.participants(None).await
}

/// An awareness frame published under somebody else's client id, which is what
/// a room cannot stop a connection from doing: the ids travel on the wire and
/// an agent's is a hash of a label that is on screen.
fn claim_frame(id: ClientID) -> Vec<u8> {
    let mut clients = HashMap::new();
    clients.insert(
        id,
        AwarenessUpdateEntry {
            clock: 9,
            json: "{\"user\":{\"name\":\"somebody else\"}}".into(),
        },
    );
    Message::Awareness(AwarenessUpdate { clients }).encode_v1()
}

/// An `append` edit of `alpha`, the shape an agent adding a line sends.
fn append_edit(content: &str, expected_checksum: Option<&str>) -> EditParams {
    EditParams {
        identifier: "alpha".to_string(),
        domain: "eng".to_string(),
        operation: "append".to_string(),
        content: Some(content.to_string()),
        expected_checksum: expected_checksum.map(str::to_string),
        ..EditParams::default()
    }
}

/// The checksum of a stored document, the way the engine computes one.
fn sha256_hex_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The one-based line a read's observation list reports for the bullet
/// carrying `needle`, which is how `split_engram` names the lines it moves.
fn line_of(read: &Value, needle: &str) -> usize {
    read["observations"]
        .as_array()
        .expect("a read lists observations")
        .iter()
        .find(|o| o["content"].as_str().unwrap_or_default().contains(needle))
        .and_then(|o| o["line"].as_u64())
        .expect("the typed observation is in the read") as usize
}

/// **The brief's first test.** A person is typing in the room; the agent's
/// edit composes with what they typed rather than over it, lands in the live
/// document, and says so in the receipt.
#[tokio::test]
async fn an_agent_edit_composes_with_a_typed_line_and_lands_live() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "a person typed this").await;

    let receipt = engine
        .edit_engram_as(
            &append_edit("and the agent added that", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");

    assert_eq!(
        receipt["landed"].as_str(),
        Some("live"),
        "the receipt says where it went: {receipt}"
    );
    assert!(
        receipt["present"].is_array(),
        "and who is in the room with it: {receipt}"
    );

    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("a person typed this") && live.contains("and the agent added that"),
        "both lines stand in the live document: {live:?}"
    );

    // Nothing was written behind the room's back: the room's own saver is
    // what makes an agent's edit durable, exactly as it is for a typed one.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the file is untouched until the room saves");

    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("a person typed this") && on_disk.contains("and the agent added that"),
        "and then the file carries both: {on_disk:?}"
    );
}

/// A set_frontmatter that leaves the live document saying what it says
/// composes nothing: the document keeps its text and its generated block,
/// the receipt says unchanged: true and names no landing. A stale checksum is
/// still refused first.
#[tokio::test]
async fn an_unchanged_set_frontmatter_composes_nothing_into_a_live_document() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "a person typed this").await;
    let before = client_text(&doc);
    let tags = |expected_checksum: Option<String>| EditParams {
        identifier: "alpha".to_string(),
        domain: "eng".to_string(),
        operation: "set_frontmatter".to_string(),
        key: Some("tags".to_string()),
        values: Some(vec!["Eng".to_string()]),
        expected_checksum,
        ..EditParams::default()
    };

    let err = engine
        .edit_engram_as(
            &tags(Some("0".repeat(64))),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stale edit"), "{err}");

    let receipt = engine
        .edit_engram_as(
            &tags(Some(sha256_hex_of(&before))),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit is answered");
    assert_eq!(receipt["unchanged"], json!(true), "{receipt}");
    assert!(receipt.get("landed").is_none(), "{receipt}");
    resync(&joined, &doc).await;
    assert_eq!(client_text(&doc), before, "nothing composed");
}

/// An edit that composed into an open room announces nothing: nothing
/// landed. The room's save a moment later is what announces, once.
#[tokio::test]
async fn an_agent_edit_into_an_open_room_announces_nothing_until_the_room_saves() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "a person typed this").await;
    let mut rx = engine.changes().subscribe();
    let receipt = engine
        .edit_engram_as(
            &append_edit("and the agent added that", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(receipt["landed"].as_str(), Some("live"));
    assert!(rx.try_recv().is_err(), "nothing landed, nothing announced");
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let saved = rx.try_recv().expect("the room's save announced");
    assert_eq!(saved.change.name(), "engram");
    assert!(rx.try_recv().is_err(), "once");
}

/// A guarded split of a page somebody is typing in is guarded against the
/// document, not against the file.
///
/// `read_engram` answers a live page from the room and hands back the LIVE
/// text's checksum - that is what makes read-then-edit work while somebody is
/// in there. The split used to read the file, refuse that checksum as stale,
/// and tell the caller to re-read - which answered the same value again. Worse,
/// with no checksum at all it planned the split against the file and then had
/// its own staged edit refuse it, because the staged edit compares against the
/// document: the verb could not run while anybody was typing, and it created
/// and rolled back an engram on every attempt.
#[tokio::test]
async fn a_guarded_split_of_a_live_engram_takes_the_live_checksum() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    // Enough typed lines that what stays behind still passes the verify rule
    // a split has to leave the source under.
    for line in [
        "- [fact] the first thing they typed #eng",
        "- [fact] the second thing they typed #eng",
        "- [fact] the third thing they typed #eng",
        "- [decision] a person typed this one #eng",
    ] {
        append_line(&joined, &doc, line).await;
    }

    let stored = sha256_hex_of(ALPHA);
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("eng".to_string()),
                share_link: None,
                include_images: false,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(read["live"], json!(true), "the page is open: {read}");
    let live = read["checksum"].as_str().unwrap().to_string();
    assert_ne!(live, stored, "and the room has moved past the file");

    let split = async |checksum: &str| {
        engine
            .split_engram(&crystalline_service::params::SplitParams {
                domain: "eng".to_string(),
                identifier: "alpha".to_string(),
                title: "Typed Line".to_string(),
                observations: vec![line_of(&read, "a person typed this one")],
                expected_checksum: Some(checksum.to_string()),
                ..Default::default()
            })
            .await
    };

    let refused = split(&stored).await.expect_err("the stored text is stale");
    assert!(
        refused.to_string().contains("stale"),
        "and it is told so in the words a stale edit uses: {refused}"
    );

    let receipt = split(&live).await.expect("the live checksum is the one");
    assert_eq!(
        receipt["new"]["permalink"],
        json!("typed-line"),
        "{receipt}"
    );

    resync(&joined, &doc).await;
    let text = client_text(&doc);
    assert!(
        !text.contains("a person typed this one"),
        "the line left the open document rather than the file behind it: {text:?}"
    );
    assert!(
        text.contains("- split_into [[typed-line]]"),
        "and the document carries the link to where it went: {text:?}"
    );
}

/// A guarded delete of a page somebody is typing in reads the same checksum
/// back.
///
/// The delete does not compose into the document - it ends the engram the
/// document is of - but the checksum a caller presents came from a read, and a
/// read of a live page answers the room. Comparing it against the file refused
/// the careful caller and pushed them towards the unguarded delete, which is
/// the call the guard exists to prevent.
#[tokio::test]
async fn a_guarded_delete_of_a_live_engram_takes_the_live_checksum() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed but never saved").await;

    let stored = sha256_hex_of(ALPHA);
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("eng".to_string()),
                share_link: None,
                include_images: false,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let live = read["checksum"].as_str().unwrap().to_string();
    assert_ne!(live, stored);

    let delete = async |checksum: &str| {
        engine
            .delete_engram(&crystalline_service::params::DeleteParams {
                identifier: "alpha".to_string(),
                domain: "eng".to_string(),
                expected_checksum: Some(checksum.to_string()),
            })
            .await
    };

    let refused = delete(&stored).await.expect_err("the stored text is stale");
    assert!(
        refused.to_string().contains("stale"),
        "and says so the way every stale guard says it: {refused}"
    );
    let receipt = delete(&live).await.expect("the live checksum is the one");
    assert_eq!(receipt["deleted"], json!(true), "{receipt}");
}

/// A retirement in a direct domain composes into the open room.
///
/// The verb had two arms: in review mode it went through the shared edit path,
/// which composes; in a direct domain it read the file, rewrote it and wrote it
/// back beside the room. The person typing then had their page retired
/// underneath them - their next save came back as a three-way merge notice, or
/// as a conflict they had to settle by hand. One arm now, so one answer.
#[tokio::test]
async fn a_retirement_in_a_direct_domain_composes_into_the_open_room() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed while it was being retired").await;

    engine
        .retire_engram(&crystalline_service::params::RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "deprecated".to_string(),
            successor: None,
            valid_to: None,
        })
        .await
        .expect("the retirement lands");

    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("status: deprecated"),
        "the retirement is in the document the person is looking at: {live:?}"
    );
    assert!(
        live.contains("typed while it was being retired"),
        "and what they typed is still there: {live:?}"
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(
        on_disk, ALPHA,
        "nothing was written behind the room's back; its saver is what lands this"
    );

    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("status: deprecated")
            && on_disk.contains("typed while it was being retired"),
        "and then the file carries both: {on_disk:?}"
    );
}

/// **The brief's second test.** A read taken while the room is open answers
/// the unsaved text and flags that it did.
#[tokio::test]
async fn a_mid_session_read_returns_the_unsaved_line_and_flags_live() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed but never saved").await;

    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("eng".to_string()),
                ..ReadParams::default()
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the read answers");

    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("typed but never saved"),
        "the read is answered from the live document: {read}"
    );
    assert_eq!(
        read["live"].as_bool(),
        Some(true),
        "and says so, so the agent knows the bytes are somebody's unsaved work: {read}"
    );
    assert!(read["present"].is_array(), "with who is in there: {read}");
}

/// **The brief's third test.** The checksum a caller guards an edit with is
/// evaluated against the LIVE text, so a checksum read before somebody typed
/// is stale exactly as it would be against a changed file.
#[tokio::test]
async fn a_stale_checksum_against_the_live_text_refuses() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;

    let stale = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("eng".to_string()),
                ..ReadParams::default()
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap()["checksum"]
        .as_str()
        .unwrap()
        .to_string();

    append_line(&joined, &doc, "typed after that read").await;

    let refused = engine
        .edit_engram_as(
            &append_edit("the agent's line", Some(&stale)),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect_err("a guarded edit against text that moved is refused");
    let said = refused.to_string();
    assert!(
        said.contains("stale edit"),
        "refused as the stale edit it is: {said}"
    );

    resync(&joined, &doc).await;
    assert!(
        !client_text(&doc).contains("the agent's line"),
        "and nothing of the refused edit reached the document"
    );
}

/// A CRLF engram read before anybody opened it hands out the checksum of
/// its CRLF bytes. A room then opened over it with nothing typed stands for
/// those same bytes, so the guarded edit is not refused.
#[tokio::test]
async fn a_guard_taken_before_a_clean_room_opened_over_a_crlf_file_still_holds() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    std::fs::write(tmp.path().join("eng/alpha.md"), ALPHA.replace('\n', "\r\n")).unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let read = |engine: Arc<Engine>| async move {
        engine
            .read_engram(
                &ReadParams {
                    identifier: "alpha".to_string(),
                    domain: Some("eng".to_string()),
                    ..ReadParams::default()
                },
                &crystalline_service::Scope::Unrestricted,
            )
            .await
            .unwrap()
    };
    let before = read(engine.clone()).await["checksum"]
        .as_str()
        .unwrap()
        .to_string();

    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let _doc = sync_client(&joined).await;
    let in_room = read(engine.clone()).await;
    assert_eq!(in_room["live"].as_bool(), Some(true));
    assert_eq!(
        in_room["checksum"].as_str().unwrap(),
        before,
        "a clean room hands out the stored bytes' checksum"
    );

    let edited = engine
        .edit_engram_as(
            &append_edit("guarded across the open", Some(&before)),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the guard taken before the room opened holds");
    assert_eq!(edited["landed"], "live");
}

/// The other way round: a checksum read while a clean room was open over a
/// CRLF engram still guards an edit after the room closed without a save.
#[tokio::test]
async fn a_guard_taken_in_a_clean_room_over_a_crlf_file_holds_after_it_closes() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let path = tmp.path().join("eng/alpha.md");
    std::fs::write(&path, ALPHA.replace('\n', "\r\n")).unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let _doc = sync_client(&joined).await;
    let checksum = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("eng".to_string()),
                ..ReadParams::default()
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap()["checksum"]
        .as_str()
        .unwrap()
        .to_string();

    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    sessions.dispose_if_empty(&joined.session).await;
    assert!(
        std::fs::read(&path).unwrap().contains(&b'\r'),
        "a room that only read the file left it as it was"
    );

    engine
        .edit_engram_as(
            &append_edit("guarded after the close", Some(&checksum)),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the guard taken in the clean room holds against the file");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains('\r'), "{on_disk:?}");
    assert!(on_disk.contains("guarded after the close"), "{on_disk:?}");
}

/// **Ruling 1.** In a domain that reviews changes, the live document an agent
/// composes into is the room over ITS OWN overlay document - never another
/// actor's, and with no join anywhere in it.
///
/// An actor is an account, so a person's room over their own draft and their
/// agent's write on the same account meet in one document because they ARE one
/// document. The second half is the half that matters: a room over somebody
/// ELSE's draft is standing open at the same address, and the agent's edit
/// does not touch it.
#[tokio::test]
async fn an_agent_edit_in_a_reviewing_domain_composes_into_its_own_overlay_room() {
    let (_tmp, engine, _scratch) = engine_fixture(true).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    // The machine owner is who a local agent acts as, so the owner's room is
    // the agent's own overlay document.
    let mine = sessions.join("eng", "alpha", Some("owner")).await.unwrap();
    let mine_doc = sync_client(&mine).await;
    append_line(&mine, &mine_doc, "the owner typed this").await;
    // And somebody else has a room over their own draft of the same page.
    let hers = sessions.join("eng", "alpha", Some("alice")).await.unwrap();
    let hers_doc = sync_client(&hers).await;
    append_line(&hers, &hers_doc, "alice typed this").await;

    let receipt = engine
        .edit_engram_as(
            &append_edit("the agent added that", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert_eq!(receipt["landed"].as_str(), Some("live"), "{receipt}");
    assert_eq!(
        receipt["draft"].as_bool(),
        Some(true),
        "and it is still a draft of the owner's: {receipt}"
    );

    resync(&mine, &mine_doc).await;
    assert!(
        client_text(&mine_doc).contains("the agent added that"),
        "the agent's own overlay document has it: {:?}",
        client_text(&mine_doc)
    );
    resync(&hers, &hers_doc).await;
    assert!(
        !client_text(&hers_doc).contains("the agent added that"),
        "and alice's draft of the same page does not: {:?}",
        client_text(&hers_doc)
    );
}

/// A section edit of a MANIFEST somebody has open composes into their
/// document, and its receipt says what every other landing says: which
/// repeated heading was dropped, and what the MANIFEST rules find in the text
/// that went live. The text is not durable yet, but it is on their screen and
/// the room's saver will write it, so the finding is owed now.
#[tokio::test]
async fn a_live_manifest_edit_reports_the_dropped_heading_and_the_findings() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "manifest", None).await.unwrap();
    let doc = sync_client(&joined).await;
    let before = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();

    let receipt = engine
        .edit_engram_as(
            &EditParams {
                identifier: "manifest".to_string(),
                domain: "eng".to_string(),
                operation: "replace_section".to_string(),
                section: Some("## When to Use".to_string()),
                content: Some("## When to Use".to_string()),
                ..EditParams::default()
            },
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert_eq!(receipt["landed"].as_str(), Some("live"), "{receipt}");
    assert_eq!(receipt["heading_stripped"], "## When to Use", "{receipt}");
    assert!(
        receipt["manifest_findings"]
            .as_array()
            .is_some_and(|findings| findings.iter().any(|f| f["code"] == "M004")),
        "{receipt}"
    );

    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert_eq!(live.matches("## When to Use").count(), 1, "{live:?}");
    assert!(!live.contains("Route here for eng questions"), "{live:?}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
        before,
        "the file waits for the room's saver"
    );
}

/// A capture titled after the MANIFEST, with `overwrite`, while the MANIFEST is
/// open in the editor. Its destination slugs to the MANIFEST's own address,
/// which used to let the live arm replace the document in the room with an
/// ordinary engram. It is refused before it reaches the room: a MANIFEST
/// changes through edit_engram only, and neither the room's document nor the
/// file moves.
#[tokio::test]
async fn a_capture_titled_after_an_open_manifest_is_refused() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "manifest", None).await.unwrap();
    let _doc = sync_client(&joined).await;
    let before = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();

    let err = engine
        .write_engram(&WriteParams {
            domain: "eng".to_string(),
            title: "MANIFEST".to_string(),
            content: "## Scope\n\n- Everything about eng\n".to_string(),
            folder: None,
            engram_type: None,
            tags: vec![],
            status: None,
            metadata: None,
            overwrite: true,
            share_link: None,
            model: None,
        })
        .await
        .expect_err("a capture never replaces the MANIFEST, open or not");
    assert!(err.to_string().contains("edit_engram"), "{err}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
        before,
        "the file is untouched"
    );
}

/// A room that is open over a DIFFERENT document leaves the ordinary write
/// path exactly as it was: the live arm is asked about one document, not about
/// the domain.
#[tokio::test]
async fn an_edit_of_a_page_no_room_is_open_over_still_writes_the_file() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "manifest", None).await.unwrap();
    let _doc = sync_client(&joined).await;

    let receipt = engine
        .edit_engram_as(
            &append_edit("written straight to the file", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert!(
        receipt.get("landed").is_none(),
        "nothing to compose into, so the receipt says nothing about live: {receipt}"
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("written straight to the file"),
        "{on_disk:?}"
    );
}

/// A move rewrites a link in an engram somebody has open by composing into
/// the room, the way an edit does, and keeps what the person typed.
///
/// The typed line sits right under the link on purpose: a rewrite written to
/// the file and merged into the room three ways collides with it there, and
/// the person would get a conflict from a move they never saw. Composed from
/// the live text, it cannot collide. Nothing is written behind the room; its
/// own save writes both.
#[tokio::test]
async fn a_move_relinks_a_linker_open_in_a_room_and_keeps_the_typing() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    std::fs::write(
        tmp.path().join("eng/beta.md"),
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Beta\n\nSee [[alpha]].\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "beta", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed while alpha moved").await;

    let receipt = engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".to_string(),
                domain: "eng".to_string(),
                destination: "gamma.md".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the move lands");
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    assert_eq!(
        receipt["rewritten"][0]["landed"], "live",
        "the receipt says the rewrite went into the open room: {receipt}"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(
        on_disk.contains("See [[alpha]]."),
        "nothing was written behind the room's back: {on_disk:?}"
    );
    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("See [[gamma]]."),
        "the rewrite is in the document the person is looking at: {live:?}"
    );
    assert!(
        live.contains("typed while alpha moved"),
        "and what the person typed is still there: {live:?}"
    );

    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(
        on_disk.contains("See [[gamma]].") && on_disk.contains("typed while alpha moved"),
        "the room's save writes both: {on_disk:?}"
    );
}

/// The linker every room test below opens: `beta`, linking `alpha`.
const BETA: &str = "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Beta\n\nSee [[alpha]].\n";

/// Move `alpha` to `gamma.md`, so every link to it follows.
async fn move_alpha_to_gamma(engine: &Engine) -> Value {
    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".to_string(),
                domain: "eng".to_string(),
                destination: "gamma.md".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the move lands")
}

/// A room whose last connection left and whose final save ran saves nothing
/// any more, even while it still stands in the registry for the moment
/// before it is disposed. A move in that moment writes the linker's file, so
/// its rewrite is not composed into a room nobody saves again.
#[tokio::test]
async fn a_move_writes_the_file_of_a_linker_whose_room_already_said_goodbye() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    std::fs::write(tmp.path().join("eng/beta.md"), BETA).unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "beta", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before leaving").await;
    assert!(
        joined.session.remove_conn(joined.conn).await,
        "the last one left"
    );
    joined.session.final_save().await;
    // Not disposed yet: the socket's teardown disposes right after this.

    let receipt = move_alpha_to_gamma(&engine).await;
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    assert!(
        receipt["rewritten"][0].get("landed").is_none(),
        "not a live landing: {receipt}"
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(
        on_disk.contains("See [[gamma]].") && on_disk.contains("typed before leaving"),
        "the rewrite is in the file, beside what the final save wrote: {on_disk:?}"
    );
}

/// Between the last connection leaving and the room's final save, the room
/// still saves what it holds: that final save is still to come. A move in
/// that moment composes into the room, so the final save writes the typing
/// and the rewrite together, instead of writing the file behind the room and
/// leaving the final save to collide with it.
#[tokio::test]
async fn a_move_between_the_last_leave_and_the_final_save_lands_with_the_typing() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    std::fs::write(tmp.path().join("eng/beta.md"), BETA).unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "beta", None).await.unwrap();
    let doc = sync_client(&joined).await;
    // Right under the link, where a three-way merge of a file rewrite
    // would collide with it.
    append_line(&joined, &doc, "typed before leaving").await;
    assert!(
        joined.session.remove_conn(joined.conn).await,
        "the last one left"
    );

    let receipt = move_alpha_to_gamma(&engine).await;
    assert_eq!(receipt["rewritten"][0]["landed"], "live", "{receipt}");
    joined.session.final_save().await;

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(
        on_disk.contains("See [[gamma]].") && on_disk.contains("typed before leaving"),
        "the final save wrote both: {on_disk:?}"
    );
}

/// A room in conflict saves nothing until a person resolves it, and may then
/// keep the other side. A move writes the linker's file instead of composing
/// into such a room, so the rewrite does not hang on how the conflict ends.
#[tokio::test]
async fn a_move_writes_the_file_of_a_linker_whose_room_is_in_conflict() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let beta = tmp.path().join("eng/beta.md");
    std::fs::write(&beta, BETA).unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "beta", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed in the room").await;
    // Another writer appended a different line at the same place.
    std::fs::write(&beta, format!("{BETA}written on disk\n")).unwrap();
    engine.sync(None).await.unwrap();
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let live = sessions
        .live_text("eng", "beta", None)
        .await
        .expect("the room stands");
    assert!(!live.saving, "the room is in conflict and saves nothing");

    let receipt = move_alpha_to_gamma(&engine).await;
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    assert!(
        receipt["rewritten"][0].get("landed").is_none(),
        "not a live landing: {receipt}"
    );
    let on_disk = std::fs::read_to_string(&beta).unwrap();
    assert!(
        on_disk.contains("See [[gamma]].") && on_disk.contains("written on disk"),
        "the rewrite is in the file: {on_disk:?}"
    );
}

/// A move that keeps the engram's address (a new path, the same permalink) is
/// allowed while the engram is open in the editor, and the room keeps saving
/// to wherever the engram now lives: it saves by address, not by path. A move
/// that changes the address closes the room with `moved` instead
/// (`an_agent_move_over_mcp_closes_the_persons_room_with_moved`).
#[tokio::test]
async fn a_path_only_move_of_an_open_engram_keeps_the_rooms_typing() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before the move").await;

    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".to_string(),
                domain: "eng".to_string(),
                destination: "guides/alpha.md".to_string(),
                destination_domain: None,
                permalink: Some("keep".to_string()),
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("a path-only move is not refused by the open room");
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;

    assert!(
        !tmp.path().join("eng/alpha.md").exists(),
        "the old path is gone"
    );
    let moved = std::fs::read_to_string(tmp.path().join("eng/guides/alpha.md")).unwrap();
    assert!(
        moved.contains("typed before the move"),
        "the room saved its typing at the new path: {moved:?}"
    );
    let live = sessions
        .live_text("eng", "alpha", None)
        .await
        .expect("the room stands");
    assert!(live.saving, "and raised no conflict");
}

/// A path-only move rebases the engram's own relative links while its room
/// stays open. The room's next save meets the rebase as a change under it and
/// merges it, so the file ends with both the typing and the rebased links.
#[tokio::test]
async fn a_path_only_move_under_an_open_room_keeps_the_typing_and_the_rebased_links() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let dir = tmp.path().join("eng");
    std::fs::create_dir_all(dir.join("guides")).unwrap();
    std::fs::write(
        dir.join("guides/peer.md"),
        "---\ntype: engram\ntitle: Peer\npermalink: peer\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Peer\n\nThe peer.\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("alpha.md"),
        ALPHA.replace(
            "A rule about alpha.\n",
            "A rule about alpha.\n\nSee [the peer](guides/peer.md).\n",
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before the move").await;

    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".to_string(),
                domain: "eng".to_string(),
                destination: "guides/alpha.md".to_string(),
                destination_domain: None,
                permalink: Some("keep".to_string()),
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("a path-only move is not refused by the open room");
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;

    let moved = std::fs::read_to_string(dir.join("guides/alpha.md")).unwrap();
    assert!(
        moved.contains("typed before the move") && moved.contains("See [the peer](peer.md)."),
        "the typing and the rebased link both landed: {moved:?}"
    );
    assert!(!moved.contains("guides/peer.md"), "{moved:?}");
    let live = sessions
        .live_text("eng", "alpha", None)
        .await
        .expect("the room stands");
    assert!(live.saving, "and raised no conflict");
    assert!(
        live.text.contains("See [the peer](peer.md).")
            && live.text.contains("typed before the move"),
        "the room holds what the file holds: {:?}",
        live.text
    );
}

/// **Task 41c.** A room's save that found the engram at its old path and
/// then waited for that path's lock while a path-only move held it finds the
/// file gone. The engram still answers to the room's address at its new
/// path, so the save is made there instead of raising a deletion conflict.
///
/// Lined up on alpha's lock, which is first come, first served: the test
/// holds it, the move queues on it, then the room's save does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_save_queued_behind_a_path_only_move_lands_at_the_new_path() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed while it moved").await;

    let lock = engine.file_write_lock(&tmp.path().join("eng/alpha.md"));
    let held = lock.clone().lock_owned().await;
    let mover = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .move_engram(
                    &crystalline_service::params::MoveParams {
                        identifier: "alpha".to_string(),
                        domain: "eng".to_string(),
                        destination: "guides/alpha.md".to_string(),
                        destination_domain: None,
                        permalink: Some("keep".to_string()),
                        update_links: None,
                    },
                    &crystalline_service::Scope::Unrestricted,
                )
                .await
        })
    };
    let waiting_for = |count: usize| {
        let lock = lock.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(20), async {
                // The map's handle, the test's, the guard's and this
                // future's own make four; each queued writer adds one.
                while Arc::strong_count(&lock) < count {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the writer queues on alpha's lock");
        }
    };
    waiting_for(5).await;
    let saver = {
        let session = joined.session.clone();
        tokio::spawn(async move {
            session
                .tick_save(Instant::now() + Duration::from_secs(60))
                .await
        })
    };
    waiting_for(6).await;
    drop(held);
    mover.await.unwrap().expect("the move lands");
    saver.await.unwrap();

    let moved = std::fs::read_to_string(tmp.path().join("eng/guides/alpha.md")).unwrap();
    assert!(moved.contains("typed while it moved"), "{moved:?}");
    assert!(!tmp.path().join("eng/alpha.md").exists());
    let live = sessions.live_text("eng", "alpha", None).await.unwrap();
    assert!(live.saving, "the room raised no conflict");
}

/// The files under `<state>/kept/eng/engram/` a closing room's text was kept
/// in, for `beta.md`.
fn kept_beta(tmp: &tempfile::TempDir) -> Vec<String> {
    let dir = tmp.path().join("state/kept/eng/engram");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("beta.md.")
        })
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect()
}

/// Open `beta` in a room, type a line, and write a different line at the
/// same place on disk, so the room's next save collides with the file.
async fn beta_room_colliding_with_the_file(
    tmp: &tempfile::TempDir,
    engine: &Arc<Engine>,
    sessions: &Arc<CollabSessions>,
) -> Joined {
    let beta = tmp.path().join("eng/beta.md");
    std::fs::write(&beta, BETA).unwrap();
    engine.sync(None).await.unwrap();
    let joined = sessions.join("eng", "beta", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed in the room").await;
    std::fs::write(&beta, format!("{BETA}written on disk\n")).unwrap();
    engine.sync(None).await.unwrap();
    joined
}

/// A room still in conflict when its last connection leaves saves nothing,
/// so the server keeps its text, as it does for a refused last save.
#[tokio::test]
async fn a_room_still_in_conflict_at_its_close_keeps_its_text() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = beta_room_colliding_with_the_file(&tmp, &engine, &sessions).await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    assert!(
        !sessions
            .live_text("eng", "beta", None)
            .await
            .unwrap()
            .saving
    );

    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    let kept = kept_beta(&tmp);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].contains("typed in the room"), "{kept:?}");
}

/// A room over a review draft keeps its unsaved text in its owner's draft
/// folder, never beside the domain's own texts, so the kept-text listing
/// gives it to that owner only.
#[tokio::test]
async fn a_draft_room_keeps_its_text_under_its_owner() {
    let (tmp, engine, _scratch) = engine_fixture(true).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", Some("ada")).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "ada typed this in her draft").await;
    // The owner locks the domain by hand: no save lands any more.
    let manifest = tmp.path().join("eng/MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replacen("\n---\n", "\nread_only: on\n---\n", 1),
    )
    .unwrap();
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;

    let root = tmp.path().join("state/kept");
    assert!(
        !root.join("eng/engram").exists(),
        "not beside the domain's own"
    );
    let found = crystalline_core::kept::scan(&root);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].owner.as_deref(), Some("ada"));
    assert_eq!(found[0].path, "alpha.md");
    let held = crystalline_core::kept::read(&found[0]).unwrap();
    assert!(held.contains("ada typed this in her draft"), "{held}");
    let ada = crystalline_service::Scope::User {
        account: "ada".into(),
        admin: false,
    };
    let bob = crystalline_service::Scope::User {
        account: "bob".into(),
        admin: false,
    };
    assert_eq!(engine.kept_texts("eng", &ada).await.unwrap().len(), 1);
    assert!(engine.kept_texts("eng", &bob).await.unwrap().is_empty());
}

/// A room whose final pass itself runs into a conflict keeps its text too.
#[tokio::test]
async fn a_final_pass_that_ends_in_conflict_keeps_the_rooms_text() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = beta_room_colliding_with_the_file(&tmp, &engine, &sessions).await;

    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    let kept = kept_beta(&tmp);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].contains("typed in the room"), "{kept:?}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(
        on_disk.contains("written on disk") && !on_disk.contains("typed in the room"),
        "the file keeps the other writer's text: {on_disk:?}"
    );
}

/// An `append` edit of `identifier` in `eng`.
fn append_to(identifier: &str, content: &str, expected_checksum: Option<&str>) -> EditParams {
    EditParams {
        identifier: identifier.to_string(),
        ..append_edit(content, expected_checksum)
    }
}

/// Replace the whole client document and send it, the way a person who
/// selected everything and typed over it does.
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

/// The room over `alpha` in `eng` with its save refused: the person replaced
/// the page with text that has no frontmatter, which the saver will not
/// write. `overlay` is whose document the room is over.
async fn alpha_room_whose_save_failed(
    sessions: &Arc<CollabSessions>,
    overlay: Option<&str>,
) -> (Joined, Doc) {
    room_whose_save_failed(sessions, "alpha", overlay).await
}

/// [`alpha_room_whose_save_failed`] over any engram of `eng`.
async fn room_whose_save_failed(
    sessions: &Arc<CollabSessions>,
    permalink: &str,
    overlay: Option<&str>,
) -> (Joined, Doc) {
    use crystalline_service::collab::control::{self, Control};
    let joined = sessions.join("eng", permalink, overlay).await.unwrap();
    let doc = sync_client(&joined).await;
    publish_name(&joined, &doc, "Grace Hopper").await;
    replace_all(&joined, &doc, "no frontmatter, typed by a person\n").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    let live = sessions
        .live_text("eng", permalink, overlay)
        .await
        .expect("the room stands");
    assert!(!live.saving, "the room's save failed and it saves nothing");
    (joined, doc)
}

/// **Task 41b.** An agent's edit of a page whose room is in conflict does
/// not compose into the room: the room saves nothing until a person settles
/// the conflict, and may then keep the other side. The edit lands in the
/// file instead, and the receipt says the room is waiting, with the note an
/// agent passes on.
#[tokio::test]
async fn an_edit_of_a_page_whose_room_is_in_conflict_lands_in_the_file() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = beta_room_colliding_with_the_file(&tmp, &engine, &sessions).await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    assert!(
        !sessions
            .live_text("eng", "beta", None)
            .await
            .unwrap()
            .saving
    );

    let receipt = engine
        .edit_engram_as(
            &append_to("beta", "the agent added this", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    assert_eq!(
        receipt["room_note"],
        crystalline_service::engine::ROOM_WAITING_NOTE,
        "{receipt}"
    );
    assert!(receipt["present"].is_array(), "{receipt}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(
        on_disk.contains("the agent added this") && on_disk.contains("written on disk"),
        "the edit is in the file: {on_disk:?}"
    );
    let live = sessions.live_text("eng", "beta", None).await.unwrap();
    assert!(
        live.text.contains("typed in the room") && !live.text.contains("the agent added this"),
        "and the room keeps the person's text, untouched: {:?}",
        live.text
    );
}

/// **Task 41b.** The room saves when the edit looks, and falls into conflict
/// before the edit composes. The compose decides again under the room's own
/// lock, so the edit is not composed into a room that stopped saving: it
/// lands in the file, and the room is waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_that_stops_saving_before_the_compose_gets_the_file_write() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = beta_room_colliding_with_the_file(&tmp, &engine, &sessions).await;
    assert!(
        sessions
            .live_text("eng", "beta", None)
            .await
            .unwrap()
            .saving
    );

    let (entered, release) = engine.hold_next_live_compose_until();
    let editing = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .edit_engram_as(
                    &append_to("beta", "the agent added this", None),
                    None,
                    &crystalline_service::Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the edit reaches the room");
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    assert!(
        !sessions
            .live_text("eng", "beta", None)
            .await
            .unwrap()
            .saving
    );
    release.notify_one();
    let receipt = editing.await.unwrap().expect("the edit lands");
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(on_disk.contains("the agent added this"), "{on_disk:?}");
    let live = sessions.live_text("eng", "beta", None).await.unwrap();
    assert!(
        !live.text.contains("the agent added this"),
        "{:?}",
        live.text
    );
}

/// **Task 41b, round 2.** A read shows what an edit would write to. A room in
/// conflict does not save, so an edit goes to the file behind it; the read
/// therefore answers the file and its checksum, says the room is waiting
/// with a note to pass on, and names who is in there. A guarded edit with
/// that checksum then lands in the file.
#[tokio::test]
async fn a_read_of_a_page_whose_room_is_in_conflict_answers_what_an_edit_writes_to() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = beta_room_colliding_with_the_file(&tmp, &engine, &sessions).await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let path = tmp.path().join("eng/beta.md");

    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "beta".to_string(),
                domain: Some("eng".to_string()),
                ..ReadParams::default()
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(read.get("live").is_none(), "{read}");
    assert_eq!(read["room"], "waiting", "{read}");
    assert_eq!(
        read["room_note"],
        crystalline_service::engine::ROOM_READ_NOTE,
        "{read}"
    );
    assert!(read["present"].is_array(), "{read}");
    assert_eq!(
        read["checksum"].as_str().unwrap(),
        sha256_hex_of(&std::fs::read_to_string(&path).unwrap()),
        "the file's checksum: {read}"
    );
    let content = read["content"].as_str().unwrap_or_default();
    assert!(
        content.contains("written on disk") && !content.contains("typed in the room"),
        "the file's text: {read}"
    );

    let receipt = engine
        .edit_engram_as(
            &append_to(
                "beta",
                "guarded by the read",
                Some(read["checksum"].as_str().unwrap()),
            ),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the read's checksum guards the file write");
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    assert_eq!(
        receipt["room_note"],
        crystalline_service::engine::ROOM_WAITING_NOTE,
        "{receipt}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("guarded by the read")
    );
}

/// **Task 41b, round 2.** The guarded verbs besides the edit read what the
/// read read. A delete guarded with the checksum a read of a page whose room
/// is in conflict handed out (the file's) is compared against the file, not
/// against the room's text, and lands.
#[tokio::test]
async fn a_guarded_delete_past_a_room_in_conflict_compares_the_saved_version() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = beta_room_colliding_with_the_file(&tmp, &engine, &sessions).await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let path = tmp.path().join("eng/beta.md");
    let checksum = sha256_hex_of(&std::fs::read_to_string(&path).unwrap());

    engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "beta".to_string(),
            domain: "eng".to_string(),
            expected_checksum: Some(checksum),
        })
        .await
        .expect("the saved version's checksum guards the delete");
    assert!(!path.exists());
}

/// **Task 41b.** A room whose save was refused saves nothing either, so an
/// agent's edit goes to the file, not into the room.
#[tokio::test]
async fn an_edit_of_a_page_whose_room_cannot_save_lands_in_the_file() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (_joined, _doc) = alpha_room_whose_save_failed(&sessions, None).await;

    let receipt = engine
        .edit_engram_as(
            &append_edit("the agent added this", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    assert_eq!(receipt["present"], json!(["Grace Hopper"]), "{receipt}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("A rule about alpha.") && on_disk.contains("the agent added this"),
        "{on_disk:?}"
    );
    let live = sessions.live_text("eng", "alpha", None).await.unwrap();
    assert!(
        !live.text.contains("the agent added this"),
        "{:?}",
        live.text
    );
}

/// **Task 41b.** The same in a domain that reviews changes: the room over
/// the agent's own draft cannot save, so the edit lands in the draft row,
/// and the room is left as the person has it.
#[tokio::test]
async fn an_edit_of_a_draft_whose_room_cannot_save_lands_in_the_draft() {
    let (tmp, engine, _scratch) = engine_fixture(true).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (_joined, _doc) = alpha_room_whose_save_failed(&sessions, Some("owner")).await;

    let receipt = engine
        .edit_engram_as(
            &append_edit("the agent added this", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert_eq!(receipt["draft"], true, "{receipt}");
    assert_eq!(receipt["room"], "waiting", "{receipt}");
    // The draft is in the database, so the note says saved, not on disk.
    assert!(
        receipt["room_note"]
            .as_str()
            .is_some_and(|note| note.starts_with("your write is saved")),
        "{receipt}"
    );
    let draft = engine
        .overlay_draft_at("eng", "owner", "alpha.md")
        .await
        .unwrap()
        .expect("the edit made the draft");
    assert!(draft.content.contains("the agent added this"), "{draft:?}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA,
        "the folder is untouched"
    );
    let live = sessions
        .live_text("eng", "alpha", Some("owner"))
        .await
        .unwrap();
    assert!(
        !live.text.contains("the agent added this"),
        "{:?}",
        live.text
    );
}

/// **Task 41b.** A room whose final save has run will not save again, even
/// while it still stands in the registry for the moment before it is
/// disposed. An edit in that moment writes the file, and says nothing about
/// a room nobody is in any more.
#[tokio::test]
async fn an_edit_after_the_rooms_final_save_lands_in_the_file() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before leaving").await;
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;

    let receipt = engine
        .edit_engram_as(
            &append_edit("the agent added this", None),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert!(receipt.get("landed").is_none(), "{receipt}");
    assert!(receipt.get("room").is_none(), "{receipt}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("typed before leaving") && on_disk.contains("the agent added this"),
        "the final save's text and the edit: {on_disk:?}"
    );
}

/// **Task 41b.** Somebody types between the edit's look at the room and its
/// compose. The compose is refused, because the diff to a target computed
/// from the old text would take their typing back, and the edit is applied
/// again to what the room holds now. Their line stays and the agent's line
/// lands once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typing_between_the_look_and_the_compose_is_kept() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed first").await;

    let (entered, release) = engine.hold_next_live_compose_until();
    let editing = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .edit_engram_as(
                    &append_edit("the agent added this", None),
                    None,
                    &crystalline_service::Scope::Unrestricted,
                )
                .await
        })
    };
    entered.notified().await;
    append_line(&joined, &doc, "typed in the gap").await;
    release.notify_one();
    let receipt = editing.await.unwrap().expect("the edit lands");
    assert_eq!(receipt["landed"], "live", "{receipt}");

    let live = sessions.live_text("eng", "alpha", None).await.unwrap();
    assert!(
        live.text.contains("typed first") && live.text.contains("typed in the gap"),
        "the typing stays: {:?}",
        live.text
    );
    assert_eq!(
        live.text.matches("the agent added this").count(),
        1,
        "the agent's line lands once: {:?}",
        live.text
    );
}

/// **Task 41b.** Somebody keeps typing through every try. The edit stops
/// trying the room and writes the file, and the room takes the change in by
/// its merge; nothing typed is taken back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_that_never_finds_a_gap_in_the_typing_lands_in_the_file() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed first").await;

    let mut gate = engine.hold_next_live_compose_until();
    let editing = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .edit_engram_as(
                    &EditParams {
                        identifier: "alpha".to_string(),
                        domain: "eng".to_string(),
                        operation: "find_replace".to_string(),
                        find_text: Some("# Alpha".to_string()),
                        content: Some("# Alpha, revised".to_string()),
                        ..EditParams::default()
                    },
                    None,
                    &crystalline_service::Scope::Unrestricted,
                )
                .await
        })
    };
    for round in 0..3 {
        tokio::time::timeout(Duration::from_secs(10), gate.0.notified())
            .await
            .expect("the edit tries the room again");
        append_line(&joined, &doc, &format!("typed in gap {round}")).await;
        let release = gate.1.clone();
        if round < 2 {
            gate = engine.hold_next_live_compose_until();
        }
        release.notify_one();
    }
    let receipt = editing.await.unwrap().expect("the edit lands");
    // The edit went to the file, never into the room, so the receipt never
    // says live. The room took the file in by its three-way merge (the
    // title line and the typed lines are far enough apart not to collide),
    // so nothing waits on the person and there is no room note.
    assert_eq!(receipt["landed"], "file", "{receipt}");
    assert!(receipt.get("room").is_none(), "{receipt}");

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.contains("# Alpha, revised"), "{on_disk:?}");
    let live = sessions.live_text("eng", "alpha", None).await.unwrap();
    assert!(live.text.contains("# Alpha, revised"), "{:?}", live.text);
    for line in [
        "typed first",
        "typed in gap 0",
        "typed in gap 1",
        "typed in gap 2",
    ] {
        assert!(live.text.contains(line), "{line} stays: {:?}", live.text);
    }
}

/// **Task 41b.** A guarded edit whose room moved between its look and its
/// compose is refused as stale rather than tried again: the text its
/// checksum named is gone. Nothing is composed and nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guarded_edit_whose_room_moved_under_it_is_stale() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed first").await;
    let checksum = sessions
        .live_text("eng", "alpha", None)
        .await
        .unwrap()
        .checksum;

    let (entered, release) = engine.hold_next_live_compose_until();
    let editing = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .edit_engram_as(
                    &append_edit("the agent added this", Some(&checksum)),
                    None,
                    &crystalline_service::Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the edit reaches the room");
    append_line(&joined, &doc, "typed in the gap").await;
    release.notify_one();
    let err = editing.await.unwrap().expect_err("the guard is stale");
    assert!(err.to_string().contains("stale edit"), "{err}");

    let live = sessions.live_text("eng", "alpha", None).await.unwrap();
    assert!(
        !live.text.contains("the agent added this"),
        "{:?}",
        live.text
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA
    );
}

/// **Task 41b.** The checksum rule on a virtual domain: the room over the
/// row cannot save, so the edit goes to the row, guarded by the checksum of
/// the room's text that the row never had. The live arm would have accepted
/// it, so the row's compare and swap takes the row as it was just read.
#[tokio::test]
async fn a_checksum_read_from_a_virtual_room_that_cannot_save_guards_the_row_write() {
    let scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::virtual_domain());
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
    engine
        .write_engram(&wholesale_capture("Alpha", "A rule about alpha.", false))
        .await
        .expect("alpha lands");
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (_joined, _doc) = alpha_room_whose_save_failed(&sessions, None).await;
    let live_sum = sessions
        .live_text("eng", "alpha", None)
        .await
        .unwrap()
        .checksum;

    let receipt = engine
        .edit_engram_as(
            &append_edit("guarded by the room's text", Some(&live_sum)),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the room's checksum is accepted");
    assert_eq!(receipt["landed"], "file", "{receipt}");
    let stored = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("eng".to_string()),
                ..ReadParams::default()
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        stored.get("live").is_none() && stored["room"] == "waiting",
        "the read answers the row and names the waiting room: {stored}"
    );
    let row = engine
        .engram_text("eng", "alpha")
        .await
        .expect("the row is there")
        .content;
    assert!(row.contains("guarded by the room's text"), "{row:?}");
    drop(scratch);
}

/// The checksum the room over `alpha` stands for: the token a caller holds
/// who read the page while that room still saved.
async fn room_checksum(sessions: &CollabSessions) -> String {
    sessions
        .live_text("eng", "alpha", None)
        .await
        .expect("the room stands")
        .checksum
}

/// **Task 41b.** A caller that read a page while its room could not save
/// holds the checksum of the room's unsaved text, which the file never had.
/// The edit then lands in the file, and that checksum is still accepted:
/// the live arm would have accepted it, and the file is still the one the
/// room last saved. A checksum of neither text is refused.
#[tokio::test]
async fn a_checksum_read_from_a_room_that_cannot_save_guards_the_file_write() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (_joined, _doc) = alpha_room_whose_save_failed(&sessions, None).await;
    let live_sum = room_checksum(&sessions).await;
    let path = tmp.path().join("eng/alpha.md");
    assert_ne!(
        live_sum,
        sha256_hex_of(&std::fs::read_to_string(&path).unwrap()),
        "the room's text is not the file's"
    );

    let stale = engine
        .edit_engram_as(
            &append_edit("never written", Some(&"0".repeat(64))),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect_err("a checksum of neither text is stale");
    assert!(stale.to_string().contains("stale edit"), "{stale}");

    let receipt = engine
        .edit_engram_as(
            &append_edit("guarded by the room's text", Some(&live_sum)),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the room's checksum is accepted");
    assert_eq!(receipt["landed"], "file", "{receipt}");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("guarded by the room's text"),
        "{on_disk:?}"
    );
    assert!(!on_disk.contains("never written"), "{on_disk:?}");

    let file_sum = sha256_hex_of(&on_disk);
    engine
        .edit_engram_as(
            &append_edit("guarded by the file", Some(&file_sum)),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the file's own checksum is accepted too");
}

/// **Task 41b review.** The room's checksum stands for its text over the
/// file it last saved. Once another writer changed that file, a caller
/// holding the room's checksum has not seen the change, so the edit is
/// refused as stale rather than written over it.
#[tokio::test]
async fn a_room_checksum_is_stale_once_another_writer_changed_the_file() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (_joined, _doc) = alpha_room_whose_save_failed(&sessions, None).await;
    let live_sum = room_checksum(&sessions).await;
    let path = tmp.path().join("eng/alpha.md");
    std::fs::write(&path, format!("{ALPHA}written by a third writer\n")).unwrap();

    let err = engine
        .edit_engram_as(
            &append_edit("never written", Some(&live_sum)),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect_err("the file moved past the room's base");
    assert!(err.to_string().contains("stale edit"), "{err}");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("written by a third writer") && !on_disk.contains("never written"),
        "{on_disk:?}"
    );
}

/// **Task 41b review.** The room closes between the overwrite's look and its
/// compose. There is no room to refuse on behalf of any more, so the
/// overwrite takes the ordinary path and replaces the file, as it does for
/// a page nobody has open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_overwrite_whose_room_closed_before_the_compose_replaces_the_file() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed before leaving").await;

    let (entered, release) = engine.hold_next_live_compose_until();
    let writing = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .write_engram_present(
                    &wholesale_capture("Alpha", REPLACEMENT, true),
                    None,
                    &crystalline_service::Scope::Unrestricted,
                    None,
                    None,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("the overwrite reaches the room");
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    sessions.dispose_if_empty(&joined.session).await;
    release.notify_one();
    let receipt = writing
        .await
        .unwrap()
        .expect("the overwrite takes the ordinary path");
    assert!(receipt.get("landed").is_none(), "{receipt}");
    assert_eq!(receipt["action"], "written", "{receipt}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.contains(REPLACEMENT), "{on_disk:?}");
}

/// **Task 41b.** A wholesale replace of a page whose room cannot save is
/// refused: it would land over text the room holds and nobody has saved,
/// and composing it into the room would land nowhere. The refusal names who
/// is in there, and neither the file nor the room moves.
#[tokio::test]
async fn an_overwrite_of_a_page_whose_room_cannot_save_is_refused_naming_who_is_in_there() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (_joined, _doc) = alpha_room_whose_save_failed(&sessions, None).await;

    let err = engine
        .write_engram_present(
            &wholesale_capture("Alpha", REPLACEMENT, true),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            None,
        )
        .await
        .expect_err("the replace is refused");
    assert!(err.to_string().contains("Grace Hopper"), "{err}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA,
        "the file is untouched"
    );
    let live = sessions.live_text("eng", "alpha", None).await.unwrap();
    assert!(!live.text.contains(REPLACEMENT), "{:?}", live.text);
}

/// Every verb that goes through the shared edit body meets the live document,
/// not only `edit_engram`.
///
/// A retirement reaches it through `Engine::apply_source_edit`, the shorthand
/// three other verbs share, so an engram somebody has open is retired in their
/// document and their session writes it down. The receipt says nothing about
/// it - a retirement's receipt is about what it retired - so what this pins is
/// the behaviour rather than the words.
///
/// Driven in a REVIEWING domain, where the room is over the actor's own
/// draft. A domain that takes changes directly goes through the same shared
/// body, and `a_retirement_in_a_direct_domain_composes_into_the_open_room`
/// pins that side.
#[tokio::test]
async fn a_retirement_of_an_open_draft_lands_in_the_document_too() {
    let (_tmp, engine, _scratch) = engine_fixture(true).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", Some("owner")).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "a person typed this").await;

    engine
        .retire_engram(&crystalline_service::params::RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "archived".to_string(),
            successor: None,
            valid_to: None,
        })
        .await
        .expect("the retirement lands");

    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("status: archived") && live.contains("a person typed this"),
        "the retirement composed with what they had typed: {live:?}"
    );
    assert!(
        engine
            .overlay_draft_at("eng", "owner", "alpha.md")
            .await
            .unwrap()
            .is_none(),
        "and nothing went behind the room into the row"
    );

    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let draft = engine
        .overlay_draft_at("eng", "owner", "alpha.md")
        .await
        .unwrap()
        .expect("which the room's own saver then writes down");
    assert!(
        draft.content.contains("status: archived") && draft.content.contains("a person typed this"),
        "{draft:?}"
    );
}

/// A supersession whose successor is open in a room that cannot save: the
/// reciprocal line goes to the successor's file, and a file that already
/// carries it does not get it twice, whatever the room's text says.
#[tokio::test]
async fn a_successor_open_in_a_room_that_cannot_save_gets_its_supersedes_line_once() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let beta = tmp.path().join("eng/beta.md");
    std::fs::write(&beta, format!("{BETA}\n- supersedes [[alpha]]\n")).unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let (_joined, _doc) = room_whose_save_failed(&sessions, "beta", None).await;

    engine
        .retire_engram(&crystalline_service::params::RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "superseded".to_string(),
            successor: Some("beta".to_string()),
            valid_to: None,
        })
        .await
        .expect("the retirement lands");
    let on_disk = std::fs::read_to_string(&beta).unwrap();
    assert_eq!(
        on_disk.matches("supersedes [[alpha]]").count(),
        1,
        "{on_disk:?}"
    );
}

/// A successor that already says it supersedes the retired engram is not
/// written at all: no second line, and no fresh `generated` stamp either.
#[tokio::test]
async fn a_successor_that_already_says_so_is_left_as_it_is() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let beta = tmp.path().join("eng/beta.md");
    let before = format!("{BETA}\n- supersedes [[alpha]]\n");
    std::fs::write(&beta, &before).unwrap();
    engine.sync(None).await.unwrap();

    engine
        .retire_engram(&crystalline_service::params::RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "superseded".to_string(),
            successor: Some("beta".to_string()),
            valid_to: None,
        })
        .await
        .expect("the retirement lands");
    assert_eq!(std::fs::read_to_string(&beta).unwrap(), before);
}

/// The other way round: the room that cannot save holds the supersedes line
/// and the file does not. The line goes to the file, where the edit lands.
#[tokio::test]
async fn a_successor_whose_unsaving_room_holds_the_line_gets_it_in_the_file() {
    use crystalline_service::collab::control::{self, Control};
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let beta = tmp.path().join("eng/beta.md");
    std::fs::write(&beta, BETA).unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "beta", None).await.unwrap();
    let doc = sync_client(&joined).await;
    replace_all(&joined, &doc, "no frontmatter\n\n- supersedes [[alpha]]\n").await;
    joined
        .session
        .handle_frame(joined.conn, &control::encode(&Control::Flush))
        .await;
    joined.session.tick_save(Instant::now()).await;
    assert!(
        !sessions
            .live_text("eng", "beta", None)
            .await
            .unwrap()
            .saving
    );

    engine
        .retire_engram(&crystalline_service::params::RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "superseded".to_string(),
            successor: Some("beta".to_string()),
            valid_to: None,
        })
        .await
        .expect("the retirement lands");
    let on_disk = std::fs::read_to_string(&beta).unwrap();
    assert_eq!(
        on_disk.matches("supersedes [[alpha]]").count(),
        1,
        "{on_disk:?}"
    );
}

/// **The brief's first test.** An agent that works in somebody's open document
/// is a peer in it: the room hears an awareness state carrying the agent's own
/// label, flagged as an agent so the strip can draw it as one.
///
/// The second half is what keeps the receipt honest. `present` is what the
/// agent is told about who is in there WITH it, so its own slot is never in
/// that list - not on the first call, which mints it, and not on the second,
/// which finds it already standing.
#[tokio::test]
async fn an_agent_action_broadcasts_an_awareness_state_flagged_agent() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    publish_name(&joined, &doc, "Grace Hopper").await;
    append_line(&joined, &doc, "a person typed this").await;
    drain(&mut joined.rx);

    let peer = AgentPeer {
        account: "ada".to_string(),
        label: "ada (agent: claude-code/2.0)".to_string(),
    };
    let receipt = engine
        .edit_engram_present(
            &append_edit("the agent added that", None),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            Some(&peer),
        )
        .await
        .expect("the edit lands");
    assert_eq!(receipt["landed"].as_str(), Some("live"), "{receipt}");
    assert_eq!(
        receipt["present"].as_array().map(Vec::len),
        Some(1),
        "the agent is told who is in there with it, not counted among them: {receipt}"
    );
    assert_eq!(
        receipt["present"][0].as_str(),
        Some("Grace Hopper"),
        "{receipt}"
    );

    let states = awareness_states(&mut joined.rx);
    let peer_state = states
        .iter()
        .find(|(_, json)| json.contains("ada (agent: claude-code/2.0)"))
        .expect("the room hears the agent arrive");
    assert!(
        peer_state.1.contains("\"agent\":true"),
        "flagged as an agent, so the strip draws it as one: {}",
        peer_state.1
    );

    // A second call inside the TTL finds the slot standing: the agent is one
    // peer however often it works, and it is still not in its own `present`.
    let receipt = engine
        .edit_engram_present(
            &append_edit("and then the agent added this", None),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            Some(&peer),
        )
        .await
        .expect("the second edit lands");
    assert_eq!(
        receipt["present"].as_array().map(Vec::len),
        Some(1),
        "still only the person: {receipt}"
    );
    let names = joined.session.participants(None).await;
    assert_eq!(
        names.iter().filter(|name| name.contains("(agent")).count(),
        1,
        "and the room holds one agent peer, not two: {names:?}"
    );

    // Somebody opening the page WHILE the agent is working gets the strip in
    // their greeting rather than from a broadcast they were not subscribed
    // for: the full awareness a join hands over carries the agent's slot like
    // anybody else's.
    let arriving = sessions.join("eng", "alpha", None).await.unwrap();
    let greeted: Vec<String> = messages_of(&arriving.greeting)
        .into_iter()
        .filter_map(|message| match message {
            Message::Awareness(update) => Some(update),
            _ => None,
        })
        .flat_map(|update| {
            update
                .clients
                .into_values()
                .map(|entry| entry.json.to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        greeted.iter().any(|json| {
            json.contains("ada (agent: claude-code/2.0)") && json.contains("\"agent\":true")
        }),
        "a tab opened mid-work is told who is in here: {greeted:?}"
    );
}

/// **The brief's second test.** The agent's slot is not a connection: a person
/// leaving the room does not take it with them, and nothing but the TTL does.
#[tokio::test]
async fn agent_presence_clears_after_the_ttl_and_survives_a_peer_leaving() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut mine = sessions.join("eng", "alpha", None).await.unwrap();
    let my_doc = sync_client(&mine).await;
    publish_name(&mine, &my_doc, "Grace Hopper").await;
    let hers = sessions.join("eng", "alpha", None).await.unwrap();
    let her_doc = sync_client(&hers).await;
    publish_name(&hers, &her_doc, "Ada Lovelace").await;

    let peer = AgentPeer {
        account: "ada".to_string(),
        label: "ada (agent)".to_string(),
    };
    engine
        .edit_engram_present(
            &append_edit("the agent added that", None),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            Some(&peer),
        )
        .await
        .expect("the edit lands");
    let agent_id = awareness_states(&mut mine.rx)
        .into_iter()
        .find(|(_, json)| json.contains("ada (agent)"))
        .expect("the room heard the agent arrive")
        .0;
    assert!(
        joined_names(&mine)
            .await
            .contains(&"ada (agent)".to_string()),
        "the agent stands in the room"
    );

    // A person leaves. Their own awareness state is nulled and the agent's is
    // not: the slot is nobody's connection.
    hers.session.remove_conn(hers.conn).await;
    let names = joined_names(&mine).await;
    assert!(
        names.contains(&"ada (agent)".to_string()) && !names.contains(&"Ada Lovelace".to_string()),
        "the peer left and the agent did not: {names:?}"
    );

    // A saver pass INSIDE the window is not what takes it: the TTL is a
    // threshold rather than "the next tick", and without this the whole test
    // passes with the TTL set to a millisecond.
    mine.session
        .tick_save(Instant::now() + Duration::from_secs(30))
        .await;
    assert!(
        joined_names(&mine)
            .await
            .contains(&"ada (agent)".to_string()),
        "half a minute in, the agent still stands"
    );

    // And the TTL is what does take it, on the saver's own pass.
    drain(&mut mine.rx);
    mine.session
        .tick_save(Instant::now() + Duration::from_secs(61))
        .await;
    assert!(
        !joined_names(&mine)
            .await
            .contains(&"ada (agent)".to_string()),
        "a minute of silence and the agent is gone"
    );
    let cleared = awareness_states(&mut mine.rx);
    assert!(
        cleared
            .iter()
            .any(|(id, json)| *id == agent_id && json == "null"),
        "and the room was told to drop the chip: {cleared:?}"
    );
}

/// **Ruling I1.** The strip is bounded the way the connection map is: past
/// [`MAX_PARTICIPANTS`] a touch is refused rather than growing the room, and
/// nobody is evicted to make space for it.
///
/// One caller with a fresh label on every call is not a hypothetical - a
/// modern-era peer that carries `_meta.clientInfo` on some calls and omits it
/// on others produces two labels for one account without anybody trying - and
/// the label is client-supplied, so "a room holds as many chips as a caller
/// cares to mint" is a growth vector as well as a mess. What must never be
/// paid for a chip is somebody's connection, and the edit itself is not a chip:
/// it lands whatever the strip decides.
#[tokio::test]
async fn agent_presence_is_bounded_the_way_the_connection_map_is() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mine = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&mine).await;
    publish_name(&mine, &doc, "Grace Hopper").await;

    let overflow = MAX_PARTICIPANTS + 4;
    let mut last = serde_json::Value::Null;
    for n in 0..overflow {
        let peer = AgentPeer {
            account: "ada".to_string(),
            label: format!("ada (agent: harness-{n})"),
        };
        last = engine
            .edit_engram_present(
                &append_edit(&format!("line {n}"), None),
                None,
                &crystalline_service::Scope::Unrestricted,
                None,
                Some(&peer),
            )
            .await
            .expect("the edit lands whatever the strip does with it");
    }

    let names = joined_names(&mine).await;
    assert_eq!(
        names.iter().filter(|name| name.contains("(agent")).count(),
        MAX_PARTICIPANTS,
        "the strip stops growing at the cap: {names:?}"
    );
    assert!(
        names.contains(&"Grace Hopper".to_string()),
        "and the person in the room was not evicted to make room: {names:?}"
    );
    assert!(
        !mine.session.is_empty().await,
        "their connection stands, which is what the cap is protecting"
    );

    // The write is not the chip. A refused slot refuses nothing else: the text
    // composed, the receipt says so, and - with no slot of its own to leave
    // out - the refused agent is told about everybody who is in there.
    assert_eq!(last["landed"].as_str(), Some("live"), "{last}");
    assert_eq!(
        last["present"].as_array().map(Vec::len),
        Some(MAX_PARTICIPANTS + 1),
        "a refused touch excludes nobody from the answer: {last}"
    );
    resync(&mine, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains(&format!("line {}", overflow - 1)),
        "and the last edit of all is in the document: {live:?}"
    );
}

/// **Ruling M3.** An agent whose published state was taken out from under it
/// puts itself back on its next action, rather than working invisibly for the
/// rest of the minute.
///
/// The id an agent publishes under is a hash of a label that is on screen, so
/// a connection in the room can publish under it; when that connection leaves,
/// the room nulls every id it sent, the agent's among them. The slot in the
/// map would still be standing, and a standing slot publishes nothing - so
/// without this the chip is gone until the TTL sweeps a slot nobody can see.
#[tokio::test]
async fn an_agent_whose_state_was_taken_away_publishes_itself_again() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut mine = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&mine).await;
    publish_name(&mine, &doc, "Grace Hopper").await;

    let peer = AgentPeer {
        account: "ada".to_string(),
        label: "ada (agent)".to_string(),
    };
    engine
        .edit_engram_present(
            &append_edit("the agent added that", None),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            Some(&peer),
        )
        .await
        .expect("the edit lands");
    let agent_id = awareness_states(&mut mine.rx)
        .into_iter()
        .find(|(_, json)| json.contains("ada (agent)"))
        .expect("the room heard the agent arrive")
        .0;

    // Another connection publishes under the agent's id and then leaves.
    let theirs = sessions.join("eng", "alpha", None).await.unwrap();
    theirs
        .session
        .handle_frame(theirs.conn, &claim_frame(agent_id))
        .await;
    theirs.session.remove_conn(theirs.conn).await;
    assert!(
        !joined_names(&mine)
            .await
            .contains(&"ada (agent)".to_string()),
        "the chip went with them, which is the harm"
    );

    drain(&mut mine.rx);
    engine
        .edit_engram_present(
            &append_edit("and the agent added this", None),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            Some(&peer),
        )
        .await
        .expect("it lands");
    assert!(
        joined_names(&mine)
            .await
            .contains(&"ada (agent)".to_string()),
        "and the agent's next action puts it back"
    );
    let states = awareness_states(&mut mine.rx);
    assert!(
        states
            .iter()
            .any(|(id, json)| *id == agent_id && json.contains("\"agent\":true")),
        "the room was told, rather than the map quietly disagreeing with it: {states:?}"
    );
}

// --- the wholesale overwrite of a live document, refused ---------------------
//
// An edit COMPOSES into somebody's open document; a `write_engram` carrying
// overwrite=true REPLACES it, and replacing work a person can see on screen
// and has not saved is the one thing an agent may not do quietly. So the call
// is refused for every client, one that can elicit included, and the refusal
// names who is in there and the verb to use instead.
//
// Driven over the wire rather than through the engine, because the refusal is
// the MCP layer's. The fixtures below are copied from `mcp_modern_era.rs` for
// the reason the module header already gives - integration test crates share
// no helpers beyond `tests/support`, which holds the server half
// (`support::mcp_session`).

/// The revision these rounds are served at.
const ERA: &str = "2026-07-28";

/// A stdio MCP connection to a server over one engine.
struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    /// The server half, waited on once the first message that chooses the
    /// lifecycle was answered (never a `server/discover` on its own; see
    /// `support::mcp_session`) and kept alive for the rest of the conversation.
    server: ServedSession<McpServer>,
}

impl Wire {
    /// A connection to a server over `engine`, not yet opened.
    fn to(engine: Arc<Engine>) -> Wire {
        let (client_io, server_io) = tokio::io::duplex(1 << 18);
        let server = ServedSession::spawn(McpServer::new(engine), server_io);
        let (read, write) = tokio::io::split(client_io);
        Wire {
            write,
            lines: tokio::io::BufReader::new(read).lines(),
            server,
        }
    }

    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    async fn recv(&mut self) -> Option<Value> {
        match tokio::time::timeout(Duration::from_millis(2000), self.lines.next_line()).await {
            Ok(Ok(Some(line))) => Some(serde_json::from_str(&line).unwrap()),
            _ => None,
        }
    }

    /// Send the session opener and read its answer.
    async fn open(&mut self, message: Value) -> Value {
        self.exchange(message, "the opener was answered").await
    }

    /// Send a request on an open connection and read its answer.
    async fn call(&mut self, message: Value) -> Value {
        self.exchange(message, "the request was answered").await
    }

    /// Send `message`, read its answer, and collect the running service the
    /// start-up loop hands back once `message` chose the lifecycle.
    async fn exchange(&mut self, message: Value, answered: &str) -> Value {
        let chooses = !self.server.is_started() && chooses_lifecycle(&message);
        self.send(message).await;
        let answer = self.recv().await.expect(answered);
        if chooses {
            self.server.started().await;
        }
        answer
    }
}

fn request(id: u32, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A modern request from a client that cannot put a question to anybody.
fn modern(id: u32, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "collab-agent-test", "version": "9.9.9" },
    });
    request(id, method, params)
}

/// A modern request from a client that can.
fn eliciting(id: u32, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": { "form": {} } },
        "io.modelcontextprotocol/clientInfo": { "name": "collab-agent-test", "version": "9.9.9" },
    });
    request(id, method, params)
}

/// The engine payload a tool result carries, as JSON.
fn payload_of(answer: &Value) -> Value {
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// The refusal text a tool result carries.
fn refusal_of(answer: &Value) -> String {
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The body every wholesale overwrite below tries to land over the document.
const REPLACEMENT: &str = "A wholesale replacement.";

/// One `write_engram` call replacing `alpha` wholesale.
fn overwrite_alpha() -> Value {
    json!({
        "name": "write_engram",
        "arguments": {
            "domain": "eng",
            "title": "Alpha",
            "content": REPLACEMENT,
            "overwrite": true,
        },
    })
}

/// The same capture WITHOUT `overwrite`: the shape an agent sends when it does
/// not know the permalink is taken.
fn capture_alpha() -> Value {
    json!({
        "name": "write_engram",
        "arguments": {
            "domain": "eng",
            "title": "Alpha",
            "content": REPLACEMENT,
        },
    })
}

/// A room over `alpha` with one person in it who has typed a line nobody has
/// saved yet, and an MCP connection to the same engine.
///
/// The sessions registry is handed back because the engine holds it weakly:
/// dropping it here would close every room before the call under test runs.
async fn a_person_typing_in_alpha() -> (
    tempfile::TempDir,
    Arc<Engine>,
    Arc<CollabSessions>,
    Joined,
    Doc,
    crate::support::ScratchStateDir,
) {
    let (tmp, engine, scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    publish_name(&joined, &doc, "Jordi").await;
    append_line(&joined, &doc, "typed but never saved").await;
    (tmp, engine, sessions, joined, doc, scratch)
}

/// A wholesale overwrite of a document somebody has open is refused for a
/// client that can elicit too: nobody is asked, the refusal names who is in
/// there and the verb to use instead, and nothing is replaced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_wholesale_overwrite_into_a_live_document_is_refused_naming_who_is_present() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    // Opened through the era's own onboarding call, so the call below runs on
    // a connection that never handshook.
    let discovered = wire.open(modern(1, "server/discover", json!({}))).await;
    assert!(
        discovered["result"]["instructions"].is_string(),
        "the connection is open at the era: {discovered}"
    );

    let refused = wire
        .call(eliciting(2, "tools/call", overwrite_alpha()))
        .await;
    assert_ne!(
        refused["result"]["resultType"],
        json!("input_required"),
        "nothing is asked: {refused}"
    );
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "the call is refused rather than written: {refused}"
    );
    let text = refusal_of(&refused);
    assert_eq!(
        text,
        "The engram is open in the editor right now (present: Jordi), so a full replace was \
         refused and nothing was written. Use edit_engram for a targeted change, or replace it \
         after they close it.",
        "it says what did not happen and what to do instead: {text}"
    );
    assert!(
        text.contains("Jordi"),
        "and who is in the document it would have replaced: {text}"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the file is untouched: {on_disk:?}");
    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("typed but never saved") && !live.contains(REPLACEMENT),
        "and their unsaved line still stands: {live:?}"
    );
}

/// A capture that did not pass `overwrite` into a live document is refused
/// the same way: the permalink is taken and a room is open over it, and no
/// question is put about either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_capture_into_a_live_document_is_refused_too() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    let refused = wire.open(eliciting(1, "tools/call", capture_alpha())).await;
    assert_ne!(
        refused["result"]["resultType"],
        json!("input_required"),
        "nothing is asked: {refused}"
    );
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    let text = refusal_of(&refused);
    assert!(
        text.contains("nothing was written") && text.contains("Jordi"),
        "the refusal names who is in there: {text}"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the file is untouched: {on_disk:?}");
    resync(&joined, &doc).await;
    assert!(
        !client_text(&doc).contains(REPLACEMENT),
        "and the document is untouched"
    );
}

/// An overwrite the engine is handed directly (no MCP call reaches it while a
/// room is open) lands the replacement in the document rather than over it:
/// the room keeps the document it has, the person's session is still what
/// writes it down, and the file is not touched behind their back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engine_overwrite_replaces_the_text_and_keeps_history() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;

    let receipt = engine
        .write_engram_present(
            &wholesale_capture("Alpha", REPLACEMENT, true),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            None,
        )
        .await
        .expect("the capture lands");
    assert_eq!(
        receipt["landed"],
        json!("live"),
        "and says where it went: {receipt}"
    );
    assert!(
        receipt["present"].is_array(),
        "with who is in there: {receipt}"
    );

    // The client doc is the one it was: an incremental resync over the state
    // vector it already holds brings it the replacement, which a room that had
    // thrown its document away could not do.
    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains(REPLACEMENT),
        "the replacement is in the document they are looking at: {live:?}"
    );
    assert!(
        !live.contains("typed but never saved"),
        "and it is a replacement, so their line went with the rest: {live:?}"
    );

    // Nothing was written behind the room's back; the room's own saver is what
    // makes the replacement durable, exactly as it is for a typed line.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the file waits for the save: {on_disk:?}");
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains(REPLACEMENT),
        "and then the file carries it: {on_disk:?}"
    );
}

/// **The brief's third test.** A targeted change is never refused. It
/// composes with what the person typed instead of replacing it, which is the
/// whole reason the refusal exists for one verb and not the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_targeted_edit_never_asks() {
    let (_tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    let done = wire
        .open(eliciting(
            1,
            "tools/call",
            json!({
                "name": "edit_engram",
                "arguments": {
                    "domain": "eng",
                    "identifier": "alpha",
                    "operation": "append",
                    "content": "and the agent added that",
                },
            }),
        ))
        .await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "a targeted edit asks nobody anything: {done}"
    );
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "it simply lands: {done}"
    );
    assert_eq!(
        payload_of(&done)["landed"],
        json!("live"),
        "in the open document: {done}"
    );

    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("typed but never saved") && live.contains("and the agent added that"),
        "both lines stand: {live:?}"
    );
}

/// **The brief's fourth test.** A client without elicitation is refused, not
/// served the replacement, exactly as one with it is: an overwrite of an open
/// document would be somebody's unsaved work gone with no way to know.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_without_elicitation_is_refused_not_stomped() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    let refused = wire.open(modern(1, "tools/call", overwrite_alpha())).await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "the call is refused rather than written: {refused}"
    );
    let text = refusal_of(&refused);
    assert!(
        text.contains("nothing was written"),
        "and says what did not happen: {text}"
    );
    assert!(
        text.contains("edit_engram"),
        "and what to do instead: {text}"
    );
    assert!(
        text.contains("Jordi"),
        "and who is in the document it would have replaced: {text}"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the file is untouched: {on_disk:?}");
    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("typed but never saved") && !live.contains(REPLACEMENT),
        "and their unsaved line still stands: {live:?}"
    );
}

/// **Ruling from review.** The write's live arm stands ahead of the OVERLAY arm
/// as well as the file one, which is the ordering the edit's own arm had to be
/// corrected into: a domain in review mode is where most rooms are, and an arm
/// behind the overlay arm is unreachable there because that arm returns.
///
/// Driven through the engine rather than the wire: the asking is the MCP
/// layer's, the arm order is the engine's, and this is about the arm order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wholesale_overwrite_in_a_reviewing_domain_lands_in_the_draft_room() {
    let (tmp, engine, _scratch) = engine_fixture(true).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    // The machine owner is who a local agent acts as, so the owner's room is
    // the agent's own overlay document.
    let mine = sessions.join("eng", "alpha", Some("owner")).await.unwrap();
    let doc = sync_client(&mine).await;
    append_line(&mine, &doc, "the owner typed this").await;

    let receipt = engine
        .write_engram_present(
            &WriteParams {
                domain: "eng".to_string(),
                title: "Alpha".to_string(),
                content: REPLACEMENT.to_string(),
                folder: None,
                engram_type: None,
                tags: Vec::new(),
                status: None,
                metadata: None,
                overwrite: true,
                share_link: None,
                model: None,
            },
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            None,
        )
        .await
        .expect("the capture lands");
    assert_eq!(
        receipt["landed"].as_str(),
        Some("live"),
        "it went into the draft's room rather than past it: {receipt}"
    );
    assert_eq!(
        receipt["draft"].as_bool(),
        Some(true),
        "and it is still a draft of the owner's: {receipt}"
    );

    resync(&mine, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains(REPLACEMENT) && !live.contains("the owner typed this"),
        "the document they are looking at holds the replacement: {live:?}"
    );
    // The team's own folder never hears about a draft, room or no room.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the reviewed file stands: {on_disk:?}");
}

/// Issue 112, spec item 7: the check an overwrite is refused on and the write
/// it guards resolve ONE landing. Bob works inside ada's draft of a page
/// whose file name is not its title's slug, and ada has that page open. The
/// preview has to screen the file the write lands in, not the slug path the
/// title spells: screening `beta.md` would refuse the join, answer `None`,
/// and the write would then compose into ada's open page unrefused.
#[tokio::test]
async fn a_joined_overwrite_of_a_title_named_page_screens_the_room_the_write_lands_in() {
    let (tmp, engine, _scratch) = engine_fixture(true).await;
    std::fs::write(
        tmp.path().join("eng/Beta Notes.md"),
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Beta\n\nA rule about beta.\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let hers = sessions.join("eng", "beta", Some("ada")).await.unwrap();
    let doc = sync_client(&hers).await;
    append_line(&hers, &doc, "ada typed this").await;

    let bob = crystalline_service::Scope::User {
        account: "bob".to_string(),
        admin: false,
    };
    let join = crystalline_service::Join {
        account: "bob".to_string(),
        holder: crystalline_service::Holder::Process(1),
        domain: "eng".to_string(),
        path: "Beta Notes.md".to_string(),
        owner: "ada".to_string(),
        expires_at: None,
    };
    let capture = wholesale_capture("Beta", REPLACEMENT, true);
    let target = engine
        .live_write_target(&capture, &bob, Some(&join), None)
        .await
        .expect("the preview finds ada's open page, so the overwrite is refused");
    assert_eq!(target.permalink, "beta");

    let receipt = engine
        .write_engram_present(&capture, None, &bob, Some(&join), None)
        .await
        .expect("the capture lands");
    assert_eq!(receipt["landed"].as_str(), Some("live"), "{receipt}");
    assert_eq!(receipt["path"].as_str(), Some("Beta Notes.md"), "{receipt}");
    assert_eq!(
        receipt["joined"].as_str(),
        Some("landed in ada's draft"),
        "{receipt}"
    );
}

/// **Ruling from review.** The refusal is the same one at the legacy era,
/// which is what nearly every client in the field still speaks, however
/// loudly the client declares elicitation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_is_refused_the_wholesale_overwrite_too() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-collab-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(
        handshake["result"]["protocolVersion"],
        json!("2025-11-25"),
        "the session is the legacy one: {handshake}"
    );

    let refused = wire.call(request(2, "tools/call", overwrite_alpha())).await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "a legacy peer is refused rather than served the replacement: {refused}"
    );
    let text = refusal_of(&refused);
    assert!(
        text.contains("nothing was written") && text.contains("Jordi"),
        "in the same words, naming who is in there: {text}"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the file is untouched: {on_disk:?}");
    resync(&joined, &doc).await;
    assert!(
        client_text(&doc).contains("typed but never saved"),
        "and their unsaved line still stands"
    );
}

// --- the locks, the precondition and the tails -----------------------------
//
// Fix round 1. Three rulings, and they are all about what the write's live arm
// may do rather than about what it lands: it may not hold a file lock while it
// reaches into a room, it may not decide "this is a replacement" from a check
// somebody else made, and it may not drop a tail that keeps the instance
// truthful without saying which tail belongs to whom.

/// One engram written straight into a helper for the capture tests below.
fn wholesale_capture(title: &str, content: &str, overwrite: bool) -> WriteParams {
    WriteParams {
        domain: "eng".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite,
        share_link: None,
        model: None,
    }
}

/// The name a line declares a function under, if it declares one.
///
/// Shared by [`no_engine_function_composes_into_a_room_under_a_file_write_lock`]
/// and [`declared_fn_re_attributes_lines_after_a_nested_fn`], the test that
/// pins the one acknowledged limitation this exact shape has: it runs on
/// every line before any comment skip, and it never resets - so a nested `fn`
/// (this function is itself that shape, nested inside the guard until this
/// round moved it out) re-points every later line at the nested name for the
/// rest of the enclosing body. Neither engine function this guard
/// watches has that shape today, which is the whole of why the guard is
/// still sound; the other test is what keeps that a checked fact rather than
/// an assumption.
fn declared_fn(line: &str) -> Option<&str> {
    let rest = line.trim_start();
    let rest = rest
        .strip_prefix("pub(crate) ")
        .or_else(|| rest.strip_prefix("pub(super) "))
        .or_else(|| rest.strip_prefix("pub "))
        .unwrap_or(rest);
    let rest = rest.strip_prefix("async ").unwrap_or(rest);
    let rest = rest.strip_prefix("fn ")?;
    let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    Some(&rest[..end])
}

/// **Ruling C1.** No engine function may make a room call - one that reaches
/// into a [`CollabSessions`] room - while it holds a per-path file write
/// lock.
///
/// The two locks are taken in the opposite order by the room's own saver - it
/// holds the session state lock across `Engine::save_engram`, which takes the
/// file lock for the same path - so a verb that holds the file lock and then
/// waits for the state lock closes a cycle neither side can break. Nothing
/// times out: the file lock is held for ever, so every later write, edit, save
/// or delete of that engram hangs and the person's unsaved work never lands.
///
/// **Seven needles, not one.** `apply_text` is the write; `has_live_room`,
/// `live_text`, `participants` and `touch_agent_presence` are reads, and
/// every one of the five reaches `state.lock()` on the very same session
/// state lock the room's saver holds across the file lock - so a future
/// function that took the file lock and then only READ the room, never
/// wrote it, would wedge exactly the way C1 did, with a green suite if only
/// the write were watched.
///
/// The other two are the disposals, `dispose_domain` and
/// `dispose_domain_discarding`, and they are the sharpest of the seven:
/// `dispose_domain_discarding` takes the registry lock and then calls
/// `final_save` on each victim, which holds the session state lock across
/// `Engine::save_engram` - the cycle verbatim. Neither of their callers
/// (`unregister_domain` and `leave_review_mode`) holds a file lock today, and
/// `leave_review_mode` is precisely the function that could grow one later: it
/// already writes folded drafts into the folder. Watching them costs nothing
/// and is what keeps that a checked fact.
///
/// A source scan rather than a behavioural assertion, and for the reason the
/// other guards in this repo are source scans: the failure is a lock taken one
/// line too early, which no request can be written to provoke on demand - the
/// window is a scheduling accident, so a test that drives the two at each
/// other proves nothing when it passes. What CAN be checked exactly is the
/// discipline: inside one function, every room call comes before the lock or
/// not at all. `apply_source_edit_staged` is the shape this describes - its
/// live arm returns above the arms that take locks - and the write's arm now
/// stands the same way.
#[test]
fn no_engine_function_composes_into_a_room_under_a_file_write_lock() {
    // The engine is a directory of section files: every one of them is
    // scanned, each on its own, so a function never inherits the last name
    // of the file before it.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../engine/src/engine");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    assert!(
        files.len() > 10,
        "the scan found only {} files under {}, so it is not looking where it thinks it is",
        files.len(),
        dir.display()
    );
    let mut text = String::new();
    let mut file_starts = std::collections::HashSet::new();
    for file in &files {
        file_starts.insert(text.lines().count());
        text.push_str(&std::fs::read_to_string(file).unwrap());
        text.push('\n');
    }

    /// Every way an engine function reaches into a room, all of which take
    /// the same session state lock the room's saver holds across the file
    /// lock: the writes (an engram's, a reflection's merge), the reads and
    /// the disposals, and the switch-off's close of a domain's reflection
    /// rooms, which takes the registry and then each room's state.
    const ROOM_ENTRY_NEEDLES: [&str; 15] = [
        ".apply_text(",
        ".apply_doc_text(",
        ".absorb_doc_change(",
        ".has_live_room(",
        ".has_live_doc(",
        ".live_text(",
        ".participants(",
        ".touch_agent_presence(",
        ".dispose_domain(",
        ".dispose_domain_discarding(",
        ".close_docs(",
        ".close_reflection_rooms(",
        ".close_reflection_rooms_if_switching_off(",
        ".compose_while_saving(",
        ".has_engram_rooms_in(",
    ];

    /// Every way an engine function takes a per-path write lock: the file's
    /// own, and a draft's mirror path through `Engine::draft_lock`, which
    /// wraps the same map. Both are per-path mutexes held across a
    /// read-modify-write, so both close the same cycle against the room's
    /// saver; watching only the spelling that says `write_lock` would leave
    /// every overlay arm unwatched. `lock_file` is the reflection pages'
    /// spelling of the same lock, `lock_files` takes several of them, and
    /// `write_checked` takes a file's lock and hands it back for its caller
    /// to hold across the reindex.
    const LOCK_NEEDLES: [&str; 5] = [
        ".write_lock(",
        ".draft_lock(",
        ".lock_file(",
        ".lock_files(",
        ".write_checked(",
    ];

    // Per function, the first line that takes a per-path write lock and the
    // first that makes a room call. A comment mentioning either is not a call,
    // so the scan skips the comment lines the arms are thick with.
    let mut current = "<file scope>".to_string();
    let mut locked: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // A function that releases its lock by name (`drop(_lock)`) before it
    // reaches a room is safe; one that composes while holding it is not.
    let mut offending: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut composed: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, line) in text.lines().enumerate() {
        if file_starts.contains(&i) {
            current = "<file scope>".to_string();
        }
        if let Some(name) = declared_fn(line) {
            current = name.to_string();
        }
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with("///") {
            continue;
        }
        if LOCK_NEEDLES.iter().any(|needle| code.contains(needle)) {
            locked.entry(current.clone()).or_insert(i);
        }
        if code.starts_with("drop(_lock)") {
            locked.remove(&current);
        }
        if ROOM_ENTRY_NEEDLES
            .iter()
            .any(|needle| code.contains(needle))
        {
            // The LAST room call, not the first: a function is safe only if
            // EVERY room call inside it comes before the lock, and the first
            // occurrence alone would go blind the moment a cheap read (a
            // `has_live_room` or `live_text` probe, which is the idiomatic
            // shape - decide first, then take the lock) sits above the lock
            // while the write moves below it. The comparison below is then
            // "any needle after any lock", which is what the discipline
            // actually requires.
            composed.insert(current.clone(), i);
            if locked.contains_key(&current) {
                offending.insert(current.clone());
            }
        }
    }

    let mut offenders: Vec<&String> = composed
        .keys()
        .filter(|name| {
            offending.contains(*name)
                || matches!(locked.get(*name), Some(lock) if lock < &composed[*name])
        })
        .collect();
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "these engine functions take a file write lock and then compose into a room while holding \
         it, which deadlocks against the room's saver taking the two in the other order: \
         {offenders:?}"
    );
}

/// **The acknowledged cost of the scan above, pinned rather than only
/// described.** [`declared_fn`] runs on every line before any comment skip
/// and never resets when a nested `fn` ends, so a needle after a nested `fn`
/// is attributed to the NESTED name for the rest of the enclosing body, not
/// to the function it is textually still inside. The guard's own real
/// target, `apply_source_edit_staged` and the write's live arm, has no
/// nested `fn` today, so this is a synthetic case rather than a finding
/// against the engine module; it is what stands between "the guard is sound" being
/// a checked fact and an assumption a nested `fn` added later could quietly
/// break.
#[test]
fn declared_fn_re_attributes_lines_after_a_nested_fn() {
    let source = [
        "fn outer() {",
        "    fn inner() {",
        "        let x = 1;",
        "    }",
        "    room.apply_text(x);",
        "}",
    ];
    let mut current = "<file scope>".to_string();
    let mut owner_of_needle = None;
    for line in source {
        if let Some(name) = declared_fn(line) {
            current = name.to_string();
        }
        if line.trim_start().contains(".apply_text(") {
            owner_of_needle = Some(current.clone());
        }
    }
    assert_eq!(
        owner_of_needle,
        Some("inner".to_string()),
        "the needle sits inside `outer`, past `inner`'s closing brace, but the scan's \
         never-reset attribution names the nested function instead - this is the exact \
         limitation the doc comment above describes, pinned so a fix to the scan (or a new \
         nested fn in the real target that finally exercises it) has to touch this test too"
    );
}

/// **Ruling C1, the behaviour.** A save and a wholesale overwrite driven at one
/// another both finish.
///
/// Bounded rather than exact: the deadlock the guard above pins is a race, so
/// this cannot be staged to fail on demand - what it can do is refuse to hang.
/// A wedged pair never returns at all, so a generous timeout around both is a
/// true statement about the pair rather than a stopwatch on either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_save_and_a_wholesale_overwrite_driven_at_each_other_both_finish() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "a person typed this").await;

    let saving = {
        let session = joined.session.clone();
        tokio::spawn(async move {
            for _ in 0..50 {
                session
                    .tick_save(Instant::now() + Duration::from_secs(60))
                    .await;
                tokio::task::yield_now().await;
            }
        })
    };
    let writing = {
        let engine = engine.clone();
        tokio::spawn(async move {
            for i in 0..50 {
                engine
                    .write_engram_present(
                        &wholesale_capture("Alpha", &format!("Replacement {i}."), true),
                        None,
                        &crystalline_service::Scope::Unrestricted,
                        None,
                        None,
                    )
                    .await
                    .expect("the capture lands, live or on disk");
                tokio::task::yield_now().await;
            }
        })
    };

    let both = async {
        saving.await.unwrap();
        writing.await.unwrap();
    };
    tokio::time::timeout(Duration::from_secs(30), both)
        .await
        .expect("a save and a wholesale overwrite must not wedge each other");
}

/// **Ruling I1.** The live arm is for a REPLACEMENT, and it says so itself
/// rather than inheriting the collision check's word for it.
///
/// The window: an engram is deleted while its room is still open - the room
/// only learns of that on its next save - so a capture at that permalink finds
/// the permalink free, passes the collision check without `overwrite` and,
/// under an arm that trusted that check, morphs the document somebody is
/// looking at into a brand new engram with a receipt saying "created".
///
/// What happens instead is the ordinary create: the file is written beside the
/// room, exactly as a capture at a free permalink always has, and the person's
/// document is left alone. That is the truthful outcome for a call that never
/// asked to replace anything - and the room, which is a room over an engram
/// that was deleted, settles that on its own next save.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_without_overwrite_never_morphs_an_open_document() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed but never saved").await;

    // The row and the file go while the room stands.
    engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "alpha".to_string(),
            domain: "eng".to_string(),
            expected_checksum: None,
        })
        .await
        .expect("the engram is deleted under the open room");

    let receipt = engine
        .write_engram_present(
            &wholesale_capture("Alpha", "A brand new engram.", false),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            None,
        )
        .await
        .expect("a capture at a free permalink is an ordinary create");
    assert!(
        receipt["landed"].is_null(),
        "a create is not a live landing: {receipt}"
    );
    assert_eq!(
        receipt["action"].as_str(),
        Some("created"),
        "and says what it was: {receipt}"
    );

    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("typed but never saved") && !live.contains("A brand new engram."),
        "the open document was not morphed by a capture that never asked to replace it: {live:?}"
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk.contains("A brand new engram."),
        "the create landed as an ordinary file write: {on_disk:?}"
    );
}

/// **Ruling I2.** A virtual domain's MANIFEST replaced inside its room reaches
/// the routing cache when the room saves - the tail the live arm leaves to the
/// saver, pinned end to end so "the saver owes it" is a fact rather than a
/// claim in a comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_virtual_manifest_replaced_in_its_room_reaches_the_routing_cache() {
    let scratch = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::virtual_domain());
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
    engine
        .write_engram(&wholesale_capture(
            "MANIFEST",
            "# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for the old question\n",
            false,
        ))
        .await
        .expect("the manifest lands");
    engine.refresh_routing_cache().await;
    assert!(
        engine
            .virtual_routing_bullets()
            .await
            .get("eng")
            .is_some_and(|bullets| bullets.iter().any(|b| b.contains("the old question"))),
        "the routing starts where the manifest says"
    );

    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "manifest", None).await.unwrap();
    let _doc = sync_client(&joined).await;

    engine
        .write_engram_present(
            &wholesale_capture(
                "MANIFEST",
                "# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for the new question\n",
                true,
            ),
            None,
            &crystalline_service::Scope::Unrestricted,
            None,
            None,
        )
        .await
        .expect("the replacement lands in the room");

    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let bullets = engine.virtual_routing_bullets().await;
    assert!(
        bullets
            .get("eng")
            .is_some_and(|bullets| bullets.iter().any(|b| b.contains("the new question"))),
        "the saved manifest is what the domain routes on: {bullets:?}"
    );
    drop(scratch);
}

/// Item 4: the preview reads the domain's id and never registers it. A domain
/// the index has never seen has no document anybody can have open, so the
/// answer is `None`, and asking left no row behind.
async fn a_live_preview_for_a_domain_with_no_row_answers_none_and_registers_nothing_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let cold = tmp.path().join("cold");
    std::fs::create_dir_all(&cold).unwrap();
    std::fs::write(
        cold.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: cold\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# cold\n\n## Scope\n\n- Everything about cold\n\n## When to Use\n\n- Route here for cold questions\n",
    )
    .unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("cold".to_string(), DomainEntry::file(cold));
    // No sync: the domain is registered and the index has never seen it.
    let engine = Arc::new(
        Engine::new(store.clone(), cfg, None, None).with_state_dir(tmp.path().join("state")),
    );
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let capture = WriteParams {
        domain: "cold".to_string(),
        title: "Gamma".to_string(),
        content: "x".to_string(),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: true,
        share_link: None,
        model: None,
    };
    let target = engine
        .live_write_target(
            &capture,
            &crystalline_service::Scope::Unrestricted,
            None,
            None,
        )
        .await;
    assert!(target.is_none(), "no row, so no open document");
    let row = { store.lock().await.domain_id("cold").await.unwrap() };
    assert_eq!(row, None, "a preview registers nothing");
}
both_backends!(
    a_live_preview_for_a_domain_with_no_row_answers_none_and_registers_nothing,
    a_live_preview_for_a_domain_with_no_row_answers_none_and_registers_nothing_body
);

/// A draft never starts from the body alone. A file domain's index row
/// holds an engram's body only, so with the file gone from this disk
/// (deleted by hand before the next sync) there is no whole engram to start
/// from: the edit is refused with the way out, and no draft is written.
/// Before, the draft kept the body and lost the frontmatter (title, type,
/// tags, permalink).
#[tokio::test]
async fn a_draft_never_starts_from_the_body_alone() {
    let (tmp, engine, _scratch) = engine_fixture(true).await;
    std::fs::remove_file(tmp.path().join("eng/alpha.md")).unwrap();
    let ada = crystalline_service::Scope::User {
        account: "ada".into(),
        admin: false,
    };
    let edited = engine
        .edit_engram_as(
            &EditParams {
                identifier: "alpha".into(),
                domain: "eng".into(),
                operation: "append".into(),
                content: Some("ada adds a line".into()),
                ..Default::default()
            },
            None,
            &ada,
        )
        .await;
    let store = engine.store();
    let store = store.lock().await;
    let did = store.domain_id("eng").await.unwrap().unwrap();
    let draft = store.overlay_entry(did, "ada", "alpha.md").await.unwrap();
    let err = edited.expect_err("no draft starts from the body alone");
    assert!(
        matches!(err, crystalline_service::EngineError::NotFound(_))
            && err.to_string().contains("is not on this disk"),
        "{err}"
    );
    assert!(draft.is_none(), "{draft:?}");
}

/// The `Closed` control the room's sockets were sent, skipping every other
/// frame (the room's last save announces itself first).
async fn closed_control(
    rx: &mut broadcast::Receiver<Frame>,
) -> crystalline_service::collab::control::Control {
    use crystalline_service::collab::control::{self, Control};
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a control frame arrives in time")
            .expect("the broadcast channel stays open");
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

/// An agent's `move_engram` over MCP that gives an open engram a new
/// permalink closes the person's room with `moved` and the new address,
/// after the room saved what they typed. Before, the call was refused while
/// the engram was open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_move_over_mcp_closes_the_persons_room_with_moved() {
    use crystalline_service::collab::control::Control;
    let (tmp, engine, _sessions, mut joined, _doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());
    wire.open(modern(1, "server/discover", json!({}))).await;
    let answer = wire
        .call(modern(
            2,
            "tools/call",
            json!({
                "name": "move_engram",
                "arguments": {
                    "identifier": "alpha",
                    "domain": "eng",
                    "destination": "guides/alpha.md",
                    "permalink": "path",
                },
            }),
        ))
        .await;
    assert_ne!(answer["result"]["isError"], json!(true), "{answer}");
    assert_eq!(
        closed_control(&mut joined.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: Some("guides/alpha.md".into()),
            permalink: Some("guides/alpha".into()),
        }
    );
    let moved = std::fs::read_to_string(tmp.path().join("eng/guides/alpha.md")).unwrap();
    assert!(moved.contains("typed but never saved"), "{moved}");
}

/// A permalink move of an open engram on either backend: on Postgres the
/// room's last save and the move's rows both pass the file domain's host-lock
/// fence, and the room is closed with `moved` after its save landed.
async fn a_move_closes_the_room_after_its_save_on_either_backend_body(
    store: Arc<Mutex<dyn Store>>,
) {
    use crystalline_service::collab::control::Control;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    write_manifest(&dir, "eng");
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir.clone()));
    // An instance id, as a daemon has: on Postgres the walk then takes the
    // file domain's host lock, and every write after it is fenced by it.
    let engine = Arc::new(
        Engine::new(store, cfg, None, None)
            .with_state_dir(tmp.path().join("state"))
            .with_instance_id("instance-a".to_string()),
    );
    engine.sync(None).await.unwrap();
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    append_line(&joined, &doc, "typed on either backend").await;
    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "alpha".into(),
                domain: "eng".into(),
                destination: "moved/alpha.md".into(),
                destination_domain: None,
                permalink: Some("path".into()),
                update_links: None,
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        closed_control(&mut joined.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: Some("moved/alpha.md".into()),
            permalink: Some("moved/alpha".into()),
        }
    );
    let moved = std::fs::read_to_string(dir.join("moved/alpha.md")).unwrap();
    assert!(moved.contains("typed on either backend"), "{moved}");
    assert!(!dir.join("alpha.md").exists());
}
both_backends!(
    a_move_closes_the_room_after_its_save_on_either_backend,
    a_move_closes_the_room_after_its_save_on_either_backend_body
);

/// A review-mode move that names a new permalink readdresses the mover's own
/// draft: their room over it is saved, closed with `moved` and the new
/// address, and nobody else's draft room of the page is touched.
#[tokio::test]
async fn a_readdressing_draft_move_closes_the_movers_own_draft_room() {
    use crystalline_service::collab::control::Control;
    let (_tmp, engine, _scratch) = engine_fixture(true).await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let mut mine = sessions.join("eng", "alpha", Some("owner")).await.unwrap();
    let mine_doc = sync_client(&mine).await;
    append_line(&mine, &mine_doc, "the owner typed this").await;
    let hers = sessions.join("eng", "alpha", Some("alice")).await.unwrap();
    let _hers_doc = sync_client(&hers).await;
    let receipt = engine
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
    assert_eq!(receipt["draft"], json!(true), "{receipt}");
    assert_eq!(
        closed_control(&mut mine.rx).await,
        Control::Closed {
            reason: "moved".into(),
            domain: None,
            path: Some("alpha.md".into()),
            permalink: Some("renamed".into()),
        }
    );
    assert!(!hers.session.is_disposed(), "alice's draft room stays open");
    let moved = sessions
        .join("eng", "renamed", Some("owner"))
        .await
        .unwrap();
    let (live, _) = moved.session.snapshot().await;
    assert!(live.contains("the owner typed this"), "{live}");
}

// --- an agent and a person in the same table row ----------------------------
//
// A person types in one cell of a row, an agent changes another cell of the
// same row. Both changes end in the room and on disk, and neither side's
// change is moved, lost or raised as a conflict.

/// `alpha` with a table body, the row under test `| 1 | 2 |`.
const TABLE_ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\n| a | b |\n| - | - |\n| 1 | 2 |\n";

/// [`engine_fixture`] with `alpha` holding [`TABLE_ALPHA`].
async fn table_fixture() -> (
    tempfile::TempDir,
    Arc<Engine>,
    crate::support::ScratchStateDir,
) {
    let (tmp, engine, scratch) = engine_fixture(false).await;
    std::fs::write(tmp.path().join("eng/alpha.md"), TABLE_ALPHA).unwrap();
    engine.sync(None).await.unwrap();
    (tmp, engine, scratch)
}

/// Type `typed` into the client doc right after `after` and return the
/// update WITHOUT sending it, the way a keystroke is still in flight while
/// another change reaches the room.
fn type_after_unsent(doc: &Doc, after: &str, typed: &str) -> Vec<u8> {
    let text = doc.get_or_insert_text("content");
    let mut txn = doc.transact_mut();
    let current = text.get_string(&txn);
    let at = current.find(after).expect("the anchor text is in the doc") + after.len();
    let index = current[..at].encode_utf16().count() as u32;
    text.insert(&mut txn, index, typed);
    txn.encode_update_v1()
}

/// A `find_replace` of `alpha`, the shape an agent changing one cell sends.
fn replace_edit(find: &str, content: &str) -> EditParams {
    EditParams {
        identifier: "alpha".to_string(),
        domain: "eng".to_string(),
        operation: "find_replace".to_string(),
        find_text: Some(find.to_string()),
        content: Some(content.to_string()),
        ..EditParams::default()
    }
}

/// The agent's edit composes into the live document as the characters it
/// changed, so a keystroke the person sent at the same moment, in another
/// cell of the same row, lands in its own cell rather than next to a deleted
/// copy of the row.
#[tokio::test]
async fn an_agent_edit_merges_per_character_with_a_keystroke_in_flight_in_the_same_row() {
    let (tmp, engine, _scratch) = table_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    let in_flight = type_after_unsent(&doc, "| 1", "x");

    let receipt = engine
        .edit_engram_as(
            &replace_edit("| 2 |", "| 2Z |"),
            None,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the edit lands");
    assert_eq!(receipt["landed"].as_str(), Some("live"), "{receipt}");

    joined
        .session
        .handle_frame(joined.conn, &frame_update(in_flight))
        .await;
    resync(&joined, &doc).await;
    let (live, _) = joined.session.snapshot().await;
    assert!(
        live.ends_with("| 1x | 2Z |\n"),
        "both cells changed in place: {live:?}"
    );
    assert_eq!(client_text(&doc), live, "and the client converged");

    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(on_disk.ends_with("| 1x | 2Z |\n"), "{on_disk:?}");
}

/// A file change to another cell of the row a person has unsaved typing in
/// is absorbed by the three-way merge per character: no conflict, both
/// changes in the room and, after its save, on disk.
#[tokio::test]
async fn a_file_change_to_another_cell_of_the_row_absorbs_into_unsaved_typing() {
    use crystalline_service::collab::session::{Absorbed, RoomDoc};
    let (tmp, engine, _scratch) = table_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    let typed = type_after_unsent(&doc, "| 1", "x");
    joined
        .session
        .handle_frame(joined.conn, &frame_update(typed))
        .await;

    let path = tmp.path().join("eng/alpha.md");
    std::fs::write(&path, TABLE_ALPHA.replace("| 2 |", "| 2Z |")).unwrap();
    let outcome = sessions
        .absorb_doc_change("eng", &RoomDoc::Engram("alpha".into()), None)
        .await
        .unwrap();
    assert_eq!(outcome.absorbed, Absorbed::Merged, "no conflict is raised");
    let (live, dirty) = joined.session.snapshot().await;
    assert!(live.ends_with("| 1x | 2Z |\n"), "{live:?}");
    assert!(dirty, "the merged typing is still the room's to save");

    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.ends_with("| 1x | 2Z |\n"), "{on_disk:?}");
}

/// The body of a stale full-page write: the table as the agent read it
/// before the person's cell edit, with the agent's own change to cell 2.
const STALE_TABLE_BODY: &str = "# Alpha\n\n| a | b |\n| - | - |\n| 1 | 2Z |\n";

fn stale_overwrite_alpha() -> Value {
    json!({
        "name": "write_engram",
        "arguments": {
            "domain": "eng",
            "title": "Alpha",
            "content": STALE_TABLE_BODY,
            "overwrite": true,
        },
    })
}

/// **Probe, Task 11.** An agent read the page, a person then edited cell 1
/// and the room saved it, and the agent sends a full-page `write_engram`
/// with `overwrite: true` built from its old read while the room is still
/// open. The write is refused: the person is still in there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_full_page_overwrite_while_the_room_is_open_is_refused() {
    let (tmp, engine, _scratch) = table_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    publish_name(&joined, &doc, "Jordi").await;
    let typed = type_after_unsent(&doc, "| 1", "x");
    joined
        .session
        .handle_frame(joined.conn, &frame_update(typed))
        .await;
    joined
        .session
        .tick_save(Instant::now() + Duration::from_secs(60))
        .await;
    let path = tmp.path().join("eng/alpha.md");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("| 1x | 2 |")
    );

    let mut wire = Wire::to(engine.clone());
    let answer = wire
        .open(modern(1, "tools/call", stale_overwrite_alpha()))
        .await;
    assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
    assert!(refusal_of(&answer).contains("Jordi"), "{answer}");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("| 1x | 2 |") && !on_disk.contains("2Z"),
        "the saved cell edit stands: {on_disk:?}"
    );
}

/// **Probe, Task 11, a recorded finding rather than a contract.** The same
/// stale full-page write after the person closed the page. Nothing guards it:
/// `write_engram` takes no checksum, so the old read replaces the file and
/// the person's saved cell edit is reverted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_full_page_overwrite_after_the_room_closed_reverts_the_saved_cell_edit() {
    let (tmp, engine, _scratch) = table_fixture().await;
    let sessions = CollabSessions::new(engine.clone());
    engine.set_collab_sessions(&sessions);
    let joined = sessions.join("eng", "alpha", None).await.unwrap();
    let doc = sync_client(&joined).await;
    let typed = type_after_unsent(&doc, "| 1", "x");
    joined
        .session
        .handle_frame(joined.conn, &frame_update(typed))
        .await;
    assert!(joined.session.remove_conn(joined.conn).await);
    joined.session.final_save().await;
    sessions.dispose_if_empty(&joined.session).await;
    let path = tmp.path().join("eng/alpha.md");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("| 1x | 2 |")
    );

    let mut wire = Wire::to(engine.clone());
    let answer = wire
        .open(modern(1, "tools/call", stale_overwrite_alpha()))
        .await;
    assert_ne!(answer["result"]["isError"], json!(true), "{answer}");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("| 1 | 2Z |") && !on_disk.contains("1x"),
        "the old read replaced the person's saved edit: {on_disk:?}"
    );
}
