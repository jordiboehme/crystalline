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

mod support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, OriginConfig, ResponseFormat,
    ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use tokio::sync::Mutex;

/// A real temp-directory domain synced into an in-memory store, with
/// `auth.mcp` set to `mcp_auth` and `auth.proxy_headers` to `proxy_headers`.
/// Modelled on the other service integration suites' engine builders; the
/// response format is pinned to plain JSON so no assertion here has to account
/// for TOON framing.
///
/// The forward-auth mode is a parameter so a test can prove it opens no door
/// here: this gate resolves personal MCP tokens and reads nothing else,
/// whatever a proxy in front says about the person.
async fn build_engine_with(
    mcp_auth: bool,
    proxy_headers: bool,
) -> (tempfile::TempDir, Arc<Engine>) {
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
        proxy_headers: proxy_headers.then_some(true),
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
    serve_with_mcp_auth_and(mcp_auth, false).await
}

/// [`serve_with_mcp_auth`] on an instance that also trusts the forward-auth
/// `Remote-*` headers.
async fn serve_with_mcp_auth_and(
    mcp_auth: bool,
    proxy_headers: bool,
) -> (std::net::SocketAddr, tempfile::TempDir, Arc<AuthStore>) {
    let (tmp, engine) = build_engine_with(mcp_auth, proxy_headers).await;
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

/// Serve a team instance that shares with personal GitHub identities, with the
/// gate on and nobody connected to anything.
///
/// Built in two phases, and both halves are load-bearing (the pattern is
/// `tests/mcp_modern_era.rs`'s `Harness::share_personally`). Phase one
/// subscribes a team domain through an injected mock forge, so no network is
/// touched and no credential is needed. Phase two re-opens the engine from the
/// config the subscription left on disk with NO provider injected: an injected
/// mock short-circuits credential resolution for both identity modes
/// (`Engine::resolve_share_provider`), so a test that kept it would never reach
/// the token store and never see the refusal this asserts on.
///
/// The token store is an empty temp directory throughout, which is what
/// "connected nothing yet" means here and is why the developer's real OS
/// keychain is never read.
async fn serve_personal_share_with_mcp_auth()
-> (std::net::SocketAddr, tempfile::TempDir, Arc<AuthStore>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let config_path = root.join("config.yaml");
    let token_store = root.join("token-store");
    let origins = root.join("origins");
    std::fs::create_dir_all(&token_store).unwrap();

    let mut cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        service: Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        }),
        auth: Some(AuthConfig {
            mcp: Some(true),
            ..AuthConfig::default()
        }),
        ..GlobalConfig::default()
    };
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let mock = Arc::new(support::MockProvider::new());
    let commit = mock.add_commit(
        [(
            "MANIFEST.md".to_string(),
            b"---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# kb\n\n## Scope\n\n- Everything\n\n## When to Use\n\n- Always\n"
                .to_vec(),
        )]
        .into_iter()
        .collect(),
    );
    mock.set_branch("main", &commit);
    let subscribing = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg.clone(),
            None,
            Some(config_path.clone()),
        )
        .with_token_store_dir(token_store.clone())
        .with_origin_provider(mock)
        .with_origins_dir(origins.clone()),
    );
    subscribing
        .origin_add(
            "team/knowledge",
            Some("kb"),
            None,
            None,
            Some(root.join("kb").to_str().unwrap()),
        )
        .await
        .unwrap();
    drop(subscribing);

    // Phase two, off the config the subscription wrote: the domain
    // registration is on disk, the mode is personal and no agent identity is
    // named, which is the one configuration in which an account actor and the
    // HTTP-agent actor refuse with different texts.
    cfg = crystalline_core::config::load_yaml(&config_path).unwrap();
    let github = cfg.github.get_or_insert_with(GitHubConfig::default);
    github.enabled = Some(true);
    github.share_identity = Some("personal".to_string());
    github.agent_identity = None;
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let engine = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg,
            None,
            Some(config_path),
        )
        .with_token_store_dir(token_store)
        .with_origins_dir(origins),
    );

    let store = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
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
    initialize_body_as("mcp-auth-test")
}

/// The same handshake from a client naming itself `client`, which is the whole
/// of what a client gets to say about its own identity and therefore the input
/// the provenance composition has to be safe against.
fn initialize_body_as(client: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": client, "version": "0.0.0" },
        },
    })
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

// --- the session IS the account ---------------------------------------------
//
// Authenticating at the door is half of it. The other half is that the account
// the door resolved reaches the tool call: a write records the human the agent
// acted for, and a personal-identity share runs on that human's own GitHub
// credential rather than on the instance-wide agent account. Both are driven
// end to end here, through the real transport, because the whole claim is that
// the identity survives the trip from the HTTP request into the tool body.

/// One authenticated MCP conversation over the real transport: the legacy
/// handshake, the `notifications/initialized` that follows it, and the
/// `tools/call` POSTs a test drives afterwards.
///
/// Raw HTTP/1.1 over a fresh connection per request, modelled on
/// `tests/http_stream.rs`: a `tools/call` answer is a chunked SSE stream the
/// transport leaves open for the session's own use, so there is no
/// end-of-message a buffering client could wait for. Reading for a bounded
/// window and asserting on substrings is what that shape allows.
struct McpTestSession {
    addr: std::net::SocketAddr,
    session: String,
    token: Option<String>,
}

impl McpTestSession {
    /// Handshake at `addr` presenting `token`, then send the
    /// `notifications/initialized` a client owes the session before its first
    /// call.
    async fn open(addr: &std::net::SocketAddr, token: Option<&str>) -> McpTestSession {
        McpTestSession::open_as(addr, token, "mcp-auth-test").await
    }

    /// [`McpTestSession::open`] from a client that names itself `client`.
    async fn open_as(
        addr: &std::net::SocketAddr,
        token: Option<&str>,
        client: &str,
    ) -> McpTestSession {
        let handshake = raw_post(addr, &initialize_body_as(client), &[], token).await;
        assert!(
            handshake.starts_with("HTTP/1.1 200 "),
            "the handshake must be served:\n{handshake}"
        );
        let session = raw_session_id(&handshake);
        let ready = raw_post(
            addr,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &[("Mcp-Session-Id", session.as_str())],
            token,
        )
        .await;
        assert!(
            ready.starts_with("HTTP/1.1 2"),
            "the initialized notification must be accepted:\n{ready}"
        );
        McpTestSession {
            addr: *addr,
            session,
            token: token.map(str::to_string),
        }
    }

    /// Call `tool` on this session, handing back the raw response bytes.
    async fn call_tool(&self, tool: &str, arguments: serde_json::Value) -> String {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        })
        .to_string();
        raw_post(
            &self.addr,
            &body,
            &[("Mcp-Session-Id", self.session.as_str())],
            self.token.as_deref(),
        )
        .await
    }
}

/// Send one raw HTTP/1.1 POST and read back whatever arrives within a bounded
/// window (see [`McpTestSession`] for why the window is bounded rather than a
/// read to EOF). `headers` carries whatever the shape under test needs beside
/// the fixed ones - a session id for the legacy path, the era's standard
/// headers for a stateless one.
async fn raw_post(
    addr: &std::net::SocketAddr,
    body: &str,
    headers: &[(&str, &str)],
    token: Option<&str>,
) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut request = "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Connection: close\r\n"
        .to_string();
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    let _ = stream.write_all(request.as_bytes()).await;
    let _ = stream.flush().await;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(2500);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.read(&mut chunk)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// The `mcp-session-id` header out of a raw response head, case-insensitively.
fn raw_session_id(raw: &str) -> String {
    for line in raw.split("\r\n") {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("mcp-session-id")
        {
            return value.trim().to_string();
        }
    }
    panic!("no mcp-session-id header in response:\n{raw}");
}

/// **A write by an authenticated agent records the account it acted for.**
///
/// The provenance an engram carries is `generated.by`, and with the gate on it
/// names both halves of who wrote it: the client that asked, and the human
/// whose token opened the session. Without the identity reaching the tool body
/// this reads as the client alone, and an audit of who taught the instance what
/// would stop at "some agent".
///
/// The account arrives as `for-ada` rather than `for ada` because
/// `Engine::actor` runs every actor string through the engine's sanitizer,
/// which folds whitespace runs into a hyphen; the composition itself is
/// `"<client> for <account>"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authenticated_agents_write_records_the_account_it_acts_for() {
    let (addr, guard, store) = serve_with_mcp_auth(true).await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let token = store.issue_mcp_token("ada", "t").await.unwrap().token;

    let session = McpTestSession::open(&addr, Some(&token)).await;
    let answer = session
        .call_tool(
            "write_engram",
            serde_json::json!({
                "domain": "eng",
                "title": "Auth Trace",
                "content": "- [fact] traced",
            }),
        )
        .await;
    assert!(
        answer.contains("\"result\""),
        "the write must be served, not refused:\n{answer}"
    );

    let written = std::fs::read_to_string(guard.path().join("eng").join("auth-trace.md")).unwrap();
    assert!(
        written.contains("for-ada"),
        "generated.by names the account the agent acted for: {written}"
    );
    assert!(
        written.contains("mcp-auth-test"),
        "and still names the client that asked: {written}"
    );
}

