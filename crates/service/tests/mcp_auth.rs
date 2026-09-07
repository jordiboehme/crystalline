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
//!
//! Authenticating each request on its own is not the whole of it, though: a
//! session is protocol state rmcp routes by an id, so the gate also binds each
//! session to the identity that opened it and refuses anyone else who names it.
//! Two accounts that both hold valid tokens are still two accounts.

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

/// The revision whose requests carry their own `_meta` and route statelessly,
/// with no session in the picture at all.
const ERA: &str = "2026-07-28";

/// A modern-era `tools/list`: the era's two required `_meta` keys in the body
/// and the SEP-2243 standard headers beside them, which is the shape that
/// reaches the transport without ever touching a session.
async fn post_stateless_tools_list(
    addr: &std::net::SocketAddr,
    token: Option<&str>,
) -> reqwest::Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/list",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": ERA,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {
                    "name": "mcp-auth-test",
                    "version": "0.0.0"
                },
            }
        }
    })
    .to_string();
    let mut request = reqwest::Client::new()
        .post(format!("http://{addr}/"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", ERA)
        .header("mcp-method", "tools/list")
        .body(body);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request.send().await.unwrap()
}

/// The `notifications/initialized` a client sends straight after a successful
/// handshake, on the session the handshake minted.
async fn post_on_session(
    addr: &std::net::SocketAddr,
    session: &str,
    token: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .header("authorization", format!("Bearer {token}"))
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap()
}

/// The session id a successful handshake minted.
fn minted_session(response: &reqwest::Response) -> String {
    response
        .headers()
        .get("mcp-session-id")
        .expect("a legacy handshake mints a session")
        .to_str()
        .unwrap()
        .to_string()
}

/// Two accounts, each with a live token.
async fn two_agents(store: &AuthStore) -> (String, String) {
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    store
        .add_user("bob", "Bob", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    (
        store.issue_mcp_token("ada", "agent").await.unwrap().token,
        store.issue_mcp_token("bob", "agent").await.unwrap().token,
    )
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

/// A session belongs to the account that opened it. Ada may go on using hers;
/// Bob, whose own token is perfectly good, may not borrow it - and the refusal
/// says only that the session is someone else's, never whose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_is_bound_to_the_identity_that_opened_it() {
    let (addr, _guard, store) = serve_with_mcp_auth(true).await;
    let (ada, bob) = two_agents(&store).await;

    let handshake = post_initialize_with_token(&addr, Some(&ada)).await;
    assert_eq!(handshake.status(), 200);
    let session = minted_session(&handshake);
    drop(handshake);

    let mine = post_on_session(&addr, &session, &ada).await;
    assert!(
        mine.status().is_success(),
        "the account that opened the session keeps using it: {}",
        mine.status()
    );

    let borrowed = post_on_session(&addr, &session, &bob).await;
    assert_eq!(
        borrowed.status(),
        403,
        "another identity's valid token does not open someone else's session"
    );
    assert!(
        !borrowed.headers().contains_key("www-authenticate"),
        "nothing is wrong with the credential, so this is not a challenge"
    );
    let body: serde_json::Value = borrowed.json().await.unwrap();
    let text = body["error"].as_str().unwrap();
    assert_eq!(
        text,
        crystalline_service::MCP_SESSION_IDENTITY_MISMATCH,
        "the mismatch has its own body, distinct from the 401 teaching text"
    );
    assert!(
        !text.contains("ada"),
        "the refusal must not name the session's owner: {text}"
    );

    // Ending the session gives the claim up with it, so the gate's record
    // never outlives the transport's own state. Ada's own DELETE is
    // authenticated and matches, so it goes through; what is left afterwards is
    // an id nothing stands behind, which the transport itself refuses.
    let ended = reqwest::Client::new()
        .delete(format!("http://{addr}/"))
        .header("mcp-session-id", &session)
        .header("authorization", format!("Bearer {ada}"))
        .send()
        .await
        .unwrap();
    assert!(
        ended.status().is_success(),
        "the owner may end her own session: {}",
        ended.status()
    );
    let stale = post_on_session(&addr, &session, &bob).await;
    assert_eq!(
        stale.status(),
        404,
        "with the claim released the transport answers for its own vanished session"
    );
}

/// The binding is about sessions and nothing else: a modern-era request carries
/// its own `_meta`, routes statelessly and names no session, so any
/// authenticated identity is served on it whoever else is connected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stateless_request_from_another_identity_is_untouched_by_the_binding() {
    let (addr, _guard, store) = serve_with_mcp_auth(true).await;
    let (ada, bob) = two_agents(&store).await;

    let handshake = post_initialize_with_token(&addr, Some(&ada)).await;
    assert_eq!(handshake.status(), 200);
    drop(handshake);

    let stateless = post_stateless_tools_list(&addr, Some(&bob)).await;
    assert_eq!(
        stateless.status(),
        200,
        "a session-less request is not bound to anyone"
    );
}

/// A revoked token stops working on the session it opened, and it is refused as
/// an authentication failure rather than as a mismatch: the token no longer
/// resolves at all, so the ordinary 401 path answers it before the binding is
/// ever consulted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_token_is_refused_on_the_session_it_opened() {
    let (addr, _guard, store) = serve_with_mcp_auth(true).await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let issued = store.issue_mcp_token("ada", "agent").await.unwrap();

    let handshake = post_initialize_with_token(&addr, Some(&issued.token)).await;
    assert_eq!(handshake.status(), 200);
    let session = minted_session(&handshake);
    drop(handshake);

    assert!(store.revoke_mcp_token("ada", issued.id).await.unwrap());
    let refused = post_on_session(&addr, &session, &issued.token).await;
    assert_eq!(
        refused.status(),
        401,
        "a revoked token is an authentication failure, not a mismatch"
    );
    assert_eq!(refused.headers()["www-authenticate"], "Bearer");
    let body: serde_json::Value = refused.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("Agent access"),
        "the ordinary teaching text answers it"
    );
}

/// The refusal does not depend on the shape of the request: a modern-era
/// (2026-07-28) request carries its own `_meta` and routes statelessly, by far
/// the most different path through the transport, and it is refused at the same
/// door with the same bytes as a legacy handshake.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unauthenticated_modern_era_request_is_refused_at_the_same_door() {
    let (addr, _guard, _store) = serve_with_mcp_auth(true).await;
    let baseline = post_initialize(&addr, None).await;
    assert_eq!(baseline.status(), 401);
    let baseline = baseline.text().await.unwrap();

    let refused = post_stateless_tools_list(&addr, None).await;
    assert_eq!(refused.status(), 401);
    assert_eq!(refused.headers()["www-authenticate"], "Bearer");
    assert_eq!(
        refused.text().await.unwrap(),
        baseline,
        "one refusal, whatever era the request speaks"
    );
}
