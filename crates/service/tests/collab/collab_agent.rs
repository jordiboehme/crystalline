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

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
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

/// Every verb that goes through the shared edit body meets the live document,
/// not only `edit_engram`.
///
/// A retirement reaches it through `Engine::apply_source_edit`, the shorthand
/// three other verbs share, so an engram somebody has open is retired in their
/// document and their session writes it down. The receipt says nothing about
/// it - a retirement's receipt is about what it retired - so what this pins is
/// the behaviour rather than the words.
///
/// Driven in a REVIEWING domain, which is where a retirement actually takes
/// that route: in a domain that takes changes directly `retire_engram_as`
/// keeps its own file and virtual arms and never reaches the shared body, so a
/// retirement there writes past an open room and the room meets it as the
/// external change it is. See the report's concerns.
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

// --- the wholesale overwrite, asked about before it lands -------------------
//
// An edit COMPOSES into somebody's open document; a `write_engram` carrying
// overwrite=true REPLACES it, and replacing work a person can see on screen
// and has not saved is the one thing an agent may not do quietly. So the call
// asks first, through the era's own round, and a client with no way to put the
// question to anybody is refused rather than served the replacement.
//
// Driven over the wire rather than through the engine, because the round IS
// the wire's: the question travels back as an `input_required` result and the
// answer arrives beside the original arguments on the next call. The fixtures
// below are copied from `mcp_modern_era.rs` for the reason the module header
// already gives - integration test crates share no helpers.

/// The revision these rounds are served at.
const ERA: &str = "2026-07-28";

/// A stdio MCP connection to a server over one engine.
struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    /// rmcp's init loop until the opener has been sent; the running service it
    /// yields is kept alive for the rest of the conversation.
    server: Option<
        tokio::task::JoinHandle<
            Result<
                rmcp::service::RunningService<rmcp::RoleServer, McpServer>,
                rmcp::service::ServerInitializeError,
            >,
        >,
    >,
    running: Option<rmcp::service::RunningService<rmcp::RoleServer, McpServer>>,
}

