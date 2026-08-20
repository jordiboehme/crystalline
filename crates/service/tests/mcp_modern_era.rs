//! **What a 2026-07-28 client is actually served, over both transports.**
//!
//! Every other rmcp test in this crate drives `serve_client`'s default
//! `ClientLifecycleMode::Initialize` (rmcp 3.1.2 `service/client.rs:633-649`)
//! and therefore exercises the legacy era only. That gap is what this file
//! closes, and it could not be closed before the revision was advertised:
//! `handler/server.rs:64-72` refuses any inline request whose `_meta` names a
//! version outside `supported_protocol_versions()` with `-32022`, before
//! dispatch, so until `crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS`
//! carried 2026-07-28 there was no wire path to a modern
//! `RequestContext::protocol_version()` at all.
//!
//! Three obligations earlier tasks proved another way and handed here to be
//! proved on the wire:
//!
//! - **the caching MUST.** `tests/mcp_cache_hints.rs` builds a
//!   `RequestContext` directly and calls the handler methods, because no modern
//!   peer was reachable. Both transports carry the hints here, `resources/read`
//!   included, and a legacy session on the same binary still carries none.
//! - **subscriptions over HTTP.** `tests/mcp_subscriptions.rs` reaches the
//!   modern dispatch over stdio through the metadata latch
//!   (`service/server.rs:541`, the crate's only call site). Nothing arms that
//!   latch on the streamable-HTTP path, so `subscriptions/listen` was
//!   classified legacy there and answered `method not found` at every revision
//!   we advertised. This file is the first time that path runs.
//! - **identity from request metadata.** `client_actor` reads
//!   `_meta.io.modelcontextprotocol/clientInfo` first for exactly this era; a
//!   modern write now proves it reaches `generated.by`.
//!
//! **The era is sessionless by construction.** No `Mcp-Session-Id` is issued to
//! a modern peer and none is required: rmcp inserts that header at one site
//! (`tower.rs:1911`) inside the legacy session branch, and
//! `is_legacy_request` (`tower.rs:358-408`) routes anything at or above
//! 2026-07-28 past it. Asserted rather than assumed below, because it is the
//! deployment fact an operator behind a load balancer needs.

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::mcp::McpServer;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// The revision this whole file is about, spelled once.
const ERA: &str = "2026-07-28";

/// The revision the legacy contrast legs use: the newest one that still has an
/// `initialize` handshake.
const LEGACY: &str = "2025-11-25";

// --- the engine, and the two wires ------------------------------------------

struct Harness {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    engine: Arc<Engine>,
}

impl Harness {
    async fn new() -> Harness {
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
        cfg.domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        // `configure` reads the GitHub credential once the setting is on;
        // nothing here may reach the developer's real OS keychain.
        let token_store = root.join("token-store");
        std::fs::create_dir_all(&token_store).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_token_store_dir(token_store),
        );
        engine.sync(None).await.unwrap();
        Harness {
            _tmp: tmp,
            root,
            engine,
        }
    }

    /// A raw newline-delimited JSON-RPC conversation over the same duplex
    /// transport the stdio bridge uses. The bytes are the subject here, so no
    /// rmcp client sits between the assertions and the wire.
    async fn stdio(&self) -> Wire {
        let (client_io, server_io) = tokio::io::duplex(1 << 18);
        let engine = self.engine.clone();
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

    /// Bind the daemon's real HTTP router on an ephemeral loopback port. The
    /// same construction `run_http` mounts, so the transport behaviour is the
    /// deployed one rather than a test-only assembly.
    async fn http(&self) -> std::net::SocketAddr {
        let auth = Arc::new(
            crystalline_service::rest::AuthStore::open(&self.root.join("web-auth.db"))
                .await
                .unwrap(),
        );
        let router = http_router(
            self.engine.clone(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            &[],
            auth,
            None,
        )
        .unwrap();
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
}

type ServerTask = tokio::task::JoinHandle<
    Result<
        rmcp::service::RunningService<rmcp::RoleServer, McpServer>,
        rmcp::service::ServerInitializeError,
    >,
>;

struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    /// The init loop's join handle until the opener has been sent. rmcp's
    /// `serve_server` returns as soon as it has handled the first message -
    /// whether that was an `initialize` or an inline modern request - so the
    /// handle is awaited exactly once and the running service it yields is
    /// kept alive for the rest of the conversation.
    server: Option<ServerTask>,
    running: Option<rmcp::service::RunningService<rmcp::RoleServer, McpServer>>,
}

impl Wire {
    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// The next message the server sends, or `None` if it stays silent.
    async fn recv(&mut self) -> Option<Value> {
        match tokio::time::timeout(Duration::from_millis(1500), self.lines.next_line()).await {
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

    /// Send a request on an open session and read its answer.
    async fn call(&mut self, message: Value) -> Value {
        self.send(message).await;
        self.recv().await.expect("the request was answered")
    }
}

/// The `_meta` a modern request carries: the two `DRAFT_REQUIRED_KEYS`
/// (rmcp 3.1.2 `model/meta.rs:400-403`) plus the optional `clientInfo` this
/// server reads for write provenance and for nothing else.
fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "modern-era-test", "version": "9.9.9" },
    })
}

