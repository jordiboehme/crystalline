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

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::collab::session::{AgentPeer, CollabSessions, Frame, Joined};
use crystalline_service::params::{EditParams, ReadParams};
use tokio::sync::{Mutex, broadcast};
use yrs::sync::{Awareness, Message, MessageReader, SyncMessage};
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
) -> (tempfile::TempDir, Arc<Engine>, support::ScratchStateDir) {
    let scratch = support::ScratchStateDir::acquire();
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
