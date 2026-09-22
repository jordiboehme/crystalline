//! The page URL a single-engram receipt hands an agent.
//!
//! A binary of its own because the base is a fact about the process rather
//! than about a call: every test here records the same serve intent, so the
//! daemon these tools answer inside is one bound at `0.0.0.0:7411` and the
//! address handed out is its loopback equivalent. `mcp_tools.rs` records none
//! and pins the other outcome - no HTTP surface, so no page and no key.
//!
//! The HTTP legs go through `daemon::http_router`, the production
//! construction, because the origin rule a request is resolved against is
//! installed by `http_base` while that router is built. A hand-assembled
//! router would prove the resolver and not the wiring.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::mcp::McpServer;
use crystalline_service::web_url::UNRESOLVED_NOTE;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// The bind every test in this binary agrees on. `record_serve_intent` is
/// first-call-wins, so under `cargo test` the first test to run sets it and
/// the rest find exactly the same value; under nextest each test is its own
/// process and sets it itself.
const BIND: &str = "0.0.0.0:7411";

/// The address a caller on this machine reaches that bind at: a wildcard is
/// something a server listens on and a client can never dial.
const LOCAL_BASE: &str = "http://127.0.0.1:7411";

/// Say this process is a daemon serving HTTP at [`BIND`], which is what makes
/// a stdio caller's page address derivable at all.
fn record_intent() {
    crystalline_service::instance::record_serve_intent(
        crystalline_service::instance::ServeIntent {
            started_by: crystalline_service::instance::StartMode::Serve,
            http: crystalline_service::instance::HttpBinding::Bound(BIND.to_string()),
            allowed_hosts: Vec::new(),
        },
    );
}

struct Harness {
    _tmp: tempfile::TempDir,
    engine: Arc<Engine>,
    root: std::path::PathBuf,
}

impl Harness {
    /// A harness whose responses are plain JSON, so an assertion lands on the
    /// payload rather than on TOON framing.
    async fn new(domains: &[&str]) -> Harness {
        Harness::build(domains, true).await
    }

    /// The same harness under the DEFAULT response format, TOON, which is
    /// where a template has to survive the rendering as well as the payload.
    async fn new_toon(domains: &[&str]) -> Harness {
        Harness::build(domains, false).await
    }