/// **A personal-identity share by an authenticated agent resolves that
/// account's own credential**, not the instance-wide one
/// `github.agent_identity` names.
///
/// The two refusals are what tell them apart, and this instance is configured
/// so that they differ: personal share identity, no agent identity set, no
/// credential connected for anybody. An unauthenticated HTTP agent is
/// `ShareActor::HttpAgent` and gets the text naming the setting an admin must
/// write; ada's session is `ShareActor::Account("ada")` and gets the text
/// telling ada to connect her own GitHub identity. Asserting on "Connect yours
/// in Fluid" is therefore asserting on which actor reached the engine.
///
/// The agent identity is deliberately left unset and no token is seeded: that
/// is the only configuration in which the two actors say different things. With
/// a bot token on file the share would reach the forge, and with the identity
/// set but no token both actors would refuse identically.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_personal_share_by_an_authenticated_agent_resolves_that_accounts_credential() {
    let (addr, _guard, store) = serve_personal_share_with_mcp_auth().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let token = store.issue_mcp_token("ada", "t").await.unwrap().token;

    let session = McpTestSession::open(&addr, Some(&token)).await;
    let answer = session
        .call_tool("share_changes", serde_json::json!({ "domain": "kb" }))
        .await;
    assert!(
        answer.contains("Connect yours in Fluid"),
        "the share runs as ada, so it is ada's missing connection that refuses it:\n{answer}"
    );
    assert!(
        !answer.contains("no agent identity is configured"),
        "an authenticated session is never the configured agent identity:\n{answer}"
    );
}

/// **The account reaches a stateless modern-era call too**, where there is no
/// session in the picture at all.
///
/// A 2026-07-28 peer carries its own `_meta` per request and never handshakes,
/// so it routes through the transport's stateless branch and the identity has
/// to survive a different injection site (rmcp 3.2.0
/// `transport/streamable_http_server/tower.rs:1974`, against `:1775` for the
/// session POST the test above drives). This is the path a modern harness
/// actually takes, so the claim is pinned by the transport rather than by
/// source reading.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authenticated_modern_era_call_carries_the_account_with_no_session() {
    let (addr, guard, store) = serve_with_mcp_auth(true).await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let token = store.issue_mcp_token("ada", "t").await.unwrap().token;

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "write_engram",
            "arguments": {
                "domain": "eng",
                "title": "Stateless Trace",
                "content": "- [fact] traced",
            },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": ERA,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {
                    "name": "mcp-auth-test",
                    "version": "0.0.0"
                },
            },
        },
    })
    .to_string();
    let answer = raw_post(
        &addr,
        &body,
        // The SEP-2243 standard headers rmcp requires of any client declaring
        // this revision (`validate_standard_headers`).
        &[
            ("MCP-Protocol-Version", ERA),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "write_engram"),
        ],
        Some(&token),
    )
    .await;
    assert!(
        answer.contains("\"result\""),
        "the write must be served, not refused:\n{answer}"
    );
    let head = answer.split("\r\n\r\n").next().unwrap_or(&answer);
    assert!(
        !head.to_ascii_lowercase().contains("mcp-session-id"),
        "the modern era routes statelessly, so there is no session to hang an \
         identity on:\n{head}"
    );

    let written =
        std::fs::read_to_string(guard.path().join("eng").join("stateless-trace.md")).unwrap();
    assert!(
        written.contains("for-ada"),
        "the account reaches a call that never opened a session: {written}"
    );
}

/// **A client cannot spend the provenance budget and truncate the account off
/// the end.**
///
/// `clientInfo.name` is client-supplied and unbounded, while the actor a write
/// records is capped. Composing the two halves and sanitizing once would let a
/// long enough client name fill the cap and drop, or half-drop, the half the
/// server asserts - `...-for-ad`, or no account at all, with the write
/// succeeding either way and nothing to notice it. The account is measured
/// first instead, so it lands whole and the client half is what gets cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_client_name_is_cut_and_the_account_still_lands_whole() {
    let (addr, guard, store) = serve_with_mcp_auth(true).await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let token = store.issue_mcp_token("ada", "t").await.unwrap().token;

    let long = "c".repeat(200);
    let session = McpTestSession::open_as(&addr, Some(&token), &long).await;
    let answer = session
        .call_tool(
            "write_engram",
            serde_json::json!({
                "domain": "eng",
                "title": "Budget Trace",
                "content": "- [fact] traced",
            }),
        )
        .await;
    assert!(
        answer.contains("\"result\""),
        "the write must be served, not refused:\n{answer}"
    );

    let written =
        std::fs::read_to_string(guard.path().join("eng").join("budget-trace.md")).unwrap();
    assert!(
        written.contains("-for-ada,"),
        "the account survives whole, join and all: {written}"
    );
    assert!(
        written.contains("ccc"),
        "and the client half is still recorded, just cut: {written}"
    );
}

/// **A client cannot write the composed shape itself.**
///
/// With `auth.mcp` off - the default install - nobody authenticates, so the
/// composition never runs and `generated.by` is the client's own name. A client
/// naming itself `claude-code for ada` would otherwise land
/// `claude-code-for-ada` on disk, byte-identical to what an authenticated ada
/// session writes, which would make the whole `-for-` shape worthless as
/// evidence. The join is the server's word, so it is taken out of the client
/// half.
///
/// This also pins the auth-off HTTP tier at the real transport: the request
/// carries `http::request::Parts` and no `McpIdentity`, which is the shape the
/// duplex-transport tests elsewhere cannot produce.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unauthenticated_client_cannot_write_the_composed_shape() {
    let (addr, guard, _store) = serve_with_mcp_auth(false).await;

    // The plain attempt, the same attempt spelled so that deleting one join
    // would create another, the whitespace form the sanitizer folds into the
    // hyphenated one, and the capitalized one. None of them may land the
    // shape, whatever route it took to get here.
    for (index, client) in [
        "claude-code for ada",
        "x-for-for-ada",
        "x for for ada",
        "x-FOR-ada",
    ]
    .into_iter()
    .enumerate()
    {
        let title = format!("Forged Trace {index}");
        let session = McpTestSession::open_as(&addr, None, client).await;
        let answer = session
            .call_tool(
                "write_engram",
                serde_json::json!({
                    "domain": "eng",
                    "title": title,
                    "content": "- [fact] traced",
                }),
            )
            .await;
        assert!(
            answer.contains("\"result\""),
            "the open tier still serves the write for {client}:\n{answer}"
        );

        let written = std::fs::read_to_string(
            guard
                .path()
                .join("eng")
                .join(format!("forged-trace-{index}.md")),
        )
        .unwrap();
        assert!(
            !written.contains("-for-"),
            "the join is the server's word, not the client's ({client}): {written}"
        );
    }

    let first =
        std::fs::read_to_string(guard.path().join("eng").join("forged-trace-0.md")).unwrap();
    assert!(
        first.contains("claude-code"),
        "the rest of the name a client chose is still its own: {first}"
    );
}

/// **A client whose whole name is the join word composes as the stand-in**,
/// not as a bare account.
///
/// `for` sanitizes to itself and then loses its one segment, so there is no
/// client half left. The composition is always two halves - `agent for ada` -
/// because `ada` alone would read as a client calling itself ada, and would
/// drop the one fact the composition exists to record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_is_only_the_join_word_composes_as_the_stand_in() {
    let (addr, guard, store) = serve_with_mcp_auth(true).await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let token = store.issue_mcp_token("ada", "t").await.unwrap().token;

    let session = McpTestSession::open_as(&addr, Some(&token), "for").await;
    let answer = session
        .call_tool(
            "write_engram",
            serde_json::json!({
                "domain": "eng",
                "title": "Stand In Trace",
                "content": "- [fact] traced",
            }),
        )
        .await;
    assert!(
        answer.contains("\"result\""),
        "the write must be served, not refused:\n{answer}"
    );

    let written =
        std::fs::read_to_string(guard.path().join("eng").join("stand-in-trace.md")).unwrap();
    assert!(
        written.contains("agent-for-ada"),
        "an unnamed client still records that an agent acted for ada: {written}"
    );
}

