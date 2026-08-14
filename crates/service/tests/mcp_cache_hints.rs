//! SEP-2549 caching hints: the six operations the 2026-07-28 revision makes a
//! MUST, on both servers this crate serves.
//!
//! `/server/utilities/caching`: "Servers MUST include caching hints on results
//! with `resultType: "complete"` returned by the following operations:
//! `server/discover`, `tools/list`, `prompts/list`, `resources/list`,
//! `resources/templates/list`, `resources/read`." `ttlMs` MUST be `>= 0` and
//! `cacheScope` is required because there is no safe default.
//!
//! # Why these tests call the handler methods rather than driving a wire
//!
//! **They were written while a modern peer was unreachable over any transport,
//! which was established by reading the code rather than by trying.** The gate
//! is `RequestContext::protocol_version()` (rmcp 3.1.2 `service.rs:1223-1229`):
//! the request's own `_meta` version first, then the peer's negotiated version.
//! Both are bounded by `crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS`,
//! and while that list did not carry 2026-07-28:
//!
//! - a `_meta` version outside the advertised set is refused
//!   `-32022 unsupported protocol version` before dispatch
//!   (`handler/server.rs:64-72`);
//! - the peer's version is the **negotiated** one, not the requested one:
//!   rmcp's stdio init loop overwrites whatever our `initialize` published with
//!   `negotiated_peer_info.protocol_version = init_response.protocol_version`
//!   and re-publishes it (`service/server.rs:590-595`), so a client asking for
//!   2026-07-28 and answered 2025-11-25 is a 2025-11-25 peer everywhere
//!   afterwards;
//! - over HTTP a header-only `MCP-Protocol-Version: 2026-07-28` never gets that
//!   far either: `validate_request_protocol_version_meta` refuses a request
//!   whose header names the era while its `_meta` does not
//!   (`tower.rs:498-530`), and a matching `_meta` lands back on the `-32022`
//!   above.
//!
//! So there was no wire path to a modern `protocol_version()` at all. These
//! tests therefore build a `RequestContext` directly - which is what
//! `RequestContext::new` (`service.rs:1209`) plus the public `meta` field and
//! `RequestMetaObject::set_protocol_version` (`model/meta.rs:451`) exist for -
//! and call the handler methods the dispatcher would call.
//!
//! **The era is advertised now and the wire leg exists**, in
//! `tests/mcp_modern_era.rs`: all six operations over stdio and five of them
//! over HTTP, hints asserted on real bytes. These tests keep their place
//! rather than being folded into it, because they reach two things a wire
//! cannot: `DegradedServer`, which is stdio-only and built on a failure path,
//! and a revision *newer* than the era, which pins the `>=` in the gate.
//! `http_stream.rs`'s baseline pins the legacy absence on real bytes.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use crystalline_service::{DegradedServer, StubStatus};
use rmcp::ServerHandler;
use rmcp::model::{
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, RequestId, RequestMetaObject,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, service::Peer};
use serde_json::Value;
use tokio::sync::Mutex;