    /// A real temp-directory domain per name (files are the source of truth)
    /// synced into an in-memory store. Copied from `mcp_tools.rs`, minus the
    /// knobs no test here turns.
    async fn build(domains: &[&str], pin_json: bool) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        for d in domains {
            let dir = root.join(d);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("MANIFEST.md"),
                format!(
                    "---\ntype: manifest\ntitle: {d}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {d}\n\n## Scope\n\n- Everything about {d}\n\n## When to Use\n\n- Route here for {d} questions\n"
                ),
            )
            .unwrap();
            cfg.domains.insert(d.to_string(), DomainEntry::file(dir));
        }
        if pin_json {
            cfg.service = Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                ..ServiceConfig::default()
            });
        }
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        // Nothing here may reach the developer's real OS keychain.
        let token_store = root.join("token-store");
        std::fs::create_dir_all(&token_store).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_token_store_dir(token_store)
                .with_state_dir(root.join("state")),
        );
        engine.sync(None).await.unwrap();
        Harness {
            _tmp: tmp,
            engine,
            root,
        }
    }

    async fn connect(
        &self,
    ) -> (
        RunningService<RoleClient, ()>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        // The server handshake blocks until the client sends `initialize`, so
        // the two must run concurrently.
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let client = rmcp::serve_client((), client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }

    /// [`Harness::connect`] over the HTTP transport but not over HTTP: a
    /// duplex carries no request parts, so this is a server object built
    /// outside the transport, the one case that can tell nothing about the
    /// caller's address.
    async fn connect_http(
        &self,
    ) -> (
        RunningService<RoleClient, ()>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let server_task = tokio::spawn(async move {
            rmcp::serve_server(McpServer::new_http(engine), server_io).await
        });
        let client = rmcp::serve_client((), client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }
}

/// Call a tool and return the whole result, so a test can read the payload and
/// the resource links it carries.
async fn call_result(
    peer: &Peer<RoleClient>,
    tool: &str,
    args: Value,
) -> rmcp::model::CallToolResult {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    peer.call_tool(params)
        .await
        .unwrap_or_else(|e| panic!("{tool} must answer rather than fail: {e}"))
}

/// The JSON payload of a tool result: its first content block, cut at the
/// horizontal rule a write verb's ride-along sentence would follow.
fn payload_of(result: &rmcp::model::CallToolResult) -> Value {
    let whole = serde_json::to_value(result).unwrap();
    let text = whole
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a tool result leads with a text block: {whole}"));
    let json = text.split("\n\n---\n").next().unwrap_or(text);
    serde_json::from_str(json).unwrap_or_else(|e| panic!("the receipt is JSON ({e}): {text}"))
}

/// Call a tool and return its JSON payload.
async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Value {
    payload_of(&call_result(peer, tool, args).await)
}

/// The text block a tool result leads with, exactly as the agent reads it:
/// TOON under the default format, JSON under `json`.
async fn call_text(peer: &Peer<RoleClient>, tool: &str, args: Value) -> String {
    let result = call_result(peer, tool, args).await;
    let whole = serde_json::to_value(&result).unwrap();
    whole
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{tool} answers a text block: {whole}"))
        .to_string()
}

/// The resource links a tool result carries, in order.
fn resource_links(result: &rmcp::model::CallToolResult) -> Vec<Value> {
    serde_json::to_value(result).unwrap()["content"]
        .as_array()
        .expect("a tool result carries content blocks")
        .iter()
        .filter(|block| block["type"] == json!("resource_link"))
        .cloned()
        .collect()
}

/// Serve the production router over this engine on an ephemeral loopback
/// port. `http_router` runs `http_base`, which is what installs the origin
/// rule a request's page address is resolved against.
async fn serve_http(engine: Arc<Engine>, root: &Path) -> std::net::SocketAddr {
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&root.join("web-auth.db"))
            .await
            .unwrap(),
    );
    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth, None).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

/// One raw HTTP/1.1 POST to the MCP endpoint, naming `addr` in the `Host`
/// header so the derived origin carries the port this test bound.
///
/// Raw rather than through an HTTP client because the streamable-HTTP
/// response is chunked SSE with no end a client can wait for: this reads for a
/// bounded window instead, the way `http_stream.rs` does.
async fn post(addr: std::net::SocketAddr, body: &str, session_id: Option<&str>) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut request = format!(
        "POST / HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Connection: close\r\n"
    );
    if let Some(id) = session_id {
        request.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    let _ = stream.write_all(request.as_bytes()).await;
    let _ = stream.flush().await;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// The `mcp-session-id` response header, case-insensitively.
fn extract_session_id(raw: &str) -> String {
    for line in raw.split("\r\n") {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("mcp-session-id")
        {
            return value.trim().to_string();
        }
    }
    panic!("no mcp-session-id header in response:\n{raw}");
}

/// The JSON-RPC payload out of an SSE-framed or plain-JSON response.
fn rpc_payload(raw: &str) -> Value {
    let line = raw
        .lines()
        .find_map(|line| {
            line.strip_prefix("data: ")
                .or_else(|| line.starts_with("{\"jsonrpc\"").then_some(line))
        })
        .unwrap_or_else(|| panic!("no JSON-RPC payload in:\n{raw}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("payload is not JSON ({e}):\n{line}"))
}

/// The tool payload inside a `tools/call` response.
fn tool_payload(raw: &str) -> Value {
    let rpc = rpc_payload(raw);
    let text = rpc
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the call answered a text block: {rpc}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("the receipt is JSON ({e}): {text}"))
}

/// The shared fixture both sides of the product build URLs against.
fn fixture_url(index: usize) -> String {
    let cases: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/web-url/cases.json")).unwrap();
    cases["urls"][index]["web_url"]
        .as_str()
        .unwrap_or_else(|| panic!("the fixture holds a web_url at {index}"))
        .to_string()
}

/// **A stdio caller is told the page address of the daemon it is talking to.**
///
/// The bridge has no request to derive a base from, so the bind is the answer,
/// with the wildcard rewritten to something a browser can open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stdio_read_carries_the_page_url_of_the_daemon_it_runs_in() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "Alpha knowledge" }),
    )
    .await;

    let read = call(
        peer,
        "read_engram",
        json!({ "domain": "eng", "identifier": "alpha" }),
    )
    .await;
    assert_eq!(
        read["web_url"],
        json!(format!("{LOCAL_BASE}/d/eng/e/alpha")),
        "the page address rides beside the engram address: {read}"
    );
    assert_eq!(
        read["url"],
        json!("crystalline://eng/alpha"),
        "the agent's own address is untouched: {read}"
    );
    assert!(
        read.get("web_url_note").is_none(),
        "a resolved address says nothing about the setting: {read}"
    );
}

