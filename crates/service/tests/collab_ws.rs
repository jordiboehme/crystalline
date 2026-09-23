//! The collab upgrade route end to end: the guards that refuse before any
//! protocol traffic, and a real two-socket session over tokio-tungstenite.
//!
//! Every refusal here is an ordinary problem+json answer on the plain GET, so
//! the assertions read the HTTP status the handshake was refused with rather
//! than a close code: a client that is not allowed to edit never sees a
//! WebSocket at all.

mod support;

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
use yrs::{Doc, GetString, Options, ReadTxn, Text, Transact, Update};

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
    /// A save through this surface marks its domain pending under the state
    /// directory, so the whole fixture runs against a scratch one.
    _scratch: support::ScratchStateDir,
    _tmp: tempfile::TempDir,
}

/// A served instance over a file domain `eng` holding MANIFEST, alpha, a CRLF
/// engram and a mixed-endings one. Mirrors the `serve`/`login` trio in
/// `rest_write_api.rs` and the domain of `collab_session.rs`; integration test
/// crates share no helpers, so both are copied rather than imported.
async fn serve(opts: Options_) -> Fixture {
    let scratch = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        auth: Some(AuthConfig {
            trusted_header: None,
            proxy_headers: None,
            anonymous: Some(opts.anonymous),
            mcp: None,
            oauth: None,
            max_users: None,
            oidc: None,
            login: None,
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
        _scratch: scratch,
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

/// A room saves back to the permalink it was opened under, so a move that
/// would change that address under an open editor is refused with a 409 that
/// says why, and the file stays where it was. A move that keeps the permalink
/// is not the room's business and goes through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_that_readdresses_an_open_room_is_refused() {
    let fx = serve(Options_::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let mut room = connect(
        fx.addr,
        "/api/v1/collab/eng/alpha",
        Some(&editor.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut room).await;

    let move_with = |body: serde_json::Value| {
        client()
            .post(format!("http://{}/api/v1/domains/eng/move", fx.addr))
            .header("cookie", format!("fluid_session={}", editor.0))
            .header("x-csrf-token", &editor.1)
            .json(&body)
            .send()
    };

    let refused = move_with(serde_json::json!({
        "permalink": "alpha",
        "destination": "alpha.md",
        "new_permalink": "renamed-alpha",
    }))
    .await
    .unwrap();
    assert_eq!(refused.status(), 409);
    let detail = refused.text().await.unwrap();
    assert!(detail.contains("open in the editor"), "{detail}");
    assert!(fx.domain_dir.join("alpha.md").exists());
    assert!(
        std::fs::read_to_string(fx.domain_dir.join("alpha.md"))
            .unwrap()
            .contains("permalink: alpha\n"),
        "nothing was written"
    );

    let kept = move_with(serde_json::json!({
        "permalink": "alpha",
        "destination": "guides/alpha.md",
        "new_permalink": "keep",
    }))
    .await
    .unwrap();
    assert_eq!(kept.status(), 200, "{:?}", kept.text().await);

    // Closing the editor ends the room - its last socket's teardown saves and
    // disposes it - so the ordinary flow (edit, close, then move) is never
    // refused. The teardown runs after the close frame, hence the short wait.
    room.close(None).await.unwrap();
    let deadline = std::time::Instant::now() + FRAME_TIMEOUT;
    loop {
        let renamed = move_with(serde_json::json!({
            "permalink": "alpha",
            "destination": "guides/alpha.md",
            "new_permalink": "guides/alpha",
        }))
        .await
        .unwrap();
        if renamed.status() == 200 {
            break;
        }
        assert_eq!(renamed.status(), 409, "{:?}", renamed.text().await);
        assert!(
            std::time::Instant::now() < deadline,
            "the room outlived its last editor"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        std::fs::read_to_string(fx.domain_dir.join("guides/alpha.md"))
            .unwrap()
            .contains("permalink: guides/alpha\n")
    );
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

// --- a room over somebody else's draft ---------------------------------------

/// The instance the overlay tests run against: one file domain `team` that
/// reviews changes, three editors, and the engine itself so a test can ask
/// where a save landed.
struct Review {
    addr: std::net::SocketAddr,
    engine: Arc<Engine>,
    /// The instance root, so a test can look at the files overlay on disk.
    root: std::path::PathBuf,
    /// The domain folder, so a test can assert that nothing reached it.
    domain_dir: std::path::PathBuf,
    _scratch: support::ScratchStateDir,
    _tmp: tempfile::TempDir,
}

const TEAM_PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Plan\n\nWhat the team agreed.\n";

async fn serve_review() -> Review {
    let scratch = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("team");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# team\n\n## Scope\n\n- Everything the team knows\n\n## When to Use\n\n- Route here for team questions\n",
    )
    .unwrap();
    std::fs::write(dir.join("plan.md"), TEAM_PLAN).unwrap();
    let mut entry = DomainEntry::file(dir.clone());
    entry.review = Some(crystalline_core::config::ReviewMode::Overlay);
    let mut cfg = GlobalConfig {
        auth: Some(AuthConfig {
            trusted_header: None,
            proxy_headers: None,
            anonymous: Some(false),
            mcp: None,
            oauth: None,
            max_users: None,
            oidc: None,
            login: None,
        }),
        ..GlobalConfig::default()
    };
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

    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["alice", "bob", "carol"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    let router = http_router(
        engine.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth,
        None,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Review {
        addr,
        engine,
        root,
        domain_dir: dir,
        _scratch: scratch,
        _tmp: tmp,
    }
}

impl Review {
    /// One account's draft of a page nobody else has, written as the door
    /// would have resolved them. Answers its domain-relative path.
    async fn draft(&self, account: &str, title: &str, body: &str) -> String {
        let receipt = self
            .engine
            .write_engram_as(
                &crystalline_service::params::WriteParams {
                    domain: "team".to_string(),
                    title: title.to_string(),
                    content: body.to_string(),
                    folder: None,
                    engram_type: None,
                    tags: vec!["team".to_string()],
                    status: None,
                    metadata: None,
                    overwrite: false,
                    share_link: None,
                    model: None,
                },
                None,
                &crystalline_service::Scope::User {
                    account: account.to_string(),
                    admin: false,
                },
            )
            .await
            .expect("the draft lands in that account's overlay");
        assert_eq!(receipt["draft"], serde_json::json!(true), "{receipt}");
        receipt["path"].as_str().unwrap().to_string()
    }

    fn request(
        &self,
        session: &(String, String),
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        client()
            .request(method, format!("http://{}{path}", self.addr))
            .header("cookie", format!("fluid_session={}", session.0))
            .header("x-csrf-token", &session.1)
    }

    /// Mint a share-link on the caller's own draft at `path`.
    async fn mint(&self, session: &(String, String), path: &str) -> serde_json::Value {
        let resp = self
            .request(
                session,
                reqwest::Method::POST,
                "/api/v1/domains/team/draft-links",
            )
            .json(&serde_json::json!({ "path": path }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "minting a link on one's own draft");
        resp.json().await.unwrap()
    }

    /// The same, on a link that stops working on its own at `expires_at`.
    ///
    /// The window is the author's to set at mint time and is never moved
    /// afterwards, which is what lets a join carry it rather than re-read it.
    async fn mint_until(
        &self,
        session: &(String, String),
        path: &str,
        expires_at: &str,
    ) -> serde_json::Value {
        let resp = self
            .request(
                session,
                reqwest::Method::POST,
                "/api/v1/domains/team/draft-links",
            )
            .json(&serde_json::json!({ "path": path, "expires_at": expires_at }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "minting a link with a window on it");
        resp.json().await.unwrap()
    }

    /// Redeem a link and then open a join on it: the two steps a person takes
    /// between being handed a draft and being allowed to type in it.
    async fn accept_and_join(&self, session: &(String, String), token: &str) -> serde_json::Value {
        let accepted = self
            .request(session, reqwest::Method::POST, "/api/v1/draft-links/accept")
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), 200, "the link opens the draft");
        let joined = self
            .request(session, reqwest::Method::POST, "/api/v1/draft-links/join")
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .unwrap();
        assert_eq!(joined.status(), 200, "and the join opens the editing");
        joined.json().await.unwrap()
    }
}

/// The problem+json a refused upgrade carries, so a test can read the teaching
/// sentence rather than only the status it arrived with.
fn refusal_detail(err: &tungstenite::Error) -> String {
    let tungstenite::Error::Http(response) = err else {
        panic!("the upgrade was refused with an HTTP answer");
    };
    let body = response.body().clone().unwrap_or_default();
    String::from_utf8_lossy(&body).to_string()
}

/// A grantee who joined types into the OWNER's document: the same room the
/// owner is in, opening on her text, saving into her draft row.
///
/// The whole point of the key change, end to end over the socket: before it,
/// bob would have been in a room over the page the team reviewed and his text
/// would have landed in the machine owner's draft.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grantee_join_lands_in_the_owners_document() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;

    // Alice opens her own draft: no query parameter, and nothing to ask for.
    let mut hers = connect(
        fx.addr,
        "/api/v1/collab/team/fresh",
        Some(&alice.0),
        same_host(fx.addr),
    )
    .await
    .expect("her own draft is hers to co-edit");
    let Control::Hello { epoch, .. } = decode_hello(&next_binary(&mut hers).await) else {
        panic!("the greeting opens with hello");
    };

    // Bob opens the same draft by naming whose it is.
    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .expect("a live link plus a live join is what opens somebody else's draft");
    let Control::Hello {
        epoch: his_epoch, ..
    } = decode_hello(&next_binary(&mut his).await)
    else {
        panic!("the greeting opens with hello");
    };
    assert_eq!(
        his_epoch, epoch,
        "one document, one room: they are editing the same thing"
    );

    // He syncs, types and flushes; her draft row is where it lands.
    let doc = client_doc();
    his.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut his).await;
    apply(&doc, &step2);
    assert!(
        doc.get_or_insert_text("content")
            .get_string(&doc.transact())
            .contains("A page only alice has"),
        "the room opened on her draft"
    );
    let update = append_line(&doc, "bob was here");
    his.send(binary(update_frame(&update))).await.unwrap();
    his.send(binary(control_frame(&Control::Flush)))
        .await
        .unwrap();
    wait_for_control(&mut his, "saved").await;

    let hers_now = fx
        .engine
        .overlay_draft_at("team", "alice", &path)
        .await
        .unwrap()
        .expect("her draft is still hers");
    assert!(
        hers_now.content.contains("bob was here"),
        "his typing landed in her draft: {hers_now:?}"
    );
    assert!(
        fx.engine
            .overlay_draft_at("team", "bob", &path)
            .await
            .unwrap()
            .is_none(),
        "and he forked no copy of his own"
    );
    assert!(
        !fx.domain_dir.join(&path).exists(),
        "and the folder the team reviewed heard nothing of it"
    );
}

/// Naming somebody else's document is refused unless a live link says you may
/// see it and a live join says you are in it.
///
/// Two refusals rather than one, because they are two different states:
/// nobody handed you this draft, and you were handed it to READ. The second
/// is the teaching sentence the write path already speaks - visibility and
/// editing are two steps - so a person meets one rule wherever they meet it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_join_naming_an_ungranted_owner_refuses() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let carol = login(fx.addr, "carol", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    let url = "/api/v1/collab/team/fresh?overlay=alice";

    // Carol was handed nothing: her answer is the one an unshared draft gives
    // everybody, which says nothing about whether alice drafts there at all.
    let err = connect(fx.addr, url, Some(&carol.0), same_host(fx.addr))
        .await
        .unwrap_err();
    assert_eq!(refusal_status(&err), Some(404));

    // Bob holds the link and has not joined: he is told the second step.
    let accepted = fx
        .request(&bob, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200);
    let err = connect(fx.addr, url, Some(&bob.0), same_host(fx.addr))
        .await
        .unwrap_err();
    assert_eq!(refusal_status(&err), Some(422));
    let detail = refusal_detail(&err);
    assert!(
        detail.contains("alice") && detail.contains("your own"),
        "the refusal names both ways forward: {detail}"
    );

    // And once he joins, the same request opens the room.
    fx.accept_and_join(&bob, &token).await;
    connect(fx.addr, url, Some(&bob.0), same_host(fx.addr))
        .await
        .expect("a join is what was missing");
}

/// Taking the link back puts the grantee outside the room on the next saver
/// tick - and leaves the owner in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_grantee_is_closed_on_the_next_tick() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;

    let mut hers = connect(
        fx.addr,
        "/api/v1/collab/team/fresh",
        Some(&alice.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut hers).await;
    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut his).await;

    let id = minted["id"].as_i64().unwrap();
    let revoked = fx
        .request(
            &alice,
            reqwest::Method::DELETE,
            &format!("/api/v1/draft-links/{id}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 204, "the link is hers to take back");

    let closed = wait_for_control(&mut his, "closed").await;
    assert!(matches!(closed, Control::Closed { .. }), "{closed:?}");

    // Hers stands: the eviction is addressed to one connection, and a room
    // that tore itself down over a revoked link would have taken her work
    // with it.
    let doc = client_doc();
    hers.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut hers).await;
    apply(&doc, &step2);
    assert!(
        doc.get_or_insert_text("content")
            .get_string(&doc.transact())
            .contains("A page only alice has"),
        "her socket still answers over her own draft"
    );
}

/// A file uploaded from inside a joined room lands in the OWNER's files
/// overlay, to be folded or discarded with the page it illustrates.
///
/// The routing is the write path's rather than the socket's - an upload is a
/// REST call carrying this session's join key, and
/// `a_joined_upload_of_a_new_file_lands_in_the_owners_overlay` in
/// rest_draft_links.rs pins the rule itself. What this adds is the case the
/// room makes ordinary: somebody typing in a colleague's draft drops a picture
/// into it while they are in there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grantees_upload_from_a_joined_room_lands_in_the_owners_files() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    let joined = fx.accept_and_join(&bob, &token).await;
    let key = joined["join_key"].as_str().unwrap().to_string();

    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut his).await;

    let uploaded = fx
        .request(
            &bob,
            reqwest::Method::PUT,
            "/api/v1/domains/team/files/assets/sketch.png",
        )
        .header("x-crystalline-join", &key)
        .header("content-type", "image/png")
        .body(b"a picture bob drew".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(uploaded.status(), 200, "the upload lands");

    assert!(
        fx.root
            .join("state/overlays/team/alice/files/assets/sketch.png")
            .exists(),
        "in her files overlay, where the page it belongs to is"
    );
    assert!(
        !fx.root
            .join("state/overlays/team/bob/files/assets/sketch.png")
            .exists(),
        "and not in his"
    );
}

/// Leaving the draft puts the socket outside it too, on the same tick and by
/// the same question: the room asks the join registry, and Leave, a
/// revocation, a rename, a discard and a fold all end a join there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grantee_who_leaves_the_draft_is_closed_out_of_its_room() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    let joined = fx.accept_and_join(&bob, &token).await;
    let key = joined["join_key"].as_str().unwrap().to_string();

    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut his).await;

    let left = fx
        .request(&bob, reqwest::Method::POST, "/api/v1/draft-links/leave")
        .json(&serde_json::json!({ "key": key }))
        .send()
        .await
        .unwrap();
    assert_eq!(left.status(), 204);

    let closed = wait_for_control(&mut his, "closed").await;
    assert!(matches!(closed, Control::Closed { .. }), "{closed:?}");

    // The link still opens the draft, so rejoining and reopening the room is
    // the same two presses it was the first time.
    fx.accept_and_join(&bob, &token).await;
    connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .expect("a join is a join, however many times it is opened");
}

/// **A link that runs out puts the socket outside the draft**, the way taking
/// it back does.
///
/// A grant lasts as long as its window says, and until this the window ended
/// only what came THROUGH it: a person already inside the draft went on typing
/// in somebody else's work for as long as they kept the page open, because
/// nothing in the registry knew the link had run out. The join carries the
/// window now, so the saver's next pass finds the join over and closes the
/// room the same way a revocation does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grantee_whose_link_runs_out_is_closed_out_of_its_room() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    // Long enough to be live through the join and the upgrade, short enough
    // that the saver's four-times-a-second pass reaches it inside the test.
    let until = (chrono::Utc::now() + chrono::Duration::seconds(2)).to_rfc3339();
    let minted = fx.mint_until(&alice, &path, &until).await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;

    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .expect("the link is live, so the room opens");
    let _ = next_binary(&mut his).await;

    let closed = wait_for_control(&mut his, "closed").await;
    assert!(matches!(closed, Control::Closed { .. }), "{closed:?}");

    // And the link is over rather than merely put down: rejoining it opens
    // nothing, which is what an expired grant has always answered.
    let rejoined = fx
        .request(&bob, reqwest::Method::POST, "/api/v1/draft-links/join")
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejoined.status(), 404, "a link that ran out opens nothing");
}