// ---------------------------------------------------------------------------
// Private domains over MCP
// ---------------------------------------------------------------------------
//
// The gate says who a caller is; this half says what that entitles them to.
// Every read verb answers from the domains its caller may see and every write
// verb refuses what it may not change, and both hold over the wire rather than
// only in the engine (`tests/visibility.rs` is the engine's own leg).
//
// The property under all of it is the one the whole program is built on: a
// domain somebody may not see is answered exactly as a domain nobody
// registered, and an engram inside it exactly as an engram nobody wrote. Not
// "forbidden" - the existence of a private domain is the secret it keeps.
//
// The three tiers reach here as three scopes. A local stdio session is the
// machine owner and is unrestricted; an authenticated HTTP session is the
// account it authenticated as, with the instance role the gate resolved; an
// HTTP session on an instance with `auth.mcp` off is nobody in particular,
// which is the legacy open tier and now sees only what is shared.

const VIS_OPEN_MANIFEST: &str = "---\ntype: manifest\ntitle: open\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# open\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for shared questions\n";
const VIS_SECOND_MANIFEST: &str = "---\ntype: manifest\ntitle: second\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# second\n\n## Scope\n\n- The other shared domain\n\n## When to Use\n\n- Route here for second questions\n";
const VIS_LAB_MANIFEST: &str = "---\ntype: manifest\ntitle: lab\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# lab\n\n## Scope\n\n- The private domain\n\n## When to Use\n\n- Route here for confidential lab questions\n";
const VIS_OPEN_NOTE: &str = "---\ntype: engram\ntitle: Open Note\npermalink: open-note\ntags:\n  - shared\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Open Note\n\n- [decision] the shared thing is public #shared\n";
/// The private engram, written so a cross-domain move of `open-note` reaches
/// into it. The prefixed `[[open:Open Note]]` relation is what resolves, so
/// this file is one of the inbound references the move gathers; the bare
/// `[[Open Note]]` in its prose is what the rewrite would then replace, which
/// is the side effect the mover's scope has to bound. Its third link resolves
/// to nothing, so the maintenance sweep has something to find in `lab` and a
/// sweep that reached in would say so.
const VIS_LAB_NOTE: &str = "---\ntype: dossier\ntitle: Lab Note\npermalink: lab-note\ntags:\n  - confidential\nstatus: stable\nrecorded_at: 2026-01-03\n---\n\n# Lab Note\n\n- [secret] the secret formula is here #confidential\n- relates_to [[open:Open Note]]\n- relates_to [[Nothing Here At All]]\n\nSee also [[Open Note]] for the shared half.\n";
const VIS_LAB_ASSET: &str = "the attachment nobody outside lab may read\n";

/// A three-domain instance behind the production router: `open` and `second`
/// are shared, `lab` is private to `owner` with `mem` invited as a viewer.
///
/// `fault` installs a **broken** private-domain resolver before the router
/// builds one, which is the whole of the fail-closed injection. The engine's
/// resolver slot is a `OnceLock` (`Engine::set_domain_access`), so whoever
/// installs first wins and `http_base`'s own call becomes a no-op. The gate
/// keeps the healthy store, so a token still authenticates and the request
/// still reaches a tool; only the question "what may this caller see" is
/// unanswerable.
struct VisibilityCtx {
    addr: std::net::SocketAddr,
    tmp: tempfile::TempDir,
    store: Arc<AuthStore>,
    engine: Arc<Engine>,
}

impl VisibilityCtx {
    /// Invite `account` into `domain` at `level`.
    async fn add_member(
        &self,
        domain: &str,
        account: &str,
        level: crystalline_service::rest::MemberLevel,
    ) {
        self.store
            .upsert_domain_member(domain, account, level, "owner")
            .await
            .unwrap();
    }

    /// A live MCP token for `account`.
    async fn token_for(&self, account: &str) -> String {
        self.store
            .issue_mcp_token(account, "agent")
            .await
            .unwrap()
            .token
    }

    fn path(&self, domain: &str, file: &str) -> std::path::PathBuf {
        self.tmp.path().join(domain).join(file)
    }
}

/// Give `path` a `domain_acl` table of the wrong shape before the auth store
/// opens one, so every later read of it fails.
///
/// The store's schema statement is `CREATE TABLE IF NOT EXISTS`, so it leaves a
/// table that already exists alone whatever its columns are; `private_domains`
/// then selects columns that are not there and errors. Sequential by
/// construction - this connection is closed before `AuthStore::open` makes its
/// own - so nothing here depends on two connections sharing one file.
async fn break_the_visibility_table(path: &std::path::Path) {
    let name = path.to_string_lossy().to_string();
    let db = match turso::Builder::new_local(&name)
        .experimental_multiprocess_wal(true)
        .build()
        .await
    {
        Ok(db) => db,
        // The same fallback `AuthStore`'s own opener makes where this platform
        // has no shared WAL coordination.
        Err(_) => turso::Builder::new_local(&name).build().await.unwrap(),
    };
    let conn = db.connect().unwrap();
    conn.execute_batch("CREATE TABLE domain_acl (junk TEXT);")
        .await
        .unwrap();
}

async fn mcp_ctx(mcp_auth: bool) -> VisibilityCtx {
    mcp_ctx_with(mcp_auth, false, false).await
}

/// The same instance with `lab` also carrying a GitHub origin and
/// `github.enabled` on, which is what the collaboration verbs need before they
/// answer anything at all. Only `lab` is a team domain, so a caller that may
/// not see it has an empty target list and the aggregate verbs resolve no
/// provider and reach no network for it.
async fn mcp_team_ctx() -> VisibilityCtx {
    mcp_ctx_with(true, false, true).await
}

async fn mcp_ctx_with(mcp_auth: bool, fault: bool, team: bool) -> VisibilityCtx {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    for (name, manifest) in [
        ("open", VIS_OPEN_MANIFEST),
        ("second", VIS_SECOND_MANIFEST),
        ("lab", VIS_LAB_MANIFEST),
    ] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST.md"), manifest).unwrap();
        let entry = if team && name == "lab" {
            DomainEntry {
                origin: Some(OriginConfig {
                    repo: "acme/lab".to_string(),
                    path: None,
                    branch: None,
                    poll_secs: None,
                }),
                ..DomainEntry::file(dir)
            }
        } else {
            DomainEntry::file(dir)
        };
        cfg.domains.insert(name.to_string(), entry);
    }
    if team {
        cfg.github = Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        });
    }
    std::fs::write(root.join("open").join("open-note.md"), VIS_OPEN_NOTE).unwrap();
    std::fs::write(root.join("lab").join("lab-note.md"), VIS_LAB_NOTE).unwrap();
    std::fs::create_dir_all(root.join("lab").join("assets")).unwrap();
    std::fs::write(
        root.join("lab").join("assets").join("secret.txt"),
        VIS_LAB_ASSET,
    )
    .unwrap();
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
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            // An empty temp directory, so nothing here ever reads the developer's
            // real OS keychain looking for a GitHub credential.
            .with_token_store_dir(root.join("tokens")),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    for (name, role) in [
        ("owner", Role::Editor),
        ("mem", Role::Editor),
        ("out", Role::Editor),
        ("boss", Role::Admin),
        ("looker", Role::Viewer),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("lab", true, "owner")
        .await
        .unwrap();

    if fault {
        let broken_path = root.join("broken-auth.db");
        break_the_visibility_table(&broken_path).await;
        let broken = Arc::new(AuthStore::open(&broken_path).await.unwrap());
        engine.set_domain_access(Arc::new(crystalline_service::DomainAccess::new(broken)));
    }

    let router = http_router(
        engine.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
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
    VisibilityCtx {
        addr,
        tmp,
        store: auth,
        engine,
    }
}

/// One modern-era POST, optionally authenticated: the era's `_meta` in the body
/// and the SEP-2243 standard headers beside it, which is the shape that reaches
/// the transport with no session in the picture at all.
async fn era_post(
    addr: &std::net::SocketAddr,
    id: u32,
    method: &str,
    params: serde_json::Value,
    token: Option<&str>,
) -> String {
    let name = params
        .get("name")
        .or_else(|| params.get("uri"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut params = params;
    params["_meta"] = serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "mcp-auth-test", "version": "0.0.0" },
    });
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    let mut headers: Vec<(&str, &str)> =
        vec![("MCP-Protocol-Version", ERA), ("Mcp-Method", method)];
    if let Some(name) = name.as_deref() {
        headers.push(("Mcp-Name", name));
    }
    raw_post(addr, &body, &headers, token).await
}