/// **A write and an edit hand back the page too**, in the receipt and on the
/// handle: a client that follows resource links finds the browser address
/// where it already looks for the engram address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_and_an_edit_receipt_carry_the_url_and_the_link_meta() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let written = call_result(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "Alpha knowledge" }),
    )
    .await;
    let receipt = payload_of(&written);
    let expected = format!("{LOCAL_BASE}/d/eng/e/alpha");
    assert_eq!(receipt["web_url"], json!(expected), "{receipt}");
    let links = resource_links(&written);
    assert_eq!(links.len(), 1, "one link, for the one engram: {links:?}");
    assert_eq!(
        links[0]["_meta"]["web_url"],
        json!(expected),
        "the handle carries the same page as the receipt: {:?}",
        links[0]
    );

    let edited = call_result(
        peer,
        "edit_engram",
        json!({
            "domain": "eng",
            "identifier": "alpha",
            "operation": "append",
            "content": "\nA second line.\n"
        }),
    )
    .await;
    let receipt = payload_of(&edited);
    assert_eq!(receipt["web_url"], json!(expected), "{receipt}");
    let links = resource_links(&edited);
    assert_eq!(links.len(), 1, "one link, for the engram edited: {links:?}");
    assert_eq!(
        links[0]["_meta"]["web_url"],
        json!(expected),
        "{:?}",
        links[0]
    );
}

/// **A move points at where the engram landed**, never at where it was: the
/// address it answered to before the call is the one thing a person following
/// the link cannot use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_receipt_carries_the_destination_url() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "Alpha knowledge" }),
    )
    .await;

    let moved = call_result(
        peer,
        "move_engram",
        json!({ "domain": "eng", "identifier": "alpha", "destination": "notes/alpha" }),
    )
    .await;
    let receipt = payload_of(&moved);
    let expected = format!("{LOCAL_BASE}/d/eng/e/notes/alpha");
    assert_eq!(receipt["to"]["web_url"], json!(expected), "{receipt}");
    assert!(
        receipt["from"].get("web_url").is_none(),
        "the address it left carries no page: {receipt}"
    );
    let links = resource_links(&moved);
    assert_eq!(links.len(), 1, "one link, for the destination: {links:?}");
    assert_eq!(
        links[0]["_meta"]["web_url"],
        json!(expected),
        "{:?}",
        links[0]
    );
}