fn request(id: u32, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A modern request: the method, its parameters and the era's `_meta`.
fn modern(id: u32, method: &str, mut params: Value) -> Value {
    params["_meta"] = modern_meta();
    request(id, method, params)
}

/// [`modern_meta`] from a client that can put a question to its user: the same
/// two required keys, with an `elicitation` capability declared beside them.
///
/// Capabilities travel per request in this era rather than per session
/// (`RequestContext::client_capabilities` reads `_meta` first and reads only
/// `_meta` once the metadata latch is armed, rmcp 3.1.2 `service.rs:1243-1251`),
/// so one connection can send both shapes and the two halves of the gate are
/// testable against the same server.
fn eliciting_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": { "form": {} } },
        "io.modelcontextprotocol/clientInfo": { "name": "modern-era-test", "version": "9.9.9" },
    })
}

/// A modern request from an eliciting client.
fn eliciting(id: u32, method: &str, mut params: Value) -> Value {
    params["_meta"] = eliciting_meta();
    request(id, method, params)
}

/// The three fields SEP-2549 and SEP-2322 put on a complete result for a
/// modern peer. `ttlMs: 0` and `cacheScope: "public"` are what rmcp's own
/// `#[tool_handler]` emits (`rmcp-macros-3.1.2/src/tool_handler.rs:79-81`), so
/// a hand-written endpoint and a generated one are indistinguishable here.
fn assert_hinted(label: &str, result: &Value) {
    assert_eq!(
        result["resultType"],
        json!("complete"),
        "{label} carries the SEP-2322 discriminator: {result}"
    );
    assert_eq!(result["ttlMs"], json!(0), "{label} carries ttlMs: {result}");
    assert_eq!(
        result["cacheScope"],
        json!("public"),
        "{label} carries cacheScope: {result}"
    );
}

// --- stdio ------------------------------------------------------------------

/// **A modern client is served with no handshake at all.**
///
/// The 2026-07-28 schema has no `initialize` and no `InitializeResult`
/// (`grep -i initialize schema.ts` returns zero hits). rmcp's stdio init loop
/// implements that: a first message that is not `initialize` and carries both
/// required `_meta` keys is dispatched inline, answered, and the session
/// continues (`service/server.rs:527-556`). Before this revision was
/// advertised the same request was answered `-32022` instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_client_is_served_with_no_handshake_at_all() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let answer = wire.open(modern(1, "tools/list", json!({}))).await;
    assert_eq!(answer["id"], json!(1));
    let tools = answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tool list in {answer}"));
    assert_eq!(
        tools.len(),
        22,
        "the one invariant list, unchanged by the era"
    );
    assert_hinted("tools/list", &answer["result"]);
}

/// Discovery is the era's only onboarding channel, and it carries the routing
/// block plus the caching hints rmcp's own `DiscoverResult::from_server_info`
/// sets (`ttl_ms: 0`, `cache_scope: Private`, `model.rs:1258-1263`).
///
/// `supportedVersions` is the other half of the answer, and it is what a
/// dual-era client reads to decide which revision to speak: it now names the
/// era, which is exactly what this task changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_carries_the_routing_block_and_names_the_era() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let answer = wire.open(modern(1, "server/discover", json!({}))).await;
    let result = &answer["result"];
    assert!(
        result["instructions"]
            .as_str()
            .is_some_and(|s| s.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")),
        "the modern era's onboarding path carries the block: {answer}"
    );
    assert!(
        result["instructions"]
            .as_str()
            .is_some_and(|s| s.contains("- eng: Route here for eng questions")),
        "and the block is the live one: {answer}"
    );
    let versions: Vec<&str> = result["supportedVersions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        versions.contains(&ERA),
        "a probing client is told the era is available: {versions:?}"
    );
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["ttlMs"], json!(0));
    assert_eq!(
        result["cacheScope"],
        json!("private"),
        "discover is private because the block follows this deployment's \
         configuration (rmcp sets it, model.rs:1262)"
    );
}