/// A link opens the draft it was minted on and no other of its author's.
///
/// The room is asked for by ADDRESS and the grant is held on a PATH, and the
/// two are resolved by different ladders: the grant matches the draft's
/// permalink, its path or the path with the suffix off, while the room
/// resolves the address through the owner's own view, which also matches a
/// draft's TITLE. So one of the owner's other drafts can answer the name the
/// grant was checked against - and it would be handed over in the greeting,
/// which is the one thing a share-link must never widen into.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_opens_the_draft_it_was_minted_on_and_no_other() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    // A second draft of hers, at a path that sorts first, whose TITLE is the
    // address the link was minted on. Nobody shared this one.
    let decoy = fx.draft("alice", "Aaa", "Something else entirely.").await;
    let retitled = "---\ntype: engram\ntitle: fresh\npermalink: aaa\ntags:\n  - team\nstatus: stable\n---\n\nSomething else entirely.\n";
    let read = fx
        .engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: "aaa".to_string(),
                domain: Some("team".to_string()),
                share_link: None,
            },
            &crystalline_service::Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    fx.engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "aaa".to_string(),
                content: retitled.to_string(),
                expected_checksum: read["checksum"].as_str().unwrap().to_string(),
            },
            &crystalline_service::Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .expect("her own draft is hers to retitle");
    assert_eq!(decoy, "aaa.md");

    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;

    let opened = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await;
    match opened {
        Ok(mut socket) => {
            let _ = next_binary(&mut socket).await;
            let doc = client_doc();
            socket.send(binary(step1(&doc))).await.unwrap();
            let step2 = next_sync_step2(&mut socket).await;
            apply(&doc, &step2);
            let text = doc
                .get_or_insert_text("content")
                .get_string(&doc.transact());
            assert!(
                text.contains("A page only alice has"),
                "the room is over the draft the link was minted on: {text}"
            );
        }
        // The other honest answer: the address the room would have opened is
        // not the draft the link names, so it opens nothing - in the same
        // words a name nobody shared with this account gets.
        Err(err) => assert_eq!(refusal_status(&err), Some(404)),
    }
}