/// **The legacy open tier sees only what is shared.**
///
/// With `auth.mcp` off the gate is a pass-through and there is nobody to be, so
/// every agent reaching the endpoint is the anonymous tier: it reads the shared
/// domains and a private one is simply not there. That is the one behaviour
/// change a default install can notice, and it only happens once somebody has
/// made a domain private - on an installation where nobody has, the listing is
/// byte-identical to its old self.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_legacy_open_tier_sees_only_non_private_domains() {
    let ctx = mcp_ctx(false).await;
    let session = McpTestSession::open(&ctx.addr, None).await;

    let listed = session
        .call_tool("list_domains", serde_json::json!({}))
        .await;
    assert!(
        listed.contains("open") && listed.contains("second"),
        "the shared domains are listed:\n{listed}"
    );
    assert!(!listed.contains("lab"), "the private one is not:\n{listed}");

    let browsed = session
        .call_tool("browse_domain", serde_json::json!({ "domain": "lab" }))
        .await;
    assert!(
        browsed.contains("not registered"),
        "and naming it answers exactly as naming an unregistered domain:\n{browsed}"
    );
}

/// **An invited member reads its private domain over MCP, and a viewer's
/// membership is not a licence to write it.**
///
/// The two halves are one test because they are one decision read at two rungs
/// of the same ladder: `mem` may see `lab` (so it is in the listing and its
/// engrams are readable) and holds `viewer` on it (so a write is refused, and
/// the refusal names the level, because "forbidden" on a domain the caller can
/// see and read is otherwise indistinguishable from a bug).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authenticated_member_reads_its_private_domain_over_mcp() {
    let ctx = mcp_ctx(true).await;
    ctx.add_member("lab", "mem", crystalline_service::rest::MemberLevel::Viewer)
        .await;
    let token = ctx.token_for("mem").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;

    let listed = session
        .call_tool("list_domains", serde_json::json!({}))
        .await;
    assert!(
        listed.contains("lab"),
        "an invited member sees the domain it was invited to:\n{listed}"
    );

    let read = session
        .call_tool(
            "read_engram",
            serde_json::json!({ "identifier": "lab-note", "domain": "lab" }),
        )
        .await;
    assert!(
        read.contains("secret formula"),
        "and reads what is in it:\n{read}"
    );

    let refused = session
        .call_tool(
            "write_engram",
            serde_json::json!({ "domain": "lab", "title": "Nope", "content": "x" }),
        )
        .await;
    assert!(
        refused.contains("viewer"),
        "the level is named in the refusal:\n{refused}"
    );
    assert!(
        !ctx.path("lab", "nope.md").exists(),
        "and nothing was written"
    );
}

/// **A stranger is answered as though the domain did not exist**, on every
/// shape of question: the index, a named domain, and an engram named by its
/// absolute address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stranger_reaches_nothing_of_a_private_domain_over_mcp() {
    let ctx = mcp_ctx(true).await;
    let token = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;

    let listed = session
        .call_tool("list_domains", serde_json::json!({}))
        .await;
    assert!(!listed.contains("lab"), "absent from the index:\n{listed}");

    // The same call with the routing bullets, which is what every pointer on
    // this server sends a client to - the count line the HTTP handshake now
    // carries, the connector snippet and the shipped skills all name it. If
    // this branch were unfiltered the handshake's degradation would have moved
    // the disclosure one call deeper rather than closed it.
    let routed = session
        .call_tool(
            "list_domains",
            serde_json::json!({ "include_routing": true }),
        )
        .await;
    assert!(
        routed.contains("Route here for shared questions"),
        "a stranger still gets its own routing index:\n{routed}"
    );
    assert!(
        !routed.contains("lab") && !routed.contains("confidential lab questions"),
        "with no line and no bullet of the domain it may not see:\n{routed}"
    );

    let searched = session
        .call_tool(
            "search_engrams",
            serde_json::json!({ "query": "secret formula" }),
        )
        .await;
    assert!(
        !searched.contains("secret formula"),
        "absent from search:\n{searched}"
    );

    let read = session
        .call_tool(
            "read_engram",
            serde_json::json!({ "identifier": "crystalline://lab/lab-note" }),
        )
        .await;
    assert!(
        read.contains("no engram") && !read.contains("secret formula"),
        "and its absolute address answers as an engram nobody wrote:\n{read}"
    );
}

/// **An absolute identifier cannot carry a write into a domain the caller may
/// not see.**
///
/// A `crystalline://` URL is the one identifier form that overrides the domain
/// argument, so a gate reading the argument alone would gate the wrong domain:
/// a caller who may write `open` names `crystalline://lab/lab-note` and reaches
/// an engram it was never invited to. The refusal is the missing-engram one,
/// and the file is proof it was a refusal rather than a partial move.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_absolute_identifier_cannot_write_into_a_hidden_domain() {
    let ctx = mcp_ctx(true).await;
    let token = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;
    let before = std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap();

    for (tool, arguments) in [
        (
            "move_engram",
            serde_json::json!({
                "identifier": "crystalline://lab/lab-note",
                "domain": "open",
                "destination": "stolen.md",
                "destination_domain": "open",
            }),
        ),
        (
            "delete_engram",
            serde_json::json!({
                "identifier": "crystalline://lab/lab-note",
                "domain": "open",
            }),
        ),
        (
            "edit_engram",
            serde_json::json!({
                "identifier": "crystalline://lab/lab-note",
                "domain": "open",
                "operation": "append",
                "content": "- [fact] injected\n",
            }),
        ),
    ] {
        let answer = session.call_tool(tool, arguments).await;
        assert!(
            answer.contains("no engram") && !answer.contains("secret formula"),
            "{tool} must answer as a missing engram:\n{answer}"
        );
    }

    assert_eq!(
        std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap(),
        before,
        "the private engram is byte-for-byte what it was"
    );
    assert!(!ctx.path("open", "stolen.md").exists());
}

/// **An instance viewer's agent cannot write, on any domain.**
///
/// An MCP token is issued to an account, so an agent holding one acts with that
/// account's instance role. A viewer whose agent could write over MCP what the
/// same viewer cannot write over the JSON API would be an escalation path
/// around the role system rather than a convenience.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_instance_viewers_agent_is_refused_and_an_admins_is_not() {
    let ctx = mcp_ctx(true).await;

    let viewer = ctx.token_for("looker").await;
    let session = McpTestSession::open(&ctx.addr, Some(&viewer)).await;
    let refused = session
        .call_tool(
            "write_engram",
            serde_json::json!({ "domain": "open", "title": "Nope", "content": "x" }),
        )
        .await;
    assert!(
        refused.contains("viewer"),
        "a viewer's agent is refused, and told what it holds:\n{refused}"
    );

    // An admin resolves to owner on every domain, private ones included.
    let admin = ctx.token_for("boss").await;
    let session = McpTestSession::open(&ctx.addr, Some(&admin)).await;
    let written = session
        .call_tool(
            "write_engram",
            serde_json::json!({ "domain": "lab", "title": "Admin Note", "content": "- [fact] yes" }),
        )
        .await;
    assert!(
        written.contains("admin-note"),
        "an admin writes the private domain, and the receipt names what it \
         wrote rather than merely arriving:\n{written}"
    );
    assert!(ctx.path("lab", "admin-note.md").exists());
}

/// **A cross-domain move rewrites links only where the mover can see.**
///
/// Moving an engram between domains rewrites every bare `[[target]]` that
/// pointed at it into the prefixed form, and those linking engrams were not
/// written by whoever asked for the move. A linking engram in a domain the
/// mover may not see is therefore left exactly as it was - dangling, which its
/// own members see as an unresolved-reference finding on their next sweep -
/// rather than silently edited by somebody with no access to it, and the
/// receipt counts only the rewrites its reader may know about.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_strangers_move_leaves_a_hidden_domains_link_alone() {
    let ctx = mcp_ctx(true).await;
    let token = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;
    let before = std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap();

    let moved = session
        .call_tool(
            "move_engram",
            serde_json::json!({
                "identifier": "open-note",
                "domain": "open",
                "destination": "open-note.md",
                "destination_domain": "second",
            }),
        )
        .await;
    assert!(
        moved.contains("cross_domain\\\":true"),
        "the move itself runs:\n{moved}"
    );
    assert!(
        moved.contains("links_rewritten\\\":0"),
        "and counts no rewrite it may not report:\n{moved}"
    );
    assert!(
        !moved.contains("lab"),
        "the receipt names no hidden domain:\n{moved}"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap(),
        before,
        "the private engram's link is byte-for-byte what it was"
    );
}