/// **The caching MUST, over the wire, for all six operations.**
///
/// `/server/utilities/caching`: "Servers MUST include caching hints on results
/// with `resultType: "complete"` returned by the following operations:
/// `server/discover`, `tools/list`, `prompts/list`, `resources/list`,
/// `resources/templates/list`, `resources/read`." `tests/mcp_cache_hints.rs`
/// proves the same six by calling the handler methods with a fabricated
/// context, which was the only way to reach a modern peer before the era was
/// advertised. This is the leg it could not run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_operation_the_caching_must_names_carries_its_hints_over_stdio() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    // The opener also arms the metadata latch, so every request after it is
    // dispatched on the modern path.
    let discovered = wire.open(modern(1, "server/discover", json!({}))).await;
    assert_eq!(discovered["result"]["ttlMs"], json!(0));

    for (id, method, params) in [
        (2, "tools/list", json!({})),
        (3, "prompts/list", json!({})),
        (4, "resources/list", json!({})),
        (5, "resources/templates/list", json!({})),
        (
            6,
            "resources/read",
            json!({ "uri": "skill://crystalline-routing/SKILL.md" }),
        ),
    ] {
        let answer = wire.call(modern(id, method, params)).await;
        assert!(
            answer["error"].is_null(),
            "{method} is served to a modern peer: {answer}"
        );
        assert_hinted(method, &answer["result"]);
    }
}

/// The legacy contrast, on the same binary and the same handler: a peer below
/// the era carries none of the three fields.
///
/// **A guard rather than a red**: it passes before and after the era is
/// advertised, and its job is to fail if the hints are ever emitted
/// unconditionally instead of for the peers the revision defines them for. They are 2026-07-28 wire shape,
/// so emitting them to a 2025-11-25 client would be inventing a shape that
/// revision has no field for. rmcp strips `resultType` itself below the era
/// (`handler/server.rs:246-260`); the other two are our own gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_on_the_same_binary_is_handed_none_of_it() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": {},
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));

    for (id, method) in [(2, "tools/list"), (3, "resources/list")] {
        let answer = wire.call(request(id, method, json!({}))).await;
        let result = answer["result"].as_object().unwrap();
        for field in ["resultType", "ttlMs", "cacheScope"] {
            assert!(
                !result.contains_key(field),
                "{method} carries no {field} for a legacy peer: {answer}"
            );
        }
    }
}

/// **V4 on the wire: `ping` is removed, and we inherit the removal.**
///
/// rmcp answers `method_not_found` unless the request is on the legacy
/// lifecycle (`handler/server.rs:112-118`); we implement no `ping` at all, so
/// there is nothing of ours in this behaviour beyond which era a peer is on.
/// Both halves are asserted, because a removal that also broke the legacy era
/// would be a regression rather than conformance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_is_removed_for_a_modern_peer_and_still_answered_for_a_legacy_one() {
    let h = Harness::new().await;

    let mut modern_wire = h.stdio().await;
    modern_wire
        .open(modern(1, "server/discover", json!({})))
        .await;
    let refused = modern_wire.call(modern(2, "ping", json!({}))).await;
    assert_eq!(
        refused["error"]["code"],
        json!(-32601),
        "ping is gone from the era: {refused}"
    );

    let mut legacy_wire = h.stdio().await;
    legacy_wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": {},
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    let answered = legacy_wire.call(request(2, "ping", json!({}))).await;
    assert!(
        answered["error"].is_null() && answered["result"].is_object(),
        "a legacy peer keeps ping: {answered}"
    );
}