/// Replace the whole document in one transaction, the way a client that
/// rewrote the frontmatter does.
fn replace_all(doc: &Doc, content: &str) -> Vec<u8> {
    let text = doc.get_or_insert_text("content");
    let mut txn = doc.transact_mut();
    let len = text.get_string(&txn).encode_utf16().count() as u32;
    text.remove_range(&mut txn, 0, len);
    text.insert(&mut txn, 0, content);
    txn.encode_update_v1()
}

/// A room saves at the path it was opened on, every time, and a document that
/// starts claiming to be a different page is refused rather than followed.
///
/// A room addresses its saves by the permalink its own text carries, and that
/// line is typed by whoever is in the room - so without a screen a guest
/// invited into one page holds a write over every page in its author's
/// overlay. The address ladder answers a TITLE as well as a permalink, and the
/// address check in front of a draft write deliberately does not treat a title
/// as an address, so the two together are the way out of the granted page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rooms_save_at_another_path_is_refused_and_the_room_stays_open() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;

    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut his).await;
    let doc = client_doc();
    his.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut his).await;
    apply(&doc, &step2);

    // He makes the document the team's own page word for word, with one line
    // changed: its address, set to the page's TITLE. Nothing holds that as a
    // permalink, so the address check lets it stand at the granted path - and
    // from the next save on, the name this room addresses by resolves to the
    // team's page. The text is the team's so that the merge the CAS mismatch
    // starts comes out clean, which is what makes the write land rather than
    // stall.
    let retargeted = TEAM_PLAN.replace("permalink: plan", "permalink: Plan");
    let update = replace_all(&doc, &retargeted);
    his.send(binary(update_frame(&update))).await.unwrap();
    his.send(binary(control_frame(&Control::Flush)))
        .await
        .unwrap();
    wait_for_control(&mut his, "saved").await;

    // The next save addresses "Plan", which resolves to the team's page - a
    // path nobody shared with him.
    let update = append_line(&doc, "and now somewhere else");
    his.send(binary(update_frame(&update))).await.unwrap();
    his.send(binary(control_frame(&Control::Flush)))
        .await
        .unwrap();
    let refused = wait_for_control(&mut his, "save-failed").await;
    let Control::SaveFailed { detail } = &refused else {
        panic!("a save at another path is refused: {refused:?}")
    };
    assert!(
        detail.contains("alice") && detail.contains("fresh.md") && detail.contains("plan.md"),
        "the refusal names the draft this room is and the path the write would have gone to: \
         {detail}"
    );

    assert!(
        fx.engine
            .overlay_draft_at("team", "alice", "plan.md")
            .await
            .unwrap()
            .is_none(),
        "and nothing of hers stands at the page he re-addressed to"
    );
    assert_eq!(
        std::fs::read_to_string(fx.domain_dir.join("plan.md")).unwrap(),
        TEAM_PLAN,
        "nor did the folder move"
    );
    let still_hers = fx
        .engine
        .overlay_draft_at("team", "alice", &path)
        .await
        .unwrap()
        .expect("her draft is where it always was");
    assert!(
        !still_hers.content.contains("and now somewhere else"),
        "the refused text landed nowhere: {still_hers:?}"
    );

    // The room is still open: the frame was rejected, not the socket, and his
    // text is still in the document to fix.
    his.send(binary(step1(&doc))).await.unwrap();
    let _ = next_sync_step2(&mut his).await;
}