/// One engram-less domain is enough: nothing here reads content, only the
/// shapes of the five results.
async fn build_engine() -> (tempfile::TempDir, Arc<Engine>) {
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

/// A live `Peer<RoleServer>`, which is the one piece of a `RequestContext` a
/// test cannot fabricate (`Peer::new` is crate-private). It comes from a real
/// duplex handshake; both halves are kept alive for the caller's lifetime.
struct PeerLease {
    peer: Peer<RoleServer>,
    _client: rmcp::service::RunningService<rmcp::RoleClient, ()>,
    _server: rmcp::service::RunningService<RoleServer, McpServer>,
}

async fn peer_lease(engine: Arc<Engine>) -> PeerLease {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_task =
        tokio::spawn(async move { rmcp::serve_server(McpServer::new(engine), server_io).await });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let server = server_task.await.unwrap().unwrap();
    PeerLease {
        peer: server.peer().clone(),
        _client: client,
        _server: server,
    }
}

/// Build a `ProtocolVersion` from an arbitrary string the way the wire does:
/// the type has no public constructor for unknown revisions, but its
/// `Deserialize` accepts any string (rmcp 3.1.2 `model.rs:204-220`).
fn protocol_version(s: &str) -> ProtocolVersion {
    serde_json::from_value(Value::String(s.to_string())).unwrap()
}

/// A request context whose `_meta` declares `version`, which is the first
/// thing `RequestContext::protocol_version()` reads.
fn context_at(lease: &PeerLease, version: &str) -> RequestContext<RoleServer> {
    let mut context = RequestContext::new(RequestId::Number(1), lease.peer.clone());
    let mut meta = RequestMetaObject::new();
    meta.set_protocol_version(protocol_version(version));
    context.meta = meta;
    context
}

/// The five results `McpServer` answers with, as JSON, so the assertions read
/// the wire spelling (`ttlMs`, `cacheScope`) rather than the Rust field names.
async fn mcp_results(
    engine: Arc<Engine>,
    lease: &PeerLease,
    version: &str,
) -> Vec<(&'static str, Value)> {
    let server = McpServer::new(engine);
    let tools = server
        .list_tools(None, context_at(lease, version))
        .await
        .unwrap();
    let prompts = server
        .list_prompts(None, context_at(lease, version))
        .await
        .unwrap();
    let resources = server
        .list_resources(None, context_at(lease, version))
        .await
        .unwrap();
    let templates = server
        .list_resource_templates(None, context_at(lease, version))
        .await
        .unwrap();
    let read = server
        .read_resource(
            ReadResourceRequestParams::new("skill://crystalline-routing/SKILL.md"),
            context_at(lease, version),
        )
        .await
        .unwrap();
    let ReadResourceResponse::Complete(read) = read else {
        panic!("a skill read completes rather than asking for input");
    };
    vec![
        ("tools/list", serde_json::to_value(&tools).unwrap()),
        ("prompts/list", serde_json::to_value(&prompts).unwrap()),
        ("resources/list", serde_json::to_value(&resources).unwrap()),
        (
            "resources/templates/list",
            serde_json::to_value(&templates).unwrap(),
        ),
        ("resources/read", serde_json::to_value(&read).unwrap()),
    ]
}

/// The four results `DegradedServer` answers with. `resources/read` and
/// `prompts/get` are rmcp's `method_not_found` defaults there
/// (`handler/server.rs:366-372`, `:396-404`), so they return no result and
/// carry no obligation.
async fn degraded_results(lease: &PeerLease, version: &str) -> Vec<(&'static str, Value)> {
    let server = DegradedServer::new(StubStatus {
        reason: "the index is locked".to_string(),
        binary_version: crystalline_core::VERSION.to_string(),
        daemon_version: None,
        daemon_pid: None,
        channel: None,
    });
    let tools = server
        .list_tools(None, context_at(lease, version))
        .await
        .unwrap();
    let prompts = server
        .list_prompts(None, context_at(lease, version))
        .await
        .unwrap();
    let resources = server
        .list_resources(None, context_at(lease, version))
        .await
        .unwrap();
    let templates = server
        .list_resource_templates(None, context_at(lease, version))
        .await
        .unwrap();
    vec![
        ("tools/list", serde_json::to_value(&tools).unwrap()),
        ("prompts/list", serde_json::to_value(&prompts).unwrap()),
        ("resources/list", serde_json::to_value(&resources).unwrap()),
        (
            "resources/templates/list",
            serde_json::to_value(&templates).unwrap(),
        ),
    ]
}

/// `ttlMs: 0` and `cacheScope: "public"`, which is exactly what rmcp's own
/// `#[tool_handler]` and `#[prompt_handler]` macros emit for the endpoints they
/// generate (`rmcp-macros-3.1.2/src/tool_handler.rs:79-81`,
/// `prompt_handler.rs:71-73`). Mirroring them keeps a hand-written endpoint and
/// a generated one indistinguishable on the wire.
fn assert_hinted(label: &str, result: &Value) {
    let object = result
        .as_object()
        .unwrap_or_else(|| panic!("{label} is an object"));
    assert_eq!(
        object.get("ttlMs"),
        Some(&Value::from(0)),
        "{label} carries ttlMs: 0 for a modern peer: {object:?}"
    );
    assert_eq!(
        object.get("cacheScope"),
        Some(&Value::from("public")),
        "{label} carries cacheScope: public for a modern peer: {object:?}"
    );
}

fn assert_unhinted(label: &str, result: &Value) {
    let object = result
        .as_object()
        .unwrap_or_else(|| panic!("{label} is an object"));
    for hint in ["ttlMs", "cacheScope"] {
        assert!(
            !object.contains_key(hint),
            "{label} must not carry {hint} for a legacy peer: {object:?}"
        );
    }
}

/// **The MUST, on the five operations `McpServer` answers itself.**
///
/// `server/discover` is the sixth and is rmcp's own construction; it has its
/// own test below because it is asserted rather than built.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_peer_gets_cache_hints_on_every_operation_the_specification_names() {
    let (_tmp, engine) = build_engine().await;
    let lease = peer_lease(engine.clone()).await;
    for (label, result) in mcp_results(engine, &lease, "2026-07-28").await {
        assert_hinted(label, &result);
    }
}