/// The converse, so the skip above is the scope rather than a broken rewrite:
/// an admin sees every domain, so the same move rewrites the same link.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admins_move_rewrites_the_private_domains_link() {
    let ctx = mcp_ctx(true).await;
    let token = ctx.token_for("boss").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;

    let moved = session
        .call_tool(
            "move_engram",
            serde_json::json!({
                "identifier": "open-note",
                "domain": "open",
                "destination": "open-note.md",
                "destination_domain": "second",
            }),
        )
        .await;
    assert!(
        moved.contains("links_rewritten\\\":1"),
        "an admin's move rewrites it:\n{moved}"
    );
    assert!(
        std::fs::read_to_string(ctx.path("lab", "lab-note.md"))
            .unwrap()
            .contains("[[second:Open Note]]"),
        "and the link is prefixed"
    );
}

/// **The sweep and the schema verbs are scoped too.**
///
/// `evolve_engrams` sweeps every domain when it is given none, and
/// `validate_engrams` and `infer_schema` each take a domain by name. All three
/// reach the store directly rather than through a read verb, so each needed the
/// caller's scope of its own; a queue of work in a domain the caller cannot see
/// would name its engrams, its paths and its rules.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sweep_and_schema_verbs_answer_nothing_for_a_hidden_domain() {
    let ctx = mcp_ctx(true).await;
    let token = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;

    let swept = session
        .call_tool("evolve_engrams", serde_json::json!({ "limit": 50 }))
        .await;
    assert!(
        !swept.contains("lab-note") && !swept.contains("Nothing Here At All"),
        "an unscoped sweep stops at the domains this caller may see:\n{swept}"
    );

    for tool in ["validate_engrams", "infer_schema"] {
        let answer = session
            .call_tool(
                tool,
                serde_json::json!({ "domain": "lab", "type": "dossier" }),
            )
            .await;
        assert!(
            answer.contains("not registered"),
            "{tool} refuses a hidden domain as an unregistered one:\n{answer}"
        );
        assert!(!answer.contains("confidential"), "{tool}:\n{answer}");
    }

    let named = session
        .call_tool(
            "evolve_engrams",
            serde_json::json!({ "domains": ["lab"], "limit": 50 }),
        )
        .await;
    assert!(
        !named.contains("Nothing Here At All"),
        "and naming it directly finds nothing:\n{named}"
    );

    // The same holds for every other verb that reaches a domain by name.
    // `provision` is the one of those that needs no collaboration setting to
    // reach its gate, so it stands for the set here; deciding about a domain is
    // itself a way of asking whether it exists.
    let decided = session
        .call_tool(
            "provision",
            serde_json::json!({ "action": "allow", "domain": "lab" }),
        )
        .await;
    assert!(
        decided.contains("not registered"),
        "provision refuses a hidden domain as an unregistered one:\n{decided}"
    );
}

/// **An attachment is reachable only inside a domain the caller may see.**
///
/// `resources/read` is the one attachment path a caller reaches cold: the uri
/// names its domain outright and nothing had to be read first to learn the
/// name, so a client can ask for any domain's file without ever touching a
/// scoped verb.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attachment_in_a_hidden_domain_is_not_readable() {
    let ctx = mcp_ctx(true).await;
    let uri = "crystalline://lab/assets/secret.txt";

    let stranger = ctx.token_for("out").await;
    let refused = era_post(
        &ctx.addr,
        1,
        "resources/read",
        serde_json::json!({ "uri": uri }),
        Some(&stranger),
    )
    .await;
    assert!(
        !refused.contains("nobody outside lab"),
        "a stranger never gets the bytes:\n{refused}"
    );
    assert!(
        refused.contains("not registered"),
        "and is answered as though the domain were not registered:\n{refused}"
    );

    ctx.add_member("lab", "mem", crystalline_service::rest::MemberLevel::Viewer)
        .await;
    let member = ctx.token_for("mem").await;
    let served = era_post(
        &ctx.addr,
        2,
        "resources/read",
        serde_json::json!({ "uri": uri }),
        Some(&member),
    )
    .await;
    assert!(
        served.contains("nobody outside lab"),
        "a member does:\n{served}"
    );
    assert!(
        served.contains("\\\"cacheScope\\\":\\\"private\\\"")
            || served.contains("\"cacheScope\":\"private\""),
        "and the answer is cached per caller rather than shared, since it \
         depends on who asked:\n{served}"
    );
}

/// **`server/discover` hands each modern peer its own index.**
///
/// The era deletes `initialize` outright and moves `instructions` to
/// `DiscoverResult`, so this is the modern client's only onboarding channel -
/// and unlike the legacy handshake it carries a request context, so it is
/// scoped per caller rather than degraded for everybody.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_onboards_each_caller_with_the_domains_it_may_see() {
    let ctx = mcp_ctx(true).await;
    ctx.add_member("lab", "mem", crystalline_service::rest::MemberLevel::Viewer)
        .await;

    let stranger = ctx.token_for("out").await;
    let theirs = era_post(
        &ctx.addr,
        1,
        "server/discover",
        serde_json::json!({}),
        Some(&stranger),
    )
    .await;
    assert!(
        theirs.contains("CRYSTALLINE KNOWLEDGE ROUTING") && theirs.contains("- open:"),
        "a stranger is still onboarded:\n{theirs}"
    );
    assert!(
        !theirs.contains("- lab:") && !theirs.contains("confidential lab questions"),
        "with no bullet of the domain it may not see:\n{theirs}"
    );

    let member = ctx.token_for("mem").await;
    let mine = era_post(
        &ctx.addr,
        2,
        "server/discover",
        serde_json::json!({}),
        Some(&member),
    )
    .await;
    assert!(
        mine.contains("confidential lab questions"),
        "a member is onboarded into it:\n{mine}"
    );

    // The `onboarding` prompt is the same block by another channel, and it
    // carries a request context too, so it is scoped rather than degraded.
    let prompted = era_post(
        &ctx.addr,
        3,
        "prompts/get",
        serde_json::json!({ "name": "onboarding" }),
        Some(&stranger),
    )
    .await;
    assert!(
        prompted.contains("CRYSTALLINE KNOWLEDGE ROUTING"),
        "the prompt answers:\n{prompted}"
    );
    assert!(
        !prompted.contains("confidential lab questions"),
        "and drops the bullets of a domain this caller may not see:\n{prompted}"
    );
}

/// **The legacy handshake over HTTP names no domain at all.**
///
/// `get_info` is synchronous and rmcp calls it with no request context, so this
/// one channel cannot know who is connecting and cannot leave a private
/// domain's bullets out of a per-caller block. It therefore carries every
/// behavior rule, the count of registered domains and the pointer at
/// `list_domains` - which does resolve a caller and does filter - and no name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_http_handshake_carries_the_rules_and_no_domain_name() {
    let ctx = mcp_ctx(true).await;
    let token = ctx.token_for("out").await;
    let handshake = raw_post(&ctx.addr, &initialize_body(), &[], Some(&token)).await;

    assert!(
        handshake.contains("CRYSTALLINE KNOWLEDGE ROUTING") && handshake.contains("Behavior:"),
        "the rules still arrive:\n{handshake}"
    );
    assert!(
        handshake.contains("3 domains registered"),
        "with the count line:\n{handshake}"
    );
    assert!(
        !handshake.contains("confidential lab questions")
            && !handshake.contains("Route here for shared questions"),
        "and no domain's routing bullets:\n{handshake}"
    );
}

