//! The collab upgrade route end to end: the guards that refuse before any
//! protocol traffic, and a real two-socket session over tokio-tungstenite.
//!
//! Every refusal here is an ordinary problem+json answer on the plain GET, so
//! the assertions read the HTTP status the handshake was refused with rather
//! than a close code: a client that is not allowed to edit never sees a
//! WebSocket at all.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::collab::control::{self, Control};
use crystalline_service::collab::session::MAX_PARTICIPANTS;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite;
use yrs::sync::{Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{Doc, Options, ReadTxn, Text, Transact, Update};

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// How long a helper waits for a frame it expects before failing the test.
const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What a collab-test server varies.
#[derive(Default)]
struct Options_ {
    anonymous: bool,
    read_only: bool,
}

struct Fixture {
    addr: std::net::SocketAddr,
    /// The domain folder on disk, so a test can read what a save landed.
    domain_dir: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

/// A served instance over a file domain `eng` holding MANIFEST, alpha, a CRLF
/// engram and a mixed-endings one. Mirrors the `serve`/`login` trio in
/// `rest_write_api.rs` and the domain of `collab_session.rs`; integration test
/// crates share no helpers, so both are copied rather than imported.
async fn serve(opts: Options_) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        auth: Some(AuthConfig {
            trusted_header: None,
            anonymous: Some(opts.anonymous),
            max_users: None,
        }),
        ..GlobalConfig::default()
    };
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
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir.clone()));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        read_only: Some(opts.read_only),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    // `service.read_only` is resolved by the daemon rather than by the engine's
    // constructor, so the fixture applies it the way `serve` does.
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_read_only(opts.read_only),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("eddy", "Eddy", None, Role::Editor, "eddypw")
        .await
        .unwrap();
    // The second editor: a shared session needs two accounts, not two cookies
    // for one.
    auth.add_user("adda", "Adda", None, Role::Editor, "addapw")
        .await
        .unwrap();
    auth.add_user("vera", "Vera", None, Role::Viewer, "verapw")
        .await
        .unwrap();
    // Domain management is admin-only, so the unregister-sweep test needs one.
    auth.add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();

    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth, None).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        // Served the way `run_http` serves it, connect info included: the peer
        // address the first-run setup route reads lives in the extensions this
        // adds, and a plain router would leave it missing.
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Fixture {
        addr,
        domain_dir: dir,
        _tmp: tmp,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Log in, returning (session cookie value, csrf token).
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({"name": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login as {name} must succeed");
    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .and_then(|v| v.split(';').next())
        .and_then(|v| v.strip_prefix("fluid_session="))
        .unwrap()
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    (cookie, body["csrf"].as_str().unwrap().to_string())
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open the collab socket as `cookie`, with a same-host Origin unless `origin`
/// overrides it. Err carries the HTTP response for the refusal tests.
async fn connect(
    addr: std::net::SocketAddr,
    path: &str,
    cookie: Option<&str>,
    origin: Option<String>,
) -> Result<Socket, tungstenite::Error> {
    let mut request = tungstenite::handshake::client::Request::builder()
        .uri(format!("ws://{addr}{path}"))
        .header("Host", addr.to_string())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        );
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    if let Some(cookie) = cookie {
        request = request.header("Cookie", format!("fluid_session={cookie}"));
    }
    let (socket, _) = tokio_tungstenite::connect_async(request.body(()).unwrap()).await?;
    Ok(socket)
}

fn same_host(addr: std::net::SocketAddr) -> Option<String> {
    Some(format!("http://{addr}"))
}

/// The status a refused upgrade answered with.
fn refusal_status(err: &tungstenite::Error) -> Option<u16> {
    if let tungstenite::Error::Http(response) = err {
        return Some(response.status().as_u16());
    }
    None
}

/// A JS-shaped client: a plain yjs-compatible doc mirroring the session.
fn client_doc() -> Doc {
    Doc::with_options(Options {
        offset_kind: yrs::OffsetKind::Utf16,
        ..Options::default()
    })
}

/// Split a frame into its protocol messages.
fn messages_of(bytes: &[u8]) -> Vec<Message> {
    let mut decoder = DecoderV1::from(bytes);
    MessageReader::new(&mut decoder)
        .collect::<Result<_, _>>()
        .unwrap()
}

fn binary(bytes: Vec<u8>) -> tungstenite::Message {
    tungstenite::Message::Binary(bytes.into())
}

/// The provider's opening move: SyncStep1 carrying the client's state vector.
fn step1(doc: &Doc) -> Vec<u8> {
    Message::Sync(SyncMessage::SyncStep1(doc.transact().state_vector())).encode_v1()
}

/// One client update, framed the way the provider sends it.
fn update_frame(update: &[u8]) -> Vec<u8> {
    Message::Sync(SyncMessage::Update(update.to_vec())).encode_v1()
}

fn control_frame(control: &Control) -> Vec<u8> {
    control::encode(control)
}

/// The next binary frame, skipping the ping/pong keepalive traffic.
async fn next_binary(socket: &mut Socket) -> Vec<u8> {
    let read = async {
        while let Some(message) = socket.next().await {
            match message.expect("the socket stays open") {
                tungstenite::Message::Binary(bytes) => return bytes.to_vec(),
                tungstenite::Message::Close(frame) => panic!("the socket closed: {frame:?}"),
                _ => {}
            }
        }
        panic!("the socket ended without a binary frame");
    };
    tokio::time::timeout(FRAME_TIMEOUT, read)
        .await
        .expect("a binary frame arrives inside the window")
}

/// The next SyncStep2's inner update, whichever frame carries it.
async fn next_sync_step2(socket: &mut Socket) -> Vec<u8> {
    loop {
        let frame = next_binary(socket).await;
        for message in messages_of(&frame) {
            if let Message::Sync(SyncMessage::SyncStep2(update)) = message {
                return update;
            }
        }
    }
}

/// The next relayed update's inner bytes, whichever frame carries it.
async fn next_sync_update(socket: &mut Socket) -> Vec<u8> {
    loop {
        let frame = next_binary(socket).await;
        for message in messages_of(&frame) {
            if let Message::Sync(SyncMessage::Update(update)) = message {
                return update;
            }
        }
    }
}

/// The control message's `kind` on the wire, so a test can wait for one by name.
fn control_kind(control: &Control) -> String {
    serde_json::to_value(control).unwrap()["kind"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Read frames until a control of this kind arrives, panicking on the timeout.
async fn wait_for_control(socket: &mut Socket, kind: &str) -> Control {
    let read = async {
        loop {
            let frame = next_binary(socket).await;
            for message in messages_of(&frame) {
                if let Message::Custom(tag, payload) = message
                    && tag == control::CONTROL_TAG
                    && let Some(control) = control::decode(&payload)
                    && control_kind(&control) == kind
                {
                    return control;
                }
            }
        }
    };
    tokio::time::timeout(FRAME_TIMEOUT, read)
        .await
        .unwrap_or_else(|_| panic!("no {kind} control arrived"))
}

/// The hello control a greeting opens with.
fn decode_hello(greeting: &[u8]) -> Control {
    let Some(Message::Custom(tag, payload)) = messages_of(greeting).into_iter().next() else {
        panic!("the greeting opens with a control message");
    };
    assert_eq!(tag, control::CONTROL_TAG);
    control::decode(&payload).expect("the hello parses")
}

fn apply(doc: &Doc, update: &[u8]) {
    doc.transact_mut()
        .apply_update(Update::decode_v1(update).unwrap())
        .unwrap();
}

/// Append a line at the END of the document and return the update that did it.
/// At the end, not at position 0: text ahead of the frontmatter would make the
/// engine's parse gate refuse the save, correctly.
fn append_line(doc: &Doc, line: &str) -> Vec<u8> {
    // The text handle is taken before the transaction: `get_or_insert_text`
    // opens one of its own, which would deadlock inside ours.
    let text = doc.get_or_insert_text("content");
    let mut txn = doc.transact_mut();
    let end = text.len(&txn);
    text.insert(&mut txn, end, &format!("{line}\n"));
    txn.encode_update_v1()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_upgrade_guards_hold_before_any_protocol_traffic() {
    let fx = serve(Options_ {
        anonymous: true,
        ..Options_::default()
    })
    .await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let viewer = login(fx.addr, "vera", "verapw").await;
    let path = "/api/v1/collab/eng/alpha";

    // No identity at all: the anonymous viewer never writes, so 401.
    let err = connect(fx.addr, path, None, same_host(fx.addr))
        .await
        .unwrap_err();
    assert_eq!(refusal_status(&err), Some(401));
    // A viewer account: 403 - viewers never write either.
    let err = connect(fx.addr, path, Some(&viewer.0), same_host(fx.addr))
        .await
        .unwrap_err();
    assert_eq!(refusal_status(&err), Some(403));
    // An editor with NO Origin: refused, the header is required.
    let err = connect(fx.addr, path, Some(&editor.0), None)
        .await
        .unwrap_err();
    assert_eq!(refusal_status(&err), Some(403));
    // An editor from another origin: refused before upgrade.
    let err = connect(
        fx.addr,
        path,
        Some(&editor.0),
        Some("http://evil.example".into()),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_status(&err), Some(403));
    // A missing engram: 404 in problem+json, not an upgrade.
    let err = connect(
        fx.addr,
        "/api/v1/collab/eng/ghost",
        Some(&editor.0),
        same_host(fx.addr),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_status(&err), Some(404));
    // Mixed line endings: 409, the solo-fallback signal.
    let err = connect(
        fx.addr,
        "/api/v1/collab/eng/mixed",
        Some(&editor.0),
        same_host(fx.addr),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_status(&err), Some(409));
    // And the happy path upgrades.
    let socket = connect(fx.addr, path, Some(&editor.0), same_host(fx.addr)).await;
    assert!(socket.is_ok(), "an editor from the same host connects");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_refuses_the_upgrade() {
    let fx = serve(Options_ {
        read_only: true,
        ..Options_::default()
    })
    .await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let err = connect(
        fx.addr,
        "/api/v1/collab/eng/alpha",
        Some(&editor.0),
        same_host(fx.addr),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal_status(&err), Some(403));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_sockets_sync_edit_and_the_save_lands_once() {
    let fx = serve(Options_::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let second = login(fx.addr, "adda", "addapw").await;
    let path = "/api/v1/collab/eng/alpha";

    let mut alice = connect(fx.addr, path, Some(&editor.0), same_host(fx.addr))
        .await
        .unwrap();
    let mut bob = connect(fx.addr, path, Some(&second.0), same_host(fx.addr))
        .await
        .unwrap();

    // Both greetings arrive: hello control + SyncStep1 (+ awareness).
    let greeting = next_binary(&mut alice).await;
    let Control::Hello {
        permalink,
        separator,
        ..
    } = decode_hello(&greeting)
    else {
        panic!("the greeting opens with hello");
    };
    assert_eq!(permalink, "alpha");
    assert_eq!(separator, "\n");
    let _ = next_binary(&mut bob).await;

    // Alice syncs a local doc, appends a body line, and sends the update.
    let doc = client_doc();
    alice.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut alice).await;
    apply(&doc, &step2);
    let update = append_line(&doc, "smoke line");
    alice.send(binary(update_frame(&update))).await.unwrap();

    // Bob's socket receives the same update.
    let relayed = next_sync_update(&mut bob).await;
    assert_eq!(relayed, update, "the update relays byte for byte");

    // Alice flushes; the save lands on disk.
    alice
        .send(binary(control_frame(&Control::Flush)))
        .await
        .unwrap();
    wait_for_control(&mut alice, "saved").await;
    let on_disk = std::fs::read_to_string(fx.domain_dir.join("alpha.md")).unwrap();
    assert!(on_disk.ends_with("smoke line\n"), "{on_disk:?}");

    // Both sockets close; the session disposes; the file stays as saved.
    alice.close(None).await.unwrap();
    bob.close(None).await.unwrap();
}

/// Unregistering a domain that someone is co-editing right now: the room is
/// swept while the domain is still registered, so the unsaved text lands in
/// the file that stays on disk, the participant is told the room closed, and
/// nobody can open a room in that domain afterwards.
///
/// This is the ordering the route is held to (sweep, then unregister, behind
/// a fence that refuses new joins): inverted, the sweep's final save would
/// either be refused outright or - inside the window between the config write
/// and the index clear - resolve as virtual and land in the DATABASE instead
/// of in the file `files_kept` deliberately left alone. The edit below is
/// never flushed, and the delete follows it well inside the save debounce, so
/// the only thing that can have written it is the sweep's own final save.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistering_a_domain_closes_its_rooms_and_lands_their_text() {
    let fx = serve(Options_::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let admin = login(fx.addr, "root", "rootpw").await;
    let path = "/api/v1/collab/eng/alpha";

    let mut alice = connect(fx.addr, path, Some(&editor.0), same_host(fx.addr))
        .await
        .unwrap();
    let _greeting = next_binary(&mut alice).await;
    let doc = client_doc();
    alice.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut alice).await;
    apply(&doc, &step2);
    let update = append_line(&doc, "typed but never flushed");
    alice.send(binary(update_frame(&update))).await.unwrap();
    // A barrier, not a flush: one socket's frames are processed in order, so
    // a SyncStep2 answering this second SyncStep1 proves the update above was
    // applied to the room before the unregister below is sent.
    alice.send(binary(step1(&doc))).await.unwrap();
    let _ = next_sync_step2(&mut alice).await;

    let resp = client()
        .delete(format!("http://{}/api/v1/domains/eng", fx.addr))
        .header("cookie", format!("fluid_session={}", admin.0))
        .header("x-csrf-token", &admin.1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["rooms_closed"], 1, "the open room was swept");
    assert_eq!(body["files_kept"], true);

    // The participant is told, rather than left holding a socket over a
    // document the server has forgotten.
    let closed = wait_for_control(&mut alice, "closed").await;
    assert!(matches!(closed, Control::Closed { .. }), "{closed:?}");

    // And the unflushed text is in the file that stayed on disk.
    let on_disk = std::fs::read_to_string(fx.domain_dir.join("alpha.md")).unwrap();
    assert!(
        on_disk.ends_with("typed but never flushed\n"),
        "the sweep's final save landed in the file: {on_disk:?}"
    );

    // No room reopens in a domain that is gone.
    let err = connect(fx.addr, path, Some(&editor.0), same_host(fx.addr))
        .await
        .unwrap_err();
    assert_eq!(refusal_status(&err), Some(404));
}

/// Capacity is refused like every other guard: on the plain GET, with a status
/// a client can read, never as a socket that opens and immediately closes. And
/// the slot comes back when a socket closes, which is the property that makes
/// the unwind on the socket loop's exit worth having.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_room_is_refused_before_the_upgrade_and_frees_its_slot() {
    let fx = serve(Options_::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let path = "/api/v1/collab/eng/alpha";

    let mut sockets = Vec::new();
    for _ in 0..MAX_PARTICIPANTS {
        sockets.push(
            connect(fx.addr, path, Some(&editor.0), same_host(fx.addr))
                .await
                .unwrap(),
        );
    }
    let err = connect(fx.addr, path, Some(&editor.0), same_host(fx.addr))
        .await
        .unwrap_err();
    assert_eq!(refusal_status(&err), Some(503));

    // One leaves: the server's socket loop unwinds and the slot is free again.
    sockets.pop().unwrap().close(None).await.unwrap();
    let mut opened = None;
    for _ in 0..100 {
        match connect(fx.addr, path, Some(&editor.0), same_host(fx.addr)).await {
            Ok(socket) => {
                opened = Some(socket);
                break;
            }
            Err(err) => {
                assert_eq!(refusal_status(&err), Some(503));
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    assert!(
        opened.is_some(),
        "a closed socket must give its participant slot back"
    );
}