/// The other half of the same rule: a peer that negotiated an older revision
/// gets none of it. The fields did not exist before 2026-07-28, so emitting
/// them to a legacy peer would be inventing wire shape, which is the same
/// mistake in the opposite direction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_is_never_handed_a_cache_hint() {
    let (_tmp, engine) = build_engine().await;
    let lease = peer_lease(engine.clone()).await;
    for version in ["2024-11-05", "2025-06-18", "2025-11-25"] {
        for (label, result) in mcp_results(engine.clone(), &lease, version).await {
            assert_unhinted(&format!("{label} at {version}"), &result);
        }
    }
}

/// The boundary is the revision itself, compared the way rmcp compares it:
/// `>=` on the version, which is a lexicographic comparison of ISO dates
/// (`ProtocolVersion` derives `PartialOrd` over its `Cow<str>`,
/// `model.rs:153-155`). A revision newer than the era therefore keeps the
/// obligation rather than silently losing it, which is the failure a `==`
/// would have introduced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revision_newer_than_the_era_keeps_the_obligation() {
    let (_tmp, engine) = build_engine().await;
    let lease = peer_lease(engine.clone()).await;
    for (label, result) in mcp_results(engine, &lease, "2027-01-01").await {
        assert_hinted(label, &result);
    }
}

/// `server/discover`, the sixth operation, satisfied by rmcp rather than by us:
/// `DiscoverResult::from_server_info` sets `ttl_ms: 0` and
/// `cache_scope: Private` on non-optional fields (rmcp 3.1.2
/// `model.rs:1258-1263`), so the hints are always on the wire whatever the
/// peer negotiated. Asserted rather than assumed, because our `discover`
/// override is what decides to keep building through that constructor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_is_hinted_by_rmcps_own_constructor() {
    let (_tmp, engine) = build_engine().await;
    let lease = peer_lease(engine.clone()).await;
    let server = McpServer::new(engine);
    for version in ["2025-06-18", "2026-07-28"] {
        let result = server.discover(context_at(&lease, version)).await.unwrap();
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json.get("ttlMs"),
            Some(&Value::from(0)),
            "discover at {version}: {json}"
        );
        assert_eq!(
            json.get("cacheScope"),
            Some(&Value::from("private")),
            "discover at {version}: {json}"
        );
    }
}

/// **The degraded server serves real clients and owes the same MUST.**
///
/// It advertises the tools capability alone (`stub.rs`, `get_info`), which is
/// not a defence: `Service::handle_request` (rmcp 3.1.2
/// `handler/server.rs:50-245`) dispatches every method unconditionally - the
/// only capability check in the whole match is `validate_tasks_capability` -
/// so a client replaying a stale method list from a healthy session reaches
/// rmcp's empty-list defaults. Those defaults carry no hints, which is why
/// three of these four are overridden here rather than inherited.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_degraded_server_hints_every_operation_it_answers() {
    let (_tmp, engine) = build_engine().await;
    let lease = peer_lease(engine).await;
    for (label, result) in degraded_results(&lease, "2026-07-28").await {
        assert_hinted(label, &result);
    }
    for (label, result) in degraded_results(&lease, "2025-06-18").await {
        assert_unhinted(label, &result);
    }
}
