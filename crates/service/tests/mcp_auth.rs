//! The MCP gate: with `auth.mcp` on, an agent reaching the HTTP endpoint
//! authenticates before the transport sees its request.
//!
//! Driven against the production router (`daemon::http_router`, the same
//! construction `run_http` mounts) over a live loopback listener, because the
//! whole point of the gate is where it sits in that router - in front of the
//! streamable-HTTP service and behind nothing else, so `/health` and the JSON
//! API keep their own rules and a browser navigation still gets the app shell.
//!
//! The refusal is deliberately HTTP-level rather than a JSON-RPC error: an
//! unauthenticated caller never opens a session, so there is no room to be
//! inside. Every rejected credential gets the identical answer - no header, a
//! wrong scheme, a token that never existed, a revoked one, one belonging to a
//! disabled account - so the response can never be read as an oracle telling an
//! attacker which half of a guess was right.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use tokio::sync::Mutex;

/// A real temp-directory domain synced into an in-memory store, with
/// `auth.mcp` set to `mcp_auth`. Modelled on the other service integration
/// suites' engine builders; the response format is pinned to plain JSON so no
/// assertion here has to account for TOON framing.
async fn build_engine(mcp_auth: bool) -> (tempfile::TempDir, Arc<Engine>) {
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
    cfg.auth = Some(AuthConfig {
        mcp: Some(mcp_auth),
        ..AuthConfig::default()
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

/// Serve the production router on an ephemeral loopback port, handing back the
/// address, the temp directory that has to outlive the server, and the auth
/// store so a test can issue and revoke tokens against the very store the gate
/// resolves them through.
async fn serve_with_mcp_auth(
    mcp_auth: bool,
) -> (std::net::SocketAddr, tempfile::TempDir, Arc<AuthStore>) {
    let (tmp, engine) = build_engine(mcp_auth).await;
    let store = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    let router = http_router(
        engine,
        Arc::new(AtomicUsize::new(0)),
        &[],
        store.clone(),
        None,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
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
    (addr, tmp, store)
}

/// The `initialize` a legacy client opens with - the same shape
/// `tests/http_stream.rs` drives the handshake through, so a refusal here can
/// only be the gate and never a malformed body.
fn initialize_body() -> String {
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"mcp-auth-test","version":"0.0.0"}}}"#
        .to_string()
}

/// POST that handshake at the endpoint root, optionally presenting `bearer` as
/// the `Authorization` header verbatim (so a test can send a malformed one).
async fn post_initialize(
    addr: &std::net::SocketAddr,
    authorization: Option<&str>,
) -> reqwest::Response {
    let client = reqwest::Client::new();
    let mut request = client
        .post(format!("http://{addr}/"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(initialize_body());
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    request.send().await.unwrap()
}

/// The same, with a well-formed `Bearer` presentation of `token`.
async fn post_initialize_with_token(
    addr: &std::net::SocketAddr,
    token: Option<&str>,
) -> reqwest::Response {
    match token {
        Some(token) => post_initialize(addr, Some(&format!("Bearer {token}"))).await,
        None => post_initialize(addr, None).await,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unauthenticated_http_mcp_is_refused_at_the_door_when_auth_is_on() {
    let (addr, _guard, _store) = serve_with_mcp_auth(true).await;
    let resp = post_initialize(&addr, None).await;
    assert_eq!(resp.status(), 401);
    assert_eq!(resp.headers()["www-authenticate"], "Bearer");
    let body: serde_json::Value = resp.json().await.unwrap();
    let text = body["error"].as_str().unwrap();
    assert!(
        text.contains("Agent access"),
        "teaching text names the fix: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_valid_token_opens_the_session_and_a_revoked_one_stops_working() {
    let (addr, _guard, store) = serve_with_mcp_auth(true).await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let issued = store.issue_mcp_token("ada", "t").await.unwrap();
    let ok = post_initialize_with_token(&addr, Some(&issued.token)).await;
    assert_eq!(ok.status(), 200);
    // Drop the open SSE stream before revoking, so the next request is a fresh
    // decision rather than the same connection's answer.
    drop(ok);
    assert!(store.revoke_mcp_token("ada", issued.id).await.unwrap());
    let refused = post_initialize_with_token(&addr, Some(&issued.token)).await;
    assert_eq!(refused.status(), 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_auth_off_the_legacy_open_tier_still_serves() {
    let (addr, _guard, _store) = serve_with_mcp_auth(false).await;
    let resp = post_initialize_with_token(&addr, None).await;
    assert_eq!(resp.status(), 200);
}

/// Every way of failing the gate answers the same bytes. A refusal that said
/// "malformed token" for one input and "unknown token" for another would tell a
/// caller which half of a guess landed; there is exactly one refusal here, and
/// this test is what keeps it that way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_rejected_credential_gets_the_identical_refusal() {
    let (addr, _guard, store) = serve_with_mcp_auth(true).await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    // A live token whose account is then disabled: the account is the thing
    // that stopped being allowed, and the gate must not say so.
    let disabled = store.issue_mcp_token("ada", "disabled").await.unwrap();
    store.set_disabled("ada", true).await.unwrap();

    let baseline = post_initialize(&addr, None).await;
    assert_eq!(baseline.status(), 401);
    let baseline = baseline.text().await.unwrap();

    let presentations = [
        // No scheme at all.
        "cmt_0123456789abcdef".to_string(),
        // The wrong scheme, carrying something that looks right.
        format!("Basic {}", disabled.token),
        // Well formed, but nothing this store ever issued.
        format!("Bearer cmt_{}", "0".repeat(64)),
        // Not an MCP token's shape.
        "Bearer not-a-token".to_string(),
        // Empty credentials.
        "Bearer ".to_string(),
        // A real token belonging to a disabled account.
        format!("Bearer {}", disabled.token),
    ];
    for presentation in presentations {
        let resp = post_initialize(&addr, Some(&presentation)).await;
        assert_eq!(
            resp.status(),
            401,
            "presentation must be refused: {presentation}"
        );
        assert_eq!(resp.headers()["www-authenticate"], "Bearer");
        assert_eq!(
            resp.text().await.unwrap(),
            baseline,
            "refusal must be byte-identical to the no-header one: {presentation}"
        );
    }

    // And the same lowercase scheme a spec-following client may send is not a
    // refusal at all: scheme matching is case-insensitive.
    store.set_disabled("ada", false).await.unwrap();
    let issued = store.issue_mcp_token("ada", "live").await.unwrap();
    let ok = post_initialize(&addr, Some(&format!("bearer {}", issued.token))).await;
    assert_eq!(ok.status(), 200);
}

/// The gate wraps the transport, not the router: the health probe an
/// orchestrator polls and the JSON API's own auth rules are untouched by
/// `auth.mcp`. If the gate ever migrates up to the router, this is what fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_health_probe_and_the_json_api_keep_their_own_rules() {
    let (addr, _guard, _store) = serve_with_mcp_auth(true).await;
    let client = reqwest::Client::new();
    let health = client
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    // The API answers its own way (401 from the REST guard, not the MCP gate),
    // which is visible in the header the MCP gate always sets and this does not.
    let api = client
        .get(format!("http://{addr}/api/v1/domains"))
        .send()
        .await
        .unwrap();
    assert!(
        !api.headers().contains_key("www-authenticate"),
        "the JSON API must not be answered by the MCP gate"
    );
}