/// Leaving review mode by DISCARDING an actor's drafts must not publish what a
/// room over one of them was holding.
///
/// The sweep runs one step after the key comes off, so the room's view has
/// already fallen back to the folder and its final save is an ordinary file
/// write. For a fold that is right - those bytes are about to be written
/// anyway. For a discard it is the opposite of what was asked: the rows are
/// dropped without being written, and the unsaved typing would have gone into
/// the reviewed tree a moment earlier.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_discarding_leave_closes_the_room_without_publishing_its_text() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;

    // Alice redrafts the team's page, close enough to it that the merge the
    // sweep's save runs comes out clean - which is the shape where the text
    // reaches the folder rather than stalling in a conflict.
    let read = fx
        .engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: "plan".to_string(),
                domain: Some("team".to_string()),
                share_link: None,
            },
            &crystalline_service::Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    fx.engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                content: TEAM_PLAN.replace("status: stable", "status: draft"),
                expected_checksum: read["checksum"].as_str().unwrap().to_string(),
            },
            &crystalline_service::Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .expect("her redraft is a draft of hers");

    let minted = fx.mint(&alice, "plan.md").await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;
    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/plan?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut his).await;
    let doc = client_doc();
    his.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut his).await;
    apply(&doc, &step2);
    let update = append_line(&doc, "typed but never flushed");
    his.send(binary(update_frame(&update))).await.unwrap();
    // A barrier rather than a flush: one socket's frames are processed in
    // order, so this answer proves the update above reached the room before
    // the leave below.
    his.send(binary(step1(&doc))).await.unwrap();
    let _ = next_sync_step2(&mut his).await;

    let receipt = fx
        .engine
        .set_review_mode(
            "team",
            None,
            crystalline_service::ReviewModeConfirm::Confirmed {
                folds: vec![(
                    "alice".to_string(),
                    crystalline_service::FoldChoice::Discard,
                )],
            },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("the domain stops reviewing changes");
    assert_eq!(
        receipt["rooms_closed"],
        serde_json::json!(1),
        "the room over her draft was swept: {receipt}"
    );

    assert_eq!(
        std::fs::read_to_string(fx.domain_dir.join("plan.md")).unwrap(),
        TEAM_PLAN,
        "and nothing of the discarded draft reached the folder"
    );
    assert!(
        fx.engine
            .overlay_draft_at("team", "alice", "plan.md")
            .await
            .unwrap()
            .is_none(),
        "her draft is gone, which is what discarding means"
    );

    let closed = wait_for_control(&mut his, "closed").await;
    assert!(matches!(closed, Control::Closed { .. }), "{closed:?}");
}