/// **A resolver that cannot answer refuses; it never widens.**
///
/// Every scoped read holds one set of hidden domains for the whole call, read
/// from the accounts database. The failure mode worth testing is not the one
/// where that read says "nothing is hidden" - it is the one where it cannot
/// say anything at all, because an empty answer and an unanswerable question
/// look the same to a `unwrap_or_default`. Here the resolver's own table is
/// unreadable while the door is fine, so a call authenticates, reaches a tool,
/// and has to decide what to do with a question it cannot answer.
///
/// It answers nothing. Not the unfiltered list, not a partial one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scoped_read_refuses_when_the_resolver_cannot_answer() {
    let ctx = mcp_ctx_with(true, true, false).await;
    let token = ctx.token_for("boss").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;

    for (tool, arguments) in [
        ("list_domains", serde_json::json!({})),
        ("browse_domain", serde_json::json!({ "domain": "open" })),
        ("search_engrams", serde_json::json!({ "query": "shared" })),
        (
            "read_engram",
            serde_json::json!({ "identifier": "open-note", "domain": "open" }),
        ),
    ] {
        let answer = session.call_tool(tool, arguments).await;
        assert!(
            answer.contains("\"error\""),
            "{tool} must refuse rather than answer:\n{answer}"
        );
        for leaked in ["lab", "open-note", "the shared thing is public"] {
            assert!(
                !answer.contains(leaked),
                "{tool} leaked '{leaked}' out of a read it could not scope:\n{answer}"
            );
        }
    }

    // And the engine agrees one layer down, so the refusal is the resolver's
    // rather than a happy accident of one verb's error handling.
    assert!(
        ctx.engine
            .hidden_domains(&crystalline_service::Scope::User {
                account: "boss".to_string(),
                admin: true,
            })
            .await
            .is_err(),
        "the resolver itself is what is failing, for the very scope those \
         calls were made under"
    );
}

/// **`provision` with `action: "status"` enumerates, so it is scoped like every
/// other listing.**
///
/// `status` walks every registered domain and reports one entry per domain
/// whether or not that domain declares a `## Provisioning` section, so its
/// `domains` array is a complete list of names - the same disclosure
/// `list_domains` exists to prevent, through a verb nobody would look at twice.
/// It is a pure read and stays allowed on a read-only instance, so the answer
/// is to narrow what it may see rather than to refuse it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_status_lists_only_the_domains_a_caller_may_see() {
    let ctx = mcp_ctx(true).await;
    ctx.add_member("lab", "mem", crystalline_service::rest::MemberLevel::Viewer)
        .await;

    let status = serde_json::json!({ "action": "status" });

    let stranger = ctx.token_for("out").await;
    let theirs = McpTestSession::open(&ctx.addr, Some(&stranger))
        .await
        .call_tool("provision", status.clone())
        .await;
    assert!(
        theirs.contains("open") && theirs.contains("second"),
        "a stranger still gets a real report:\n{theirs}"
    );
    assert!(
        !theirs.contains("lab"),
        "with no entry for the domain it may not see:\n{theirs}"
    );

    for (who, token) in [
        ("mem", ctx.token_for("mem").await),
        ("boss", ctx.token_for("boss").await),
    ] {
        let mine = McpTestSession::open(&ctx.addr, Some(&token))
            .await
            .call_tool("provision", status.clone())
            .await;
        assert!(
            mine.contains("lab"),
            "{who} may see lab, so its status names it:\n{mine}"
        );
    }
}

/// The same, one tier down: with `auth.mcp` off there is nobody to be, and the
/// open tier must not be the way around the filter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_status_hides_a_private_domain_from_the_open_tier_too() {
    let ctx = mcp_ctx(false).await;
    let report = McpTestSession::open(&ctx.addr, None)
        .await
        .call_tool("provision", serde_json::json!({ "action": "status" }))
        .await;
    assert!(report.contains("open"), "the report is real:\n{report}");
    assert!(
        !report.contains("lab"),
        "and names no private domain:\n{report}"
    );
}

/// **The aggregate collaboration verbs answer over the caller's own domains.**
///
/// `origin_status` and `update_domain` both take an optional domain, and with
/// none they sweep every registered domain that carries an origin. That
/// aggregate form is where a private team domain's name, its open proposals and
/// its conflicts would reach a stranger - and `update_domain` would go further
/// and pull into it, a write inside a domain the caller is not a member of.
///
/// Only `lab` carries an origin here, so a caller that may not see it has an
/// empty target list: nothing resolves a provider and nothing reaches the
/// network, which is also what keeps this test offline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_aggregate_origin_verbs_answer_over_visible_domains_only() {
    let ctx = mcp_team_ctx().await;
    ctx.add_member("lab", "mem", crystalline_service::rest::MemberLevel::Viewer)
        .await;
    let before = std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap();

    let stranger = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&stranger)).await;
    let status = session
        .call_tool("origin_status", serde_json::json!({}))
        .await;
    assert!(
        status.contains("connection"),
        "the verb answers rather than refusing:\n{status}"
    );
    assert!(
        !status.contains("lab") && !status.contains("acme"),
        "and names no team domain this caller may not see:\n{status}"
    );

    let pulled = session
        .call_tool("update_domain", serde_json::json!({}))
        .await;
    assert!(
        !pulled.contains("lab"),
        "the sweep pull names none of it either:\n{pulled}"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap(),
        before,
        "and nothing was pulled into a domain the caller is no member of"
    );

    // Naming it directly is answered as an unregistered domain, and the
    // registered set the error names is filtered too.
    let named = session
        .call_tool("origin_status", serde_json::json!({ "domain": "lab" }))
        .await;
    assert!(
        named.contains("not registered") && !named.contains("acme"),
        "a named hidden team domain is refused as unregistered:\n{named}"
    );

    for (who, token) in [
        ("mem", ctx.token_for("mem").await),
        ("boss", ctx.token_for("boss").await),
    ] {
        let mine = McpTestSession::open(&ctx.addr, Some(&token))
            .await
            .call_tool("origin_status", serde_json::json!({}))
            .await;
        assert!(
            mine.contains("lab"),
            "{who} may see lab, so the sweep reports it:\n{mine}"
        );
    }
}

/// **A call whose two halves disagree is refused before it can name anything.**
///
/// An absolute `crystalline://` identifier and a different `domain` argument
/// used to send the gate and the engine to two different domains: the gate
/// checked the identifier's, the engine used the argument's, and the argument
/// was never checked at all. Reaching the unscoped source lookup behind it
/// answers with the whole registered set, private domains included - a full
/// enumeration out of a write the caller was never entitled to make.
///
/// Both halves are now the named domain, and the engine refuses the mismatch
/// itself, so neither the registered set nor a path inside a hidden domain
/// comes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mismatched_identifier_and_domain_names_no_other_domain() {
    let ctx = mcp_ctx(true).await;
    let token = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;

    // An unregistered domain argument: the error must not enumerate.
    let ghost = session
        .call_tool(
            "move_engram",
            serde_json::json!({
                "identifier": "crystalline://open/open-note",
                "domain": "ghost",
                "destination": "y.md",
            }),
        )
        .await;
    assert!(
        ghost.contains("not registered"),
        "the call is refused at the domain check rather than somewhere earlier, \
         which is what makes the assertion below say anything:\n{ghost}"
    );
    assert!(
        !ghost.contains("lab"),
        "and the registered set it names is the caller's own:\n{ghost}"
    );

    // A hidden domain argument: answered as an unregistered one, with no
    // path-existence oracle inside it.
    let hidden = session
        .call_tool(
            "move_engram",
            serde_json::json!({
                "identifier": "crystalline://open/open-note",
                "domain": "lab",
                "destination": "lab-note.md",
            }),
        )
        .await;
    assert!(
        hidden.contains("not registered"),
        "a hidden domain argument is the unregistered answer:\n{hidden}"
    );
    assert!(
        !hidden.contains("already exists"),
        "and never a report about what is inside it:\n{hidden}"
    );
    assert!(
        std::fs::read_to_string(ctx.path("open", "open-note.md")).is_ok(),
        "nothing moved"
    );
}