/// **The encoding is the fixture's, byte for byte**: a domain with a space and
/// a permalink with slashes are exactly the pair the two builders have to
/// agree on, because Fluid's router is what has to match what an agent pastes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_permalink_with_slashes_and_a_domain_with_a_space_encode_as_the_fixture_says() {
    record_intent();
    let h = Harness::new(&["team notes"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let receipt = call(
        peer,
        "write_engram",
        json!({
            "domain": "team notes",
            "folder": "notes/deep",
            "title": "Gamma",
            "content": "Gamma knowledge"
        }),
    )
    .await;
    assert_eq!(receipt["permalink"], json!("notes/deep/gamma"), "{receipt}");
    let expected = fixture_url(1).replace("https://kb.example.com", LOCAL_BASE);
    assert_eq!(receipt["web_url"], json!(expected), "{receipt}");
}

/// **An HTTP caller is told the address it asked at**, which is the whole
/// reason the base is resolved per caller: the same engram read through a
/// browser's own origin and through the bridge is two different addresses and
/// one engram.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_read_carries_the_origin_it_was_asked_at() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "Alpha knowledge" }),
    )
    .await;

    let addr = serve_http(h.engine.clone(), &h.root).await;
    let handshake = post(
        addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"web-url-test","version":"0.0.0"}}}"#,
        None,
    )
    .await;
    let session = extract_session_id(&handshake);
    let _ = post(
        addr,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        Some(&session),
    )
    .await;

    let raw = post(
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_engram","arguments":{"domain":"eng","identifier":"alpha"}}}"#,
        Some(&session),
    )
    .await;
    let payload = tool_payload(&raw);
    let web = payload["web_url"]
        .as_str()
        .unwrap_or_else(|| panic!("the read carries a page address: {payload}"));
    // The port this test bound, which is never [`BIND`]'s: an HTTP caller
    // handed the process's own bind would pass a looser assertion than this
    // one and be wrong in exactly the way the per-caller base exists to avoid.
    assert_eq!(
        web,
        format!("http://{addr}/d/eng/e/alpha"),
        "the origin this request arrived at, port and all: {web}"
    );
}

/// **A server object built outside the transport says so.** There is a page,
/// this caller's address for it is unknown, and the honest answer names the
/// setting that ends the guessing rather than inventing an origin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_server_built_outside_the_transport_says_it_cannot_tell() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (writer, _w) = h.connect().await;
    call(
        writer.peer(),
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "Alpha knowledge" }),
    )
    .await;

    let (client, _server) = h.connect_http().await;
    let read = call(
        client.peer(),
        "read_engram",
        json!({ "domain": "eng", "identifier": "alpha" }),
    )
    .await;
    assert_eq!(
        read["web_url_note"],
        json!(UNRESOLVED_NOTE),
        "the one sentence, naming the setting: {read}"
    );
    assert!(
        read.get("web_url").is_none(),
        "no address is better than a wrong one: {read}"
    );
}

/// The word only the engram body carries, so a text search over it answers
/// exactly one hit: a domain's own MANIFEST is an indexed engram too, and a
/// query it also matches would put a second row in the table the TOON pin
/// reads.
const ONLY_IN_THE_BODY: &str = "zorbulating";

/// Write the one engram every list test here searches for.
async fn seed_one(peer: &Peer<RoleClient>) {
    call(
        peer,
        "write_engram",
        json!({
            "domain": "eng",
            "title": "Alpha",
            "content": format!("Alpha knowledge, {ONLY_IN_THE_BODY}.")
        }),
    )
    .await;
}

/// **A list says the shape once and says nothing per row.** A hit already
/// carries its domain and its permalink, so a URL on every row would be the
/// same sentence repeated as many times as the page is long; the template is
/// that sentence said once, for an agent to fill in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_carries_one_template_and_nothing_per_hit() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    seed_one(peer).await;

    let found = call_result(
        peer,
        "search_engrams",
        json!({ "query": ONLY_IN_THE_BODY, "search_type": "text" }),
    )
    .await;
    let payload = payload_of(&found);
    assert_eq!(
        payload["web_url_template"],
        json!(format!("{LOCAL_BASE}/d/{{domain}}/e/{{permalink}}")),
        "one template, at the top level: {payload}"
    );
    let hits = payload["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("a search answers hits: {payload}"));
    assert!(!hits.is_empty(), "the seeded engram is found: {payload}");
    for hit in hits {
        assert!(
            hit.get("web_url").is_none() && hit.get("web_url_note").is_none(),
            "a row says nothing about a page: {hit}"
        );
    }

    let links = resource_links(&found);
    assert_eq!(
        links.len(),
        hits.len(),
        "one link per hit, as before the template: {links:?}"
    );
    for link in &links {
        assert!(
            link.get("_meta").is_none(),
            "a search link carries no page of its own: {link}"
        );
    }
}