/// A request that omits a required `_meta` key on a connection whose latch is
/// armed is refused `-32602`, and the message names both keys.
///
/// The message text is asserted rather than the code alone: a bare code can be
/// produced by a malformed request for reasons that have nothing to do with
/// the lifecycle, which is the trap this program recorded at Task 4.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_missing_its_required_meta_is_refused_with_invalid_params() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    // The opener has to be *answered*, not merely accepted: rmcp arms the
    // latch at `service/server.rs:541` before dispatch, so a probe refused
    // `-32022` for an unadvertised version would arm it too and make the
    // refusal below pass on a connection that was never modern at all.
    let opened = wire.open(modern(1, "server/discover", json!({}))).await;
    assert!(
        opened["result"]["instructions"].is_string(),
        "the connection is a modern one: {opened}"
    );

    let refused = wire.call(request(2, "tools/list", json!({}))).await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(
        refused["error"]["message"],
        json!(
            "request _meta is missing or has malformed required fields: \
             io.modelcontextprotocol/protocolVersion, \
             io.modelcontextprotocol/clientCapabilities"
        ),
        "{refused}"
    );
}

/// The tool list does not move across a `configure` that flips a setting the
/// list used to depend on, and both readings carry their hints.
///
/// SEP-2567 forbids a list varying "as a side effect of other requests on the
/// connection". Task 4 moved the two request-mutable gates to call time and
/// Task 5 froze `skills.serve` at engine construction; this is the same
/// invariance seen by a modern peer, which is the connection the rule was
/// written for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_list_does_not_move_across_a_configure_for_a_modern_client() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let before = wire.open(modern(1, "tools/list", json!({}))).await;
    assert_hinted("tools/list", &before["result"]);

    let configured = wire
        .call(modern(
            2,
            "tools/call",
            json!({ "name": "configure", "arguments": { "set": { "github.enabled": "true" } } }),
        ))
        .await;
    let text = configured["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let snapshot: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    assert_ne!(
        snapshot["github"]["github_enabled"],
        json!(false),
        "the write has to land, or the invariance below proves nothing: {configured}"
    );

    let after = wire.call(modern(3, "tools/list", json!({}))).await;
    assert_eq!(
        before["result"]["tools"], after["result"]["tools"],
        "the list is the same on both sides of a configure"
    );
    assert_hinted("tools/list", &after["result"]);
}

/// A modern write records who asked for it, read from
/// `_meta.io.modelcontextprotocol/clientInfo`.
///
/// There is no handshake in this era, so `ctx.peer.peer_info()` carries
/// rmcp's synthesized `Implementation::default()` with an empty name; without
/// the `_meta` read every modern write would fall back to the generic actor.
/// Reading `clientInfo` for provenance is what the specification intends it
/// for - what it forbids is changing *behaviour* on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_write_records_the_client_from_its_request_metadata() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let written = wire
        .open(modern(
            1,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Provenance",
                    "content": "Who wrote this.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the write succeeds: {written}"
    );

    let text = std::fs::read_to_string(h.root.join("eng/provenance.md")).unwrap();
    assert!(
        text.contains("generated: { by: modern-era-test/9.9.9, at: "),
        "the request metadata identity reaches generated.by: {text}"
    );
}

// --- streamable HTTP --------------------------------------------------------