/// **A padded domain name is the same call, and it is answered the same way.**
///
/// `destination_domain` is a plain string with no normalizer between the
/// surface and the verb, so `"open "` and `"open"` are two different domains as
/// far as the engine is concerned and one of them is registered. A gate that
/// trimmed it would pass the call (the trimmed spelling is a domain this caller
/// may write) and hand the untrimmed one to a lookup that answers with every
/// registered domain on the instance, private ones included - a full
/// enumeration out of one trailing space.
///
/// The gate reads the field the way the verb reads it, and the lookup behind it
/// is scoped, so the answer is the ordinary unregistered-domain answer over the
/// caller's own visible set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_padded_destination_domain_is_answered_like_a_missing_one() {
    let ctx = mcp_ctx(true).await;
    // A stranger with nothing but its instance role: it may write `open`, and
    // `lab` does not exist for it.
    let token = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&token)).await;

    let padded = session
        .call_tool(
            "move_engram",
            serde_json::json!({
                "identifier": "open-note",
                "domain": "open",
                "destination": "y.md",
                "destination_domain": "open ",
            }),
        )
        .await;

    // The control: a name nobody registered, asked by the same caller in the
    // same session. The padded spelling has to answer exactly this.
    let missing = session
        .call_tool(
            "move_engram",
            serde_json::json!({
                "identifier": "open-note",
                "domain": "open",
                "destination": "y.md",
                "destination_domain": "ghost",
            }),
        )
        .await;
    assert!(
        missing.contains("not registered") && !missing.contains("lab"),
        "the control is the unregistered answer over the visible set:\n{missing}"
    );

    assert!(
        padded.contains("not registered"),
        "a padded name is refused as unregistered:\n{padded}"
    );
    assert!(
        !padded.contains("lab"),
        "and never names the domain this caller may not see:\n{padded}"
    );
    // And the whole of it, not only the two strings the assertions above pick
    // out: the JSON-RPC payload of the padded call is the payload of the
    // missing-name call with the name substituted, so there is nothing else in
    // it either. The payload rather than the raw response, because the frame
    // around it carries a per-response SSE event id that is not what this test
    // is about.
    let payload = |raw: &str| {
        raw.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap_or_else(|| panic!("no JSON-RPC payload in:\n{raw}"))
            .to_string()
    };
    assert_eq!(
        payload(&padded).replace("open ", "ghost"),
        payload(&missing),
        "a padded name is answered exactly as a name nobody registered"
    );
    assert!(
        std::fs::read_to_string(ctx.path("open", "open-note.md")).is_ok()
            && !ctx.path("open", "y.md").exists(),
        "and nothing moved"
    );
}

/// The auth-off leg of
/// [`the_aggregate_origin_verbs_answer_over_visible_domains_only`]: with
/// `auth.mcp` off there is nobody to be, and the tier that has no accounts is
/// the one where a filter regression is least likely to be noticed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_aggregate_origin_verbs_hide_a_private_team_domain_from_the_open_tier() {
    let ctx = mcp_ctx_with(false, false, true).await;
    let before = std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap();
    let session = McpTestSession::open(&ctx.addr, None).await;

    let status = session
        .call_tool("origin_status", serde_json::json!({}))
        .await;
    assert!(
        status.contains("connection"),
        "the verb answers rather than refusing:\n{status}"
    );
    assert!(
        !status.contains("lab") && !status.contains("acme"),
        "and names no private team domain:\n{status}"
    );

    let pulled = session
        .call_tool("update_domain", serde_json::json!({}))
        .await;
    assert!(
        !pulled.contains("lab"),
        "the sweep pull names none of it either:\n{pulled}"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.path("lab", "lab-note.md")).unwrap(),
        before,
        "and nothing was pulled into it"
    );
}

/// The forward-auth headers open no door here. `auth.mcp` is the locked rule
/// that every HTTP agent connection authenticates like a human user, and
/// `auth.proxy_headers` is a browser-facing mode: the gate resolves personal
/// MCP tokens and reads nothing else, so a handshake carrying the whole
/// quartet is refused in the identical words a bare one is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_headers_are_no_way_past_the_mcp_gate() {
    let (addr, _tmp, _store) = serve_with_mcp_auth_and(true, true).await;
    let refused = reqwest::Client::new()
        .post(format!("http://{addr}/"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("Remote-User", "ada")
        .header("Remote-Name", "Ada Lovelace")
        .header("Remote-Email", "ada@example.test")
        .header("Remote-Groups", "admins")
        .body(initialize_body())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401, "an agent still authenticates");
    assert_eq!(refused.headers()["www-authenticate"], "Bearer");

    let bare = post_initialize(&addr, None).await;
    assert_eq!(bare.status(), 401);
    assert_eq!(
        refused.text().await.unwrap(),
        bare.text().await.unwrap(),
        "and is told the same thing a caller with no header at all is"
    );
}

// --- removing a domain, and the instance-state gate around it ---------------

/// The JSON-RPC payload out of one raw SSE response.
fn payload_of(raw: &str) -> serde_json::Value {
    let line = raw
        .lines()
        .find(|l| l.starts_with("data: "))
        .unwrap_or_else(|| panic!("no SSE data line in:\n{raw}"));
    serde_json::from_str(line.trim_start_matches("data: ")).unwrap()
}

/// Assert that `raw` carries a refusal the CLIENT renders - a `CallToolResult`
/// with `isError` - rather than a JSON-RPC protocol error, which rmcp's own
/// guidance says clients render opaquely.
///
/// This is the assertion the text-only checks beside it cannot make: the raw
/// frame carries the message in either shape, so `contains("admin")` passes
/// whichever one the handler chose. It matters because the whole content of
/// this refusal is teaching text naming who can end the domain, and both
/// shipped skills tell an agent to relay it rather than retry.
fn refusal_is_readable(raw: &str, label: &str) {
    let payload = payload_of(raw);
    assert_eq!(
        payload["result"]["isError"],
        serde_json::json!(true),
        "{label} must come back as a tool error the model reads, not a protocol error:\n{raw}"
    );
}

/// Whether `answer` still lists `domain` for the machine owner.
///
/// Read through the engine rather than through a second MCP call, so the
/// assertion is about what the instance holds rather than about what one scope
/// is shown.
async fn still_registered(ctx: &VisibilityCtx, domain: &str) -> bool {
    ctx.engine
        .list_domains(
            &crystalline_service::params::ListDomainsParams::default(),
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap()
        .to_string()
        .contains(&format!("\"{domain}\""))
}

/// **A private domain's owner unregisters it; a manager does not.**
///
/// The spec's locked rule, at the rung it is decided on: the owner of a private
/// domain and an instance admin may remove it, and every level below - manager
/// included - is refused with text naming who can. A manager may invite people
/// into the domain and may write it; ending the domain is not a membership
/// decision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_private_domains_owner_removes_it_and_a_manager_cannot() {
    let ctx = mcp_ctx(true).await;
    ctx.add_member(
        "lab",
        "mem",
        crystalline_service::rest::MemberLevel::Manager,
    )
    .await;

    let manager = ctx.token_for("mem").await;
    let session = McpTestSession::open(&ctx.addr, Some(&manager)).await;
    let refused = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "lab" }))
        .await;
    assert!(
        refused.contains("admin") && refused.contains("owner"),
        "the refusal names who can remove it:\n{refused}"
    );
    refusal_is_readable(&refused, "a manager's removal refusal");
    assert!(
        still_registered(&ctx, "lab").await,
        "and the manager removed nothing"
    );

    let owner = ctx.token_for("owner").await;
    let session = McpTestSession::open(&ctx.addr, Some(&owner)).await;
    let removed = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "lab" }))
        .await;
    assert!(
        removed.contains("unregistered"),
        "the owner's removal lands:\n{removed}"
    );
    assert!(
        !still_registered(&ctx, "lab").await,
        "and the domain is gone"
    );
    assert!(
        ctx.path("lab", "lab-note.md").exists(),
        "a file domain's files stay on disk"
    );
}

/// **A shared domain is an admin's to remove and nobody else's.**
///
/// There is no owner concept on a shared domain and none is invented here: the
/// same account that owns `lab` is an ordinary instance editor on `open`, and
/// is refused there. The admin is allowed, which is what keeps the refusal from
/// passing vacuously.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_domain_is_removed_by_an_admin_and_by_nobody_else() {
    let ctx = mcp_ctx(true).await;

    let editor = ctx.token_for("owner").await;
    let session = McpTestSession::open(&ctx.addr, Some(&editor)).await;
    let refused = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "open" }))
        .await;
    assert!(
        refused.contains("admin"),
        "an editor on a shared domain is refused, naming who can:\n{refused}"
    );
    refusal_is_readable(&refused, "an editor's removal refusal on a shared domain");
    assert!(still_registered(&ctx, "open").await, "nothing was removed");

    let boss = ctx.token_for("boss").await;
    let session = McpTestSession::open(&ctx.addr, Some(&boss)).await;
    let removed = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "open" }))
        .await;
    assert!(
        removed.contains("unregistered"),
        "an instance admin removes it:\n{removed}"
    );
    assert!(!still_registered(&ctx, "open").await, "and it is gone");
}