/// **The template survives the default rendering.** Its braces are structural
/// characters in TOON, so the line has to come back quoted, and the hits block
/// beside it has to stay the tabular shape the whole format exists for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_toon_search_ends_with_the_quoted_template_line() {
    record_intent();
    let h = Harness::new_toon(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    seed_one(peer).await;

    let text = call_text(
        peer,
        "search_engrams",
        json!({ "query": ONLY_IN_THE_BODY, "search_type": "text" }),
    )
    .await;
    assert!(
        text.contains(&format!(
            "web_url_template: \"{LOCAL_BASE}/d/{{domain}}/e/{{permalink}}\""
        )),
        "the quoted template line, as the TOON pin renders it: {text}"
    );
    assert!(
        text.contains("hits[1]{"),
        "and the hits block is still one tabular row: {text}"
    );
}

/// **All four list verbs say it, not only the search.** Each answers a
/// different shape - nodes, engrams by recency, a folder level - and every one
/// of them has a domain and a permalink on every row, which is exactly what
/// the one template needs and why no row changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_context_recent_activity_and_browse_domain_carry_the_template() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    seed_one(peer).await;
    let expected = json!(format!("{LOCAL_BASE}/d/{{domain}}/e/{{permalink}}"));

    let context = call(
        peer,
        "build_context",
        json!({ "anchor": "crystalline://eng/alpha" }),
    )
    .await;
    assert_eq!(context["web_url_template"], expected, "{context}");

    let recent = call(peer, "recent_activity", json!({ "timeframe": "7d" })).await;
    assert_eq!(recent["web_url_template"], expected, "{recent}");

    let browsed = call(peer, "browse_domain", json!({ "domain": "eng" })).await;
    assert_eq!(browsed["web_url_template"], expected, "{browsed}");
    assert_eq!(
        browsed["domain"],
        json!("eng"),
        "the domain half of the template is the result's own key: {browsed}"
    );
    let engrams = browsed["engrams"]
        .as_array()
        .unwrap_or_else(|| panic!("a browse answers engrams: {browsed}"));
    assert!(
        engrams.iter().all(|row| row.get("permalink").is_some()),
        "and the permalink half is on every row: {browsed}"
    );
}

/// **A list with no derivable address says so once.** The note is a fact about
/// the caller, not about a row, so a page of twenty hits carries one sentence
/// rather than twenty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_list_built_outside_the_transport_carries_the_note_once() {
    record_intent();
    let h = Harness::new(&["eng"]).await;
    let (writer, _w) = h.connect().await;
    seed_one(writer.peer()).await;

    let (client, _server) = h.connect_http().await;
    let found = call_result(
        client.peer(),
        "search_engrams",
        json!({ "query": ONLY_IN_THE_BODY, "search_type": "text" }),
    )
    .await;
    let payload = payload_of(&found);
    assert_eq!(
        payload["web_url_note"],
        json!(UNRESOLVED_NOTE),
        "the one sentence, naming the setting: {payload}"
    );
    assert!(
        payload.get("web_url_template").is_none(),
        "no address is better than a wrong one: {payload}"
    );
    let whole = serde_json::to_string(&payload).unwrap();
    assert_eq!(
        whole.matches(UNRESOLVED_NOTE).count(),
        1,
        "said once at the top and never on a row: {payload}"
    );
}
