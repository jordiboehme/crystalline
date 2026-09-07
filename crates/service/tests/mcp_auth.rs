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
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig,
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