/// **A domain this caller may not see is not removable, and the refusal says
/// only that it is not registered.**
///
/// The gate would otherwise be an existence oracle: "you may not remove that"
/// tells a stranger the domain is there. The two answers are compared against
/// each other, so a later change that words one of them differently fails here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_hidden_domain_answers_exactly_as_removing_an_absent_one() {
    let ctx = mcp_ctx(true).await;
    let stranger = ctx.token_for("out").await;
    let session = McpTestSession::open(&ctx.addr, Some(&stranger)).await;

    let hidden = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "lab" }))
        .await;
    assert!(
        hidden.contains("not registered"),
        "a hidden domain is answered as an unregistered one:\n{hidden}"
    );
    let absent = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "nowhere" }))
        .await;
    // Compared as the JSON-RPC error message rather than as whole responses:
    // the SSE frame around it carries a per-connection event id and a chunk
    // length, which differ between two separate requests and say nothing about
    // what either caller was told.
    let message = |raw: &str| {
        payload_of(raw)["error"]["message"]
            .as_str()
            .unwrap_or_default()
            // The quoted name only: a bare substring swap would silently
            // mangle the comparison the day a fixture domain contains "lab".
            .replace("'lab'", "'nowhere'")
    };
    assert_eq!(
        message(&hidden),
        message(&absent),
        "the two refusals differ only in the name that was asked for"
    );
    assert!(
        still_registered(&ctx, "lab").await,
        "and the private domain is untouched"
    );
}

/// **The open tier removes nothing.**
///
/// With `auth.mcp` off an HTTP agent is nobody in particular, and unregistering
/// a domain is not something nobody in particular does. This is the one place
/// the open tier's carve-out does NOT apply: `add_domain` and `configure` keep
/// answering it exactly as they always did (the next test), because that is
/// what a default install already relies on, while removal is new and arrives
/// closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_open_tier_cannot_remove_a_domain() {
    let ctx = mcp_ctx(false).await;
    let session = McpTestSession::open(&ctx.addr, None).await;
    let refused = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "open" }))
        .await;
    assert!(
        refused.contains("admin"),
        "the open tier is refused, naming who can:\n{refused}"
    );
    refusal_is_readable(&refused, "the open tier's removal refusal");
    assert!(still_registered(&ctx, "open").await, "nothing was removed");
}

/// **Instance-state changes over MCP are an admin's, once agents authenticate.**
///
/// `add_domain` and `configure`'s mutating half write instance state, which the
/// JSON API has always gated admin-only; before this they were open to any
/// account whose agent held a token. A viewer and an editor are both refused;
/// an admin is not, which is what keeps the refusals from passing vacuously.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn instance_state_changes_over_mcp_are_admin_only() {
    let ctx = mcp_ctx(true).await;

    for account in ["looker", "owner"] {
        let token = ctx.token_for(account).await;
        let session = McpTestSession::open(&ctx.addr, Some(&token)).await;
        let added = session
            .call_tool(
                "add_domain",
                serde_json::json!({ "domain": "spare", "virtual": true }),
            )
            .await;
        assert!(
            added.contains("admin"),
            "{account} may not create a domain over MCP:\n{added}"
        );
        // The same shape the removal refusal owes, asserted here so the two
        // gates on one surface cannot disagree about whether their teaching
        // text reaches the model at all.
        refusal_is_readable(&added, "an add_domain refusal");
        let configured = session
            .call_tool(
                "configure",
                serde_json::json!({ "set": { "github.enabled": "true" } }),
            )
            .await;
        assert!(
            configured.contains("admin"),
            "{account} may not change settings over MCP:\n{configured}"
        );
        refusal_is_readable(&configured, "a configure refusal");
        let shown = session.call_tool("configure", serde_json::json!({})).await;
        assert!(
            !shown.contains("\"error\""),
            "but reading the settings is not a change:\n{shown}"
        );
    }
    assert!(
        !still_registered(&ctx, "spare").await,
        "nothing was created"
    );

    let boss = ctx.token_for("boss").await;
    let session = McpTestSession::open(&ctx.addr, Some(&boss)).await;
    let added = session
        .call_tool(
            "add_domain",
            serde_json::json!({ "domain": "spare", "virtual": true }),
        )
        .await;
    assert!(
        added.contains("spare") && !added.contains("admin"),
        "an admin creates one:\n{added}"
    );
}

/// **The open tier keeps exactly the instance-state powers it had.**
///
/// The carve-out that keeps a default install unchanged: with `auth.mcp` off
/// there are no accounts to hold a role, and refusing here would take away what
/// every single-user install already does on every session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_open_tier_still_creates_domains_and_configures() {
    let ctx = mcp_ctx(false).await;
    let session = McpTestSession::open(&ctx.addr, None).await;
    let added = session
        .call_tool(
            "add_domain",
            serde_json::json!({ "domain": "spare", "virtual": true }),
        )
        .await;
    assert!(
        added.contains("spare") && !added.contains("admin"),
        "the open tier still creates a domain:\n{added}"
    );
    let configured = session
        .call_tool(
            "configure",
            serde_json::json!({ "set": { "search.salience_weight": "0.2" } }),
        )
        .await;
    // Asserted as the setting having actually moved rather than as the absence
    // of a word: the settings snapshot this answers with documents every key,
    // and several of those doc strings say "admin" for reasons of their own.
    assert!(
        configured.contains("\\\"value\\\":\\\"0.2\\\""),
        "and still changes settings:\n{configured}"
    );
}

/// **A team domain is unregistered locally and nothing of the team's is
/// touched.**
///
/// The kind whose description makes the loudest promise: the local folder stays
/// (so `files_kept` is true and the markdown is still there), the origin state
/// this instance keeps is left alone, and the GitHub repository is never
/// reached at all - nothing here resolves a provider, which is also what keeps
/// this test offline. The question says "team" rather than "file", because
/// re-adding the folder would register a plain local domain and drop the team
/// connection, and telling somebody that inside a destructive confirmation is
/// telling them the wrong recovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_team_domain_is_unregistered_and_its_repository_is_untouched() {
    let ctx = mcp_team_ctx().await;
    let note = ctx.path("lab", "lab-note.md");
    let before = std::fs::read_to_string(&note).unwrap();
    let origin_before = ctx
        .engine
        .domain_has_origin("lab")
        .expect("the fixture's lab carries an origin");
    assert!(origin_before, "the fixture must make this a team domain");

    let preview = ctx
        .engine
        .domain_remove_preview("lab", &crystalline_service::Scope::Unrestricted, false)
        .await
        .unwrap();
    assert_eq!(
        preview["kind"],
        serde_json::json!("team"),
        "a domain carrying an origin is its own kind: {preview}"
    );

    let owner = ctx.token_for("owner").await;
    let session = McpTestSession::open(&ctx.addr, Some(&owner)).await;
    let removed = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "lab" }))
        .await;
    assert!(
        removed.contains("unregistered"),
        "the owner's removal lands:\n{removed}"
    );
    assert!(
        payload_of(&removed)["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("\"files_kept\":true"),
        "a team domain's files are kept:\n{removed}"
    );
    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        before,
        "the local folder is left byte-identical: a removal deletes no file, and \
         a team domain's is the one somebody would re-connect from"
    );
    assert!(
        ctx.path("lab", "MANIFEST.md").exists(),
        "manifest included, so the folder is still a domain to re-connect"
    );
}

/// **A virtual domain holding engrams is not removed without `purge`, whichever
/// surface asks**, and the refusal names the flag.
///
/// The rule lives in the engine rather than in one handler, because this route
/// is reachable by a private domain's owner now, not only by an admin: a
/// virtual domain's engrams are the rows, so the confirmation cannot be left to
/// whichever client happens to be asking.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_virtual_domain_needs_purge_over_mcp_and_the_refusal_names_it() {
    let ctx = mcp_ctx(true).await;
    let boss = ctx.token_for("boss").await;
    let session = McpTestSession::open(&ctx.addr, Some(&boss)).await;
    session
        .call_tool(
            "add_domain",
            serde_json::json!({ "domain": "mind", "virtual": true }),
        )
        .await;
    session
        .call_tool(
            "write_engram",
            serde_json::json!({ "domain": "mind", "title": "Only Copy", "content": "Nowhere else" }),
        )
        .await;

    let refused = session
        .call_tool("remove_domain", serde_json::json!({ "domain": "mind" }))
        .await;
    assert!(
        refused.contains("purge"),
        "the refusal names the flag:\n{refused}"
    );
    refusal_is_readable(&refused, "a virtual domain's purge refusal");
    assert!(
        still_registered(&ctx, "mind").await,
        "and nothing was removed"
    );

    let removed = session
        .call_tool(
            "remove_domain",
            serde_json::json!({ "domain": "mind", "purge": true }),
        )
        .await;
    assert!(
        removed.contains("unregistered"),
        "with purge it lands:\n{removed}"
    );
    assert!(!still_registered(&ctx, "mind").await);
}