/// The other two endings a join has, driven through to the socket.
///
/// Revoking a link and pressing Leave are asserted above; these are the two
/// that end a join from the DRAFT's side rather than from the link's - its
/// author moves it, or takes it back - and both reach the same registry the
/// tick asks. The owner's own room is not a guest of anything, so it stays;
/// what it meets instead is the deletion-conflict flow, which is the one that
/// keeps her text and lets her put it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_by_the_owner_closes_the_guests_socket() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;

    let mut hers = connect(
        fx.addr,
        "/api/v1/collab/team/fresh",
        Some(&alice.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut hers).await;
    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut his).await;

    fx.engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "fresh".to_string(),
                domain: "team".to_string(),
                destination: "notes/fresh.md".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: None,
            },
            &crystalline_service::Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .expect("her own draft is hers to move");

    let closed = wait_for_control(&mut his, "closed").await;
    assert!(matches!(closed, Control::Closed { .. }), "{closed:?}");

    // Hers stands: a move is the end of a join, not of a room.
    let doc = client_doc();
    hers.send(binary(step1(&doc))).await.unwrap();
    let _ = next_sync_step2(&mut hers).await;
}

/// Discarding one draft - the author takes it back, outside any fold - ends
/// the joins into it, and the author's own room meets the deletion conflict
/// rather than losing what it was holding.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_discarded_draft_evicts_its_guest_and_leaves_its_author_the_conflict() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let bob = login(fx.addr, "bob", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let minted = fx.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    fx.accept_and_join(&bob, &token).await;

    let mut hers = connect(
        fx.addr,
        "/api/v1/collab/team/fresh",
        Some(&alice.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut hers).await;
    let mut his = connect(
        fx.addr,
        "/api/v1/collab/team/fresh?overlay=alice",
        Some(&bob.0),
        same_host(fx.addr),
    )
    .await
    .unwrap();
    let _ = next_binary(&mut his).await;

    fx.engine
        .delete_engram_as(
            &crystalline_service::params::DeleteParams {
                identifier: "fresh".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &crystalline_service::Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .expect("her own draft is hers to take back");

    let closed = wait_for_control(&mut his, "closed").await;
    assert!(matches!(closed, Control::Closed { .. }), "{closed:?}");

    // Her own room is still there, and what it meets is the conflict that
    // keeps her text: the page she was editing is not there to save into.
    let doc = client_doc();
    hers.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut hers).await;
    apply(&doc, &step2);
    let update = append_line(&doc, "still typing");
    hers.send(binary(update_frame(&update))).await.unwrap();
    hers.send(binary(control_frame(&Control::Flush)))
        .await
        .unwrap();
    let raised = wait_for_control(&mut hers, "conflict").await;
    let Control::Conflict { conflict_kind, .. } = &raised else {
        panic!("the room is told its page is gone: {raised:?}")
    };
    assert_eq!(conflict_kind, "deleted");
}

/// The key and the join a presented link opened, or a panic naming the other
/// answer. Every call below presents a link that opens a joinable draft, so
/// `ReadOnly` here is a failure rather than a case.
fn joined(opened: crystalline_service::engine::OpenedLink) -> (String, crystalline_service::Join) {
    match opened {
        crystalline_service::engine::OpenedLink::Joined { key, join } => (key, join),
        crystalline_service::engine::OpenedLink::ReadOnly(reason) => {
            panic!("the link was expected to open a join: {reason}")
        }
    }
}

/// The holder a stateless agent is: a modern-era peer on streamable HTTP has
/// no session at all, so its joins are keyed by the identity its token
/// resolved to.
fn agent_holder(name: &str) -> crystalline_service::Holder {
    crystalline_service::Holder::Token(name.to_string())
}

/// An agent's joined edit of a page its author has open in the editor composes
/// into the author's live document: ruling 2's sentence, end to end.
///
/// The two halves of Task 14 meet here. The join decides WHOSE document the
/// write is about, and the live seam decides that while somebody has that
/// document open, the document is where the write goes - so alice sees the
/// line arrive under her cursor rather than as a conflict ten minutes later,
/// and her own session is what writes it down.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_agent_edit_composes_into_the_owners_open_document() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    let token = fx.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Alice has her own draft open, which is what makes the document live.
    let mut hers = connect(
        fx.addr,
        "/api/v1/collab/team/fresh",
        Some(&alice.0),
        same_host(fx.addr),
    )
    .await
    .expect("her own draft is hers to co-edit");
    decode_hello(&next_binary(&mut hers).await);

    // Bob's agent presents the link it was handed: both browser steps at once,
    // and the join is this session's.
    let bobs_scope = crystalline_service::Scope::User {
        account: "bob".to_string(),
        admin: false,
    };
    let (_key, join) = joined(
        fx.engine
            .open_share_link(&token, &bobs_scope, &agent_holder("bob"))
            .await
            .expect("the link opens alice's draft for bob"),
    );
    assert_eq!(join.owner, "alice");
    let receipt = fx
        .engine
        .edit_engram_joined(
            &crystalline_service::params::EditParams {
                identifier: "fresh".to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some("bob's agent added this".to_string()),
                ..crystalline_service::params::EditParams::default()
            },
            Some("bob"),
            &bobs_scope,
            Some(&join),
        )
        .await
        .expect("a joined edit lands");
    assert_eq!(
        receipt["landed"].as_str(),
        Some("live"),
        "into her open document rather than into the row behind it: {receipt}"
    );
    assert_eq!(
        receipt["joined"],
        serde_json::json!("landed in alice's draft"),
        "and it is still her draft it is about: {receipt}"
    );

    // Her screen has it.
    let doc = client_doc();
    hers.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut hers).await;
    apply(&doc, &step2);
    assert!(
        doc.get_or_insert_text("content")
            .get_string(&doc.transact())
            .contains("bob's agent added this"),
        "the line is in her document"
    );

    // And her session is what makes it durable, into her own draft row.
    hers.send(binary(control_frame(&Control::Flush)))
        .await
        .unwrap();
    wait_for_control(&mut hers, "saved").await;
    let hers_now = fx
        .engine
        .overlay_draft_at("team", "alice", &path)
        .await
        .unwrap()
        .expect("her draft is still hers");
    assert!(
        hers_now.content.contains("bob's agent added this"),
        "her draft row carries it: {hers_now:?}"
    );
}