/// One raw HTTP/1.1 POST, read for a bounded window. Mirrors
/// `tests/http_stream.rs`'s helper rather than sharing it, because an
/// integration test binary cannot reach another one's helpers.
async fn post(
    addr: std::net::SocketAddr,
    body: &str,
    version: Option<&str>,
    method: Option<&str>,
    name: Option<&str>,
    session_id: Option<&str>,
) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut head = "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Connection: close\r\n"
        .to_string();
    if let Some(version) = version {
        head.push_str(&format!("MCP-Protocol-Version: {version}\r\n"));
    }
    if let Some(method) = method {
        head.push_str(&format!("Mcp-Method: {method}\r\n"));
    }
    if let Some(name) = name {
        head.push_str(&format!("Mcp-Name: {name}\r\n"));
    }
    if let Some(id) = session_id {
        head.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    let request = format!("{head}Content-Length: {}\r\n\r\n{body}", body.len());
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

/// A modern POST: the era's `_meta` in the body and the SEP-2243 standard
/// headers beside it, which rmcp requires from any client declaring this
/// revision (`validate_standard_headers`, `tower.rs:673-700`).
async fn modern_post(addr: std::net::SocketAddr, id: u32, method: &str, params: Value) -> String {
    let name = params
        .get("name")
        .or_else(|| params.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let body = modern(id, method, params).to_string();
    post(addr, &body, Some(ERA), Some(method), name.as_deref(), None).await
}

fn head_of(raw: &str) -> String {
    raw.split("\r\n\r\n").next().unwrap_or(raw).to_string()
}

/// The first JSON-RPC payload in an SSE-framed or plain-JSON response.
fn payload(raw: &str) -> Value {
    let line = raw
        .lines()
        .map(|line| line.strip_prefix("data: ").unwrap_or(line).trim())
        .find(|line| line.starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON-RPC payload in:\n{raw}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("payload is not JSON ({e}):\n{line}"))
}

fn has_session_header(raw: &str) -> bool {
    raw.split("\r\n").any(|line| {
        line.split(':')
            .next()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("mcp-session-id"))
    })
}

/// **A modern peer is served over HTTP without a session, with its hints.**
///
/// No `Mcp-Session-Id` is presented and none is returned: rmcp inserts that
/// header at one site (`tower.rs:1911`) inside the legacy session branch, and
/// a request at this revision never reaches it. That is the deployment fact
/// SEP-2575 exists for - a team server behind a load balancer needs no session
/// affinity for these clients.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_request_over_http_is_served_statelessly_with_its_hints() {
    let h = Harness::new().await;
    let addr = h.http().await;

    for (id, method, params) in [
        (1, "tools/list", json!({})),
        (2, "prompts/list", json!({})),
        (3, "resources/list", json!({})),
        (4, "resources/templates/list", json!({})),
        (
            5,
            "resources/read",
            json!({ "uri": "skill://crystalline-routing/SKILL.md" }),
        ),
    ] {
        let raw = modern_post(addr, id, method, params).await;
        assert!(
            raw.starts_with("HTTP/1.1 200 OK"),
            "{method} is served at {ERA}:\n{}",
            head_of(&raw)
        );
        assert!(
            !has_session_header(&raw),
            "{method} gets no session id:\n{}",
            head_of(&raw)
        );
        let answer = payload(&raw);
        assert!(answer["error"].is_null(), "{method}: {answer}");
        assert_hinted(method, &answer["result"]);
    }
}

/// `server/discover` over HTTP answers the routing block. rmcp takes any
/// discover request on the stateless path (`tower.rs:1822`), so this is the
/// onboarding channel for a remote modern client - and it is a different code
/// path from the stdio one, which is why it is asserted separately.
///
/// A **bare** probe over HTTP is still the `422` `tests/http_stream.rs` pins:
/// it carries no `_meta`, so it is classified legacy and takes the session
/// branch. The era changes nothing about that, and the stdio bridge's
/// normalization is what closes it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_over_http_answers_the_routing_block() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let raw = modern_post(addr, 1, "server/discover", json!({})).await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "discover is served:\n{}",
        head_of(&raw)
    );
    assert!(!has_session_header(&raw), "and needs no session");
    let answer = payload(&raw);
    assert!(
        answer["result"]["instructions"]
            .as_str()
            .is_some_and(|s| s.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")),
        "{answer}"
    );
}

/// **The HTTP subscription path, running for the first time.**
///
/// `subscriptions/listen` is refused `method not found` while a request is
/// classified legacy (`handler/server.rs:147-150`), and nothing on the
/// streamable-HTTP path arms the metadata latch, so before the era was
/// advertised this request could not be anything but legacy there. Now it can.
///
/// The two server MUSTs are rmcp's (`SubscriptionContext::establish`,
/// `service/server.rs:337-375`): the acknowledgment is the first message on
/// the stream and it carries the subscription id in `_meta`. Nothing follows
/// it, and that silence is the point rather than a gap - after Tasks 4 and 5
/// no request can move any list, so there is no list-changed event to send.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_http_subscription_stream_acknowledges_first_and_stays_silent() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let raw = post(
        addr,
        &modern(
            1,
            "subscriptions/listen",
            json!({ "notifications": { "toolsListChanged": true } }),
        )
        .to_string(),
        Some(ERA),
        Some("subscriptions/listen"),
        None,
        None,
    )
    .await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "the subscription stream opens:\n{}",
        head_of(&raw)
    );
    assert!(!has_session_header(&raw), "and it is sessionless");

    let events: Vec<Value> = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line.trim()).unwrap())
        .collect();
    let first = events
        .first()
        .unwrap_or_else(|| panic!("no SSE event in:\n{raw}"));
    assert_eq!(
        first["method"],
        json!("notifications/subscriptions/acknowledged"),
        "the acknowledgment is the first message: {first}"
    );
    assert_eq!(
        first["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        json!(1),
        "and it names the subscription: {first}"
    );
    assert!(
        events
            .iter()
            .skip(1)
            .all(|event| event["method"] != json!("notifications/tools/list_changed")),
        "nothing is pushed on the stream: {events:?}"
    );
}