impl Wire {
    /// A connection to a server over `engine`, not yet opened.
    fn to(engine: Arc<Engine>) -> Wire {
        let (client_io, server_io) = tokio::io::duplex(1 << 18);
        let server =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let (read, write) = tokio::io::split(client_io);
        Wire {
            write,
            lines: tokio::io::BufReader::new(read).lines(),
            server: Some(server),
            running: None,
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

    /// Send the session opener and read its answer, collecting the running
    /// service the init loop hands back.
    async fn open(&mut self, message: Value) -> Value {
        self.send(message).await;
        let task = self.server.take().expect("the opener is sent once");
        self.running = Some(task.await.unwrap().expect("the server opened a session"));
        self.recv().await.expect("the opener was answered")
    }

    /// Send a request on an open connection and read its answer.
    async fn call(&mut self, message: Value) -> Value {
        self.send(message).await;
        self.recv().await.expect("the request was answered")
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

/// One `write_engram` call replacing `alpha` wholesale, with an optional
/// answer to the round.
fn overwrite_alpha(responses: Option<Value>) -> Value {
    let mut params = json!({
        "name": "write_engram",
        "arguments": {
            "domain": "eng",
            "title": "Alpha",
            "content": REPLACEMENT,
            "overwrite": true,
        },
    });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// The same capture WITHOUT `overwrite`: the shape an agent sends when it does
/// not know the permalink is taken.
fn capture_alpha(responses: Option<Value>) -> Value {
    let mut params = json!({
        "name": "write_engram",
        "arguments": {
            "domain": "eng",
            "title": "Alpha",
            "content": REPLACEMENT,
        },
    });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// The client's answer to the `confirm` question, as an `ElicitResult`.
fn answer(action: &str, confirm: bool) -> Value {
    json!({ "confirm": { "action": action, "content": { "confirm": confirm } } })
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

/// **The brief's first test.** A wholesale overwrite of a document somebody
/// has open answers the question instead of the write, and the question names
/// who is in there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wholesale_overwrite_into_a_live_document_asks_first_naming_who_is_present() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    // Opened through the era's own onboarding call, so the round below runs on
    // a connection that never handshook.
    let discovered = wire.open(modern(1, "server/discover", json!({}))).await;
    assert!(
        discovered["result"]["instructions"].is_string(),
        "the connection is open at the era: {discovered}"
    );

    let asked = wire
        .call(eliciting(2, "tools/call", overwrite_alpha(None)))
        .await;
    let result = &asked["result"];
    assert_eq!(
        result["resultType"],
        json!("input_required"),
        "the call answers with a round rather than a replacement: {asked}"
    );

    let question = &result["inputRequests"]["confirm"];
    assert_eq!(
        question["method"],
        json!("elicitation/create"),
        "the round is an elicitation keyed `confirm`: {asked}"
    );
    let message = question["params"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("live editor"),
        "the question says the document is open: {message}"
    );
    assert!(
        message.contains("Jordi"),
        "and names who is in there: {message}"
    );
    assert!(
        message.contains("alpha"),
        "and which engram it is about: {message}"
    );

    // Round one replaces nothing: not the file, and not the document the
    // person is looking at.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, ALPHA, "the file is untouched: {on_disk:?}");
    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains("typed but never saved") && !live.contains(REPLACEMENT),
        "and their unsaved line still stands: {live:?}"
    );
}

/// A capture that did not pass `overwrite` into a live document asks ONE
/// question, and the answer to it lands.
///
/// Both questions apply here: the permalink is taken, and a room is open over
/// it. The collision round used to be asked first and the live round second,
/// so the person was asked twice about one act in two different wordings - and
/// a client that sends only the newest answer, rather than accumulating them,
/// answered the second question, found `overwrite` unset again and was asked
/// the first one back. This is the two-round sequence with nothing carried
/// forward: one question, one answer, one write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_into_a_live_document_asks_one_question_and_the_answer_lands() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    let asked = wire
        .open(eliciting(1, "tools/call", capture_alpha(None)))
        .await;
    let result = &asked["result"];
    assert_eq!(
        result["resultType"],
        json!("input_required"),
        "round one asks rather than writing: {asked}"
    );
    assert!(
        result["inputRequests"]["confirm"].is_null(),
        "and it is not the confirm key, which a second round would have to \
         carry beside the answer below: {asked}"
    );
    let question = &result["inputRequests"]["resolution"];
    let message = question["params"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("already exists"),
        "the one question says the engram is there: {message}"
    );
    assert!(
        message.contains("live editor") && message.contains("Jordi"),
        "and that somebody is in it, by name: {message}"
    );

    // Round two carries the answer to that question and NOTHING else - no
    // `confirm` beside it, which is exactly the client behaviour the old
    // ordering could not terminate under.
    let done = wire
        .call(eliciting(
            2,
            "tools/call",
            capture_alpha(Some(json!({
                "resolution": { "action": "accept", "content": { "resolution": "overwrite" } }
            }))),
        ))
        .await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "round two writes rather than asking again: {done}"
    );
    let receipt = payload_of(&done);
    assert_eq!(
        receipt["landed"],
        json!("live"),
        "and it landed in the document: {receipt}"
    );

    resync(&joined, &doc).await;
    let live = client_text(&doc);
    assert!(
        live.contains(REPLACEMENT),
        "which is where the person sees it: {live:?}"
    );
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(
        on_disk, ALPHA,
        "and nothing was written behind the room's back: {on_disk:?}"
    );
}

/// **The brief's second test.** The same call carrying a yes lands the
/// replacement in the document rather than over it: the room keeps the
/// document it has, the person's session is still what writes it down, and the
/// file is not touched behind their back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_overwrite_replaces_the_text_and_keeps_history() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    let asked = wire
        .open(eliciting(1, "tools/call", overwrite_alpha(None)))
        .await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "round one asks: {asked}"
    );

    let done = wire
        .call(eliciting(
            2,
            "tools/call",
            overwrite_alpha(Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the confirmed round writes: {done}"
    );
    let receipt = payload_of(&done);
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

/// **The brief's third test.** A targeted change is never asked about. It
/// composes with what the person typed instead of replacing it, which is the
/// whole reason the question exists for one verb and not the other.
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

/// **The brief's fourth test.** A client that cannot put the question to
/// anybody is refused, not served the replacement: an overwrite nobody can be
/// asked about would be somebody's unsaved work gone with no way to know.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_without_elicitation_is_refused_not_stomped() {
    let (tmp, engine, _sessions, joined, doc, _scratch) = a_person_typing_in_alpha().await;
    let mut wire = Wire::to(engine.clone());

    let refused = wire
        .open(modern(1, "tools/call", overwrite_alpha(None)))
        .await;
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

/// **Ruling from review.** The refusal a client that cannot be asked gets is
/// the same one at the legacy era, which is what nearly every client in the
/// field still speaks: the gate is era AND capability, so a handshake at
/// 2025-11-25 reaches it however loudly the client declares elicitation.
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

    let refused = wire
        .call(request(2, "tools/call", overwrite_alpha(None)))
        .await;
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
/// rest of the enclosing body. Neither `engine.rs` function this guard
/// watches has that shape today, which is the whole of why the guard is
/// still sound; the other test is what keeps that a checked fact rather than
/// an assumption.
fn declared_fn(line: &str) -> Option<&str> {
    let rest = line.trim_start();
    let rest = rest
        .strip_prefix("pub(crate) ")
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
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine.rs");
    let text = std::fs::read_to_string(&src).unwrap();

    /// Every way an engine function reaches into a room, all of which take
    /// the same session state lock the room's saver holds across the file
    /// lock: the one write, the four reads and the two disposals.
    const ROOM_ENTRY_NEEDLES: [&str; 7] = [
        ".apply_text(",
        ".has_live_room(",
        ".live_text(",
        ".participants(",
        ".touch_agent_presence(",
        ".dispose_domain(",
        ".dispose_domain_discarding(",
    ];

    /// Every way an engine function takes a per-path write lock: the file's
    /// own, and a draft's mirror path through `Engine::draft_lock`, which
    /// wraps the same map. Both are per-path mutexes held across a
    /// read-modify-write, so both close the same cycle against the room's
    /// saver; watching only the spelling that says `write_lock` would leave
    /// every overlay arm unwatched.
    const LOCK_NEEDLES: [&str; 2] = [".write_lock(", ".draft_lock("];

    // Per function, the first line that takes a per-path write lock and the
    // first that makes a room call. A comment mentioning either is not a call,
    // so the scan skips the comment lines the arms are thick with.
    let mut current = "<file scope>".to_string();
    let mut locked: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut composed: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, line) in text.lines().enumerate() {
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
        }
    }

    let offenders: Vec<&String> = composed
        .keys()
        .filter(|name| matches!(locked.get(*name), Some(lock) if lock < &composed[*name]))
        .collect();
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
/// against `engine.rs`; it is what stands between "the guard is sound" being
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