/// A REST read of an engram somebody has open answers the STORED version, so
/// the validator it carries is still a token a write can be made with.
///
/// The one place the live seam must not reach. On this surface a checksum is a
/// version token: it comes back as an `ETag`, the browser sends it as
/// `If-Match`, and the durable write it guards compares against the row. A
/// checksum of somebody's unsaved document would be a token no save could ever
/// match, so a person who read a page while a colleague had it open would be
/// told their edit was stale, hand back the same token, and be told so again
/// until the colleague's session happened to save. The live view of a document
/// on this surface is the co-editing socket, which is the thing this test has
/// open the whole time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rest_read_of_an_open_engram_answers_a_version_a_write_can_still_use() {
    let fx = serve_review().await;
    let alice = login(fx.addr, "alice", "pw12345678").await;
    let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
    assert_eq!(path, "fresh.md");

    let mut hers = connect(
        fx.addr,
        "/api/v1/collab/team/fresh",
        Some(&alice.0),
        same_host(fx.addr),
    )
    .await
    .expect("her own draft is hers to co-edit");
    decode_hello(&next_binary(&mut hers).await);
    let doc = client_doc();
    hers.send(binary(step1(&doc))).await.unwrap();
    let step2 = next_sync_step2(&mut hers).await;
    apply(&doc, &step2);
    let update = append_line(&doc, "typed and not yet saved");
    hers.send(binary(update_frame(&update))).await.unwrap();

    let read = fx
        .request(
            &alice,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    let etag = read
        .headers()
        .get(reqwest::header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body: serde_json::Value = read.json().await.unwrap();
    assert!(
        !body["content"]
            .as_str()
            .unwrap()
            .contains("typed and not yet saved"),
        "the REST read is the stored version: {body}"
    );
    assert!(
        body.get("live").is_none(),
        "and says nothing about a live document, which is this surface's socket: {body}"
    );

    // And the validator it handed over is one a write can be made with.
    let saved = fx
        .request(
            &alice,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header("if-match", etag)
        .json(&serde_json::json!({
            "content": body["content"].as_str().unwrap().replace(
                "A page only alice has.",
                "A page only alice has, saved from the API.",
            ),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        saved.status(),
        200,
        "the version the read handed over still saves: {:?}",
        saved.text().await
    );
}

/// **An agent's join never opens a room in its person's browser, on either
/// transport.**
///
/// The pin under ruling I2. The co-editing upgrade cannot present a key - a
/// browser puts no header on an upgrade, and a key in a query string is
/// written to every log and proxy on the way - so it asks the registry whether
/// the caller is inside the draft. Asking that about the ACCOUNT made an
/// agent's join open the room for every window that account had open; it asks
/// about the browser session now, and this drives both halves: the agent joins
/// and the window is still outside, the window joins and it is inside.
///
/// Run once per agent holder kind, because the transports differ in nothing
/// else here: a stateless modern peer is a token identity, a stdio agent is a
/// process, and neither is a browser session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agents_join_never_opens_the_browsers_room() {
    for agent in [
        crystalline_service::Holder::Token("bob".to_string()),
        crystalline_service::Holder::Process(7),
    ] {
        let fx = serve_review().await;
        let alice = login(fx.addr, "alice", "pw12345678").await;
        let bob = login(fx.addr, "bob", "pw12345678").await;
        let path = fx.draft("alice", "Fresh", "A page only alice has.").await;
        let token = fx.mint(&alice, &path).await["token"]
            .as_str()
            .unwrap()
            .to_string();

        // Bob's agent presents the link and is inside the draft.
        let bobs_scope = crystalline_service::Scope::User {
            account: "bob".to_string(),
            admin: false,
        };
        fx.engine
            .open_share_link(&token, &bobs_scope, &agent)
            .await
            .expect("the link opens alice's draft for his agent");
        assert!(
            fx.engine.joins().held_by("bob", &agent, "team").len() == 1,
            "{agent:?} is inside it"
        );

        // His browser, which has joined nothing, is still outside.
        let refused = connect(
            fx.addr,
            "/api/v1/collab/team/fresh?overlay=alice",
            Some(&bob.0),
            same_host(fx.addr),
        )
        .await
        .expect_err("a window that joined nothing opens no room");
        let detail = refusal_detail(&refused);
        assert!(
            detail.contains("Join the draft"),
            "and is told how to get in, not that its agent already is: {detail}"
        );

        // And when the window joins for itself, it is in.
        fx.accept_and_join(&bob, &token).await;
        let mut his = connect(
            fx.addr,
            "/api/v1/collab/team/fresh?overlay=alice",
            Some(&bob.0),
            same_host(fx.addr),
        )
        .await
        .expect("a live link plus this window's own join opens the room");
        decode_hello(&next_binary(&mut his).await);
    }
}