/// **What a legacy-shaped handshake naming the era gets, recorded because it
/// is the one shape the era leaves ragged.**
///
/// Before this task an HTTP `initialize` declaring 2026-07-28 was refused
/// `-32022`, because we did not serve the revision. Now it is served and
/// echoed - and it gets **no session id**, because `is_legacy_request` routed
/// it statelessly from the version in its own body (`tower.rs:358-408`,
/// `:1727`) before any handler ran. A client that goes on to speak the era's
/// request shape works; a client that sends a bare follow-up is asking for the
/// session branch, has no session to present, and gets rmcp's
/// `422 Unexpected message, expect initialize request`.
///
/// That is the era's session model rather than a wedge this task introduced:
/// the handshake is deleted from the 2026-07-28 schema, so a client using it
/// while declaring that revision is contradicting itself. Pinned here so the
/// behaviour is known rather than discovered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handshake_declaring_the_era_is_served_and_gets_no_session() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": ERA,
            "capabilities": {},
            "clientInfo": { "name": "modern-era-test", "version": "9.9.9" },
        },
    })
    .to_string();
    let raw = post(addr, &body, None, None, None, None).await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "the era is no longer refused at the handshake:\n{}",
        head_of(&raw)
    );
    let answer = payload(&raw);
    assert_eq!(answer["result"]["protocolVersion"], json!(ERA), "{answer}");
    assert!(
        !has_session_header(&raw),
        "a modern peer is sessionless:\n{}",
        head_of(&raw)
    );

    // The era's own request shape works on the same endpoint, with no session.
    let served = modern_post(addr, 2, "tools/list", json!({})).await;
    assert!(served.starts_with("HTTP/1.1 200 OK"));
    assert!(payload(&served)["result"]["tools"].is_array());

    // A legacy-shaped follow-up asks for the session branch there is none of.
    let bare = post(
        addr,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#,
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(
        bare.starts_with("HTTP/1.1 422 Unprocessable Entity"),
        "a bare follow-up has no session to present:\n{}",
        head_of(&bare)
    );
}

// --- the confirmation round (SEP-2322 MRTR) ---------------------------------
//
// `delete_engram` is the first tool that answers a round of its own. The gate
// is two-sided and both sides are proved here: the peer must be on this era
// *and* must have declared that it can put a question to its user. A peer
// failing either half is served exactly what 0.15.0 served it, which is the
// contrast the last two tests carry.

/// The arguments that delete the engram the helper below writes.
fn delete_doomed(responses: Option<Value>) -> Value {
    let mut params = json!({
        "name": "delete_engram",
        "arguments": { "domain": "eng", "identifier": "doomed" },
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

/// Open a modern connection by writing the engram the delete tests kill, and
/// hand back both the connection and the file that must still be there.
async fn doomed_engram(h: &Harness) -> (Wire, std::path::PathBuf) {
    let mut wire = h.stdio().await;
    let written = wire
        .open(modern(
            1,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Doomed",
                    "content": "Delete me.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the engram to delete has to exist first: {written}"
    );
    let path = h.root.join("eng/doomed.md");
    assert!(path.exists(), "the write landed on disk");
    (wire, path)
}

/// Round one: an eliciting modern peer is asked before anything is deleted.
///
/// The whole point is the negative half of the assertion - the file is still
/// there when the question comes back - because an `input_required` result
/// that had already deleted the engram would be a confirmation in name only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_eliciting_delete_gets_a_confirmation_question_first() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let asked = wire
        .call(eliciting(2, "tools/call", delete_doomed(None)))
        .await;
    let result = &asked["result"];
    assert_eq!(
        result["resultType"],
        json!("input_required"),
        "the call answers with a round rather than a deletion: {asked}"
    );

    let question = &result["inputRequests"]["confirm"];
    assert_eq!(
        question["method"],
        json!("elicitation/create"),
        "the round is an elicitation keyed `confirm`: {asked}"
    );
    let schema = &question["params"]["requestedSchema"];
    assert_eq!(
        schema["properties"]["confirm"]["type"],
        json!("boolean"),
        "one boolean property: {asked}"
    );
    assert_eq!(
        schema["required"],
        json!(["confirm"]),
        "and it is required: {asked}"
    );

    let message = question["params"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("eng/doomed"),
        "the question names what dies: {message}"
    );
    assert!(
        message.contains("cannot be undone"),
        "and says so plainly: {message}"
    );

    assert!(
        path.exists(),
        "round one deletes nothing: {}",
        path.display()
    );
}

/// Round two with a yes: the same call, now carrying the answer, deletes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_delete_round_two_deletes() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let asked = wire
        .call(eliciting(2, "tools/call", delete_doomed(None)))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    let done = wire
        .call(eliciting(
            3,
            "tools/call",
            delete_doomed(Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the confirmed round deletes: {done}"
    );
    assert_eq!(
        done["result"]["resultType"],
        json!("complete"),
        "and it is an ordinary complete result: {done}"
    );
    assert!(!path.exists(), "the engram is gone: {}", path.display());
}

/// Round two with a no: the call refuses and the engram survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_delete_round_two_deletes_nothing() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let asked = wire
        .call(eliciting(2, "tools/call", delete_doomed(None)))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    let refused = wire
        .call(eliciting(
            3,
            "tools/call",
            delete_doomed(Some(answer("decline", false))),
        ))
        .await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "a decline is a refusal the model can read: {refused}"
    );
    let text = refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("nothing was deleted"),
        "and it says what did not happen: {text}"
    );
    assert!(
        path.exists(),
        "the engram survives a decline: {}",
        path.display()
    );
}

/// A modern peer that declared no elicitation capability gets 0.15.0's
/// behaviour: the delete happens on the first call, with no round at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_peer_without_elicitation_deletes_immediately() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let done = wire
        .call(modern(2, "tools/call", delete_doomed(None)))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the delete runs on the first call: {done}"
    );
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "a peer that cannot answer a question is not asked one: {done}"
    );
    assert!(!path.exists(), "the engram is gone: {}", path.display());
}

/// A legacy peer is served byte for byte what it was before: no `resultType`
/// at all, and the delete on the first call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_deletes_immediately_with_no_input_required() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));

    let written = wire
        .call(request(
            2,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Doomed",
                    "content": "Delete me.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the write lands: {written}"
    );
    let path = h.root.join("eng/doomed.md");
    assert!(path.exists());

    let done = wire
        .call(request(3, "tools/call", delete_doomed(None)))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the delete runs on the first call: {done}"
    );
    assert!(
        !done["result"]
            .as_object()
            .unwrap()
            .contains_key("resultType"),
        "a legacy result carries no discriminator: {done}"
    );
    assert!(!path.exists(), "the engram is gone: {}", path.display());
}

/// **An eliciting peer keeps every delete a legacy peer has**, including the
/// one this verb exists as an escape hatch for: a stray file above the
/// attachment ceiling, which the walker skips and so never gives a row.
///
/// Round one has to succeed before a user can be asked anything, so a preview
/// that refused an over-cap file would not merely lose the byte count - it
/// would refuse the delete outright, and only for the clients that ask before
/// destroying. The whole round trip is driven here rather than the question
/// alone, because "asks, then cannot act" would be the same regression one
/// step later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_peer_can_still_delete_an_over_cap_attachment() {
    let h = Harness::new().await;
    let assets = h.root.join("eng/assets");
    std::fs::create_dir_all(&assets).unwrap();
    let over_cap = crystalline_core::MAX_ATTACHMENT_BYTES + 1;
    let stray = assets.join("big.png");
    std::fs::write(&stray, vec![0u8; over_cap as usize]).unwrap();

    let mut wire = h.stdio().await;
    let call = json!({
        "name": "delete_engram",
        "arguments": { "domain": "eng", "identifier": "assets/big.png" },
    });

    let asked = wire.open(eliciting(1, "tools/call", call.clone())).await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "the oversized file is previewable, so it is asked about: {asked}"
    );
    let message = asked["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("assets/big.png") && message.contains(&format!("{over_cap} bytes")),
        "the question names the file and its real size: {message}"
    );
    assert!(stray.exists(), "round one deletes nothing");

    let mut confirmed = call;
    confirmed["inputResponses"] = answer("accept", true);
    let done = wire.call(eliciting(2, "tools/call", confirmed)).await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "and the confirmed round removes it: {done}"
    );
    assert!(
        !stray.exists(),
        "the stray file is gone: {}",
        stray.display()
    );
}
