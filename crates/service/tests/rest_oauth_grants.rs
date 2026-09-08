//! The self-service OAuth grants surface at `/api/v1/me/oauth-grants`: an
//! account lists and revokes the hosted clients it has connected through the
//! authorization code flow, and never anybody else's.
//!
//! Driven through the production router construction
//! (`crystalline_service::daemon::http_router`) over a live loopback
//! listener, the same way `rest_mcp_tokens.rs` drives its sibling surface, so
//! the mount point and the guard are exercised rather than a hand-built
//! sub-router.
//!
//! Every fixture here serves with `auth.anonymous` on, which is what makes
//! the anonymous assertion mean something: with it off, a request carrying no
//! identity is refused by the guard before any handler runs, and the test
//! would pass whether or not this surface refuses the anonymous viewer
//! itself.
//!
//! A grant is minted here through `AuthStore::register_oauth_client` and
//! `AuthStore::issue_oauth_grant` directly rather than through the full
//! `/oauth/authorize` -> consent -> `/oauth/token` dance: those endpoints are
//! Tasks 4-6's, this surface only reads and deletes rows the store already
//! knows how to make, and going straight to the store is exactly how
//! `tests/mcp_auth.rs`'s own OAuth gate tests build their fixtures.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use serde_json::json;
use tokio::sync::Mutex;

/// A client for one identity: the served address plus the session cookie and
/// CSRF token it sends, or neither for the anonymous viewer.
#[derive(Clone)]
struct SessionClient {
    addr: std::net::SocketAddr,
    /// The session cookie and its CSRF token. `None` is the anonymous viewer.
    session: Option<(String, String)>,
}

impl SessionClient {
    /// A client with proxy discovery disabled: the target is loopback, where a
    /// system proxy must never be consulted anyway.
    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = Self::client().request(method, format!("http://{}{path}", self.addr));
        if let Some((cookie, csrf)) = &self.session {
            req = req
                .header("cookie", format!("fluid_session={cookie}"))
                .header("x-csrf-token", csrf);
        }
        req
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap()
    }

    async fn delete(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::DELETE, path)
            .send()
            .await
            .unwrap()
    }
}

/// A served router with one account already logged in. The temp directory owns
/// the auth database, so it must outlive every request.
struct RestCtx {
    client: SessionClient,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

impl std::ops::Deref for RestCtx {
    type Target = SessionClient;

    fn deref(&self) -> &SessionClient {
        &self.client
    }
}

impl RestCtx {
    /// Serve the production router over an engine with `auth.anonymous` on,
    /// add `name` at `role` and log in as it.
    async fn with_user(name: &str, role: Role) -> RestCtx {
        let ctx = RestCtx::anonymous_instance().await;
        ctx.auth
            .add_user(name, name, None, role, "s3cret")
            .await
            .unwrap();
        let session = login(ctx.client.addr, name, "s3cret").await;
        RestCtx {
            client: SessionClient {
                addr: ctx.client.addr,
                session: Some(session),
            },
            ..ctx
        }
    }

    /// The same instance with no account and no session: the anonymous viewer
    /// the guard lets through and this surface refuses on its own.
    async fn anonymous_instance() -> RestCtx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig {
            auth: Some(AuthConfig {
                trusted_header: None,
                proxy_headers: None,
                anonymous: Some(true),
                mcp: None,
                oauth: None,
                max_users: None,
                oidc: None,
            }),
            ..GlobalConfig::default()
        };
        // One domain so the engine has something registered; nothing here
        // reads it, and a virtual one touches no disk.
        cfg.domains
            .insert("void".to_string(), DomainEntry::virtual_domain());
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
        let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
        let addr = serve(engine, auth.clone());
        RestCtx {
            client: SessionClient {
                addr,
                session: None,
            },
            auth,
            _tmp: tmp,
        }
    }

    /// A second identity on the same instance: another account, logged in.
    async fn other_user(&self, name: &str, role: Role) -> SessionClient {
        self.auth
            .add_user(name, name, None, role, "s3cret")
            .await
            .unwrap();
        SessionClient {
            addr: self.client.addr,
            session: Some(login(self.client.addr, name, "s3cret").await),
        }
    }
}

/// Bind `http_router` on an ephemeral loopback port and serve it for the
/// duration of the test.
fn serve(engine: Arc<Engine>, auth: Arc<AuthStore>) -> std::net::SocketAddr {
    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth, None).unwrap();
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
    addr
}

/// Log in as `name`, returning the session cookie value and the CSRF token.
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let resp = SessionClient::client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&json!({"name": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login must succeed");
    let cookie = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.split(';').next()?.strip_prefix("fluid_session="))
        .expect("login sets the session cookie")
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    let csrf = body["csrf"].as_str().expect("login returns a csrf token");
    (cookie, csrf.to_string())
}

/// The body of a response, as JSON.
async fn body(resp: reqwest::Response) -> serde_json::Value {
    resp.json().await.unwrap()
}

/// Register a throwaway client and issue one grant of `user`'s at `resource`,
/// answering the client id alongside the grant so a test can pin the row's
/// `client_id` back to the registration it came from. The redirect uri is the
/// address a hosted client's callback uses, so the fixture reads like the
/// connection this surface exists to show.
async fn grant_for(
    auth: &AuthStore,
    user: &str,
    resource: &str,
) -> (String, crystalline_service::rest::IssuedOauthGrant) {
    let client = auth
        .register_oauth_client(
            "a hosted client",
            None,
            &["https://claude.ai/api/mcp/auth_callback".to_string()],
        )
        .await
        .unwrap();
    let grant = auth
        .issue_oauth_grant(user, &client.client_id, resource)
        .await
        .unwrap();
    (client.client_id, grant)
}

#[tokio::test]
async fn an_account_lists_and_revokes_its_own_grants_and_no_others() {
    let ctx = RestCtx::with_user("ada", Role::Editor).await;
    let bob = ctx.other_user("bob", Role::Admin).await;

    let (ada_client, ada_grant) = grant_for(&ctx.auth, "ada", "http://example.test").await;
    let (_, bob_grant) = grant_for(&ctx.auth, "bob", "http://example.test").await;

    // ada's listing carries her own row and its shape - client name, the
    // redirect host to recognize the client by, and no token material at all.
    let resp = ctx.get("/api/v1/me/oauth-grants").await;
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(
        !text.contains(&ada_grant.access_token) && !text.contains(&ada_grant.refresh_token),
        "no token material of any shape is listed: {text}"
    );
    assert!(
        !text.contains("coa_") && !text.contains("cor_") && !text.contains("hash"),
        "and no prefix or column name that would hint at one either: {text}"
    );
    let listed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 1, "only ada's own grant, never bob's: {rows:?}");
    assert_eq!(rows[0]["id"].as_i64().unwrap(), ada_grant.id);
    assert_eq!(rows[0]["client_id"], ada_client);
    let mut fields: Vec<&str> = rows[0]
        .as_object()
        .expect("a row is an object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "client_id",
            "client_name",
            "created_at",
            "id",
            "last_used",
            "redirect_host",
            "refresh_expires_at",
        ]
    );
    assert_eq!(rows[0]["client_name"], "a hosted client");
    assert_eq!(rows[0]["redirect_host"], "claude.ai");
    assert!(rows[0]["last_used"].is_null(), "never presented yet");

    // bob's listing is his own, admin or not.
    let bobs = body(bob.get("/api/v1/me/oauth-grants").await).await;
    let bobs = bobs.as_array().unwrap();
    assert_eq!(bobs.len(), 1);
    assert_eq!(bobs[0]["id"].as_i64().unwrap(), bob_grant.id);

    // Not 403: naming another account's grant id would be a way to probe for
    // it, so a grant that is not the caller's own is simply not there.
    assert_eq!(
        bob.delete(&format!("/api/v1/me/oauth-grants/{}", ada_grant.id))
            .await
            .status(),
        404
    );
    let mine = body(ctx.get("/api/v1/me/oauth-grants").await).await;
    assert_eq!(
        mine.as_array().unwrap().len(),
        1,
        "ada's grant is untouched by bob's attempt"
    );

    // ada revokes her own.
    assert_eq!(
        ctx.delete(&format!("/api/v1/me/oauth-grants/{}", ada_grant.id))
            .await
            .status(),
        204
    );
    let emptied = body(ctx.get("/api/v1/me/oauth-grants").await).await;
    assert!(
        emptied.as_array().unwrap().is_empty(),
        "the revoked grant is gone: {emptied}"
    );
    // Revoking it again finds nothing: the same id is 404, not idempotent 204.
    assert_eq!(
        ctx.delete(&format!("/api/v1/me/oauth-grants/{}", ada_grant.id))
            .await
            .status(),
        404
    );
    // An id that never named anything at all.
    assert_eq!(
        ctx.delete("/api/v1/me/oauth-grants/424242").await.status(),
        404
    );

    // bob's grant is untouched throughout.
    let bobs_after = body(bob.get("/api/v1/me/oauth-grants").await).await;
    assert_eq!(bobs_after.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn the_anonymous_viewer_holds_no_grants() {
    let ctx = RestCtx::anonymous_instance().await;

    let refused = ctx.get("/api/v1/me/oauth-grants").await;
    assert_eq!(refused.status(), 401);
    let detail = body(refused).await["detail"].as_str().unwrap().to_string();
    assert!(
        detail.contains("anonymous"),
        "the refusal is this surface's own, not the guard's: {detail}"
    );

    // Revoking is refused the same way, ahead of any id lookup: the anonymous
    // viewer has no account for any id to belong to.
    assert_eq!(ctx.delete("/api/v1/me/oauth-grants/1").await.status(), 401);
}

/// Serve `auth.mcp` and `auth.oauth` both on, so a grant's access token opens
/// an MCP session through the gate the same way a hosted client's would - the
/// one test in this file that needs the gate rather than the profile surface
/// alone.
async fn serve_with_mcp_oauth() -> (std::net::SocketAddr, tempfile::TempDir, Arc<AuthStore>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("void".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    cfg.auth = Some(AuthConfig {
        mcp: Some(true),
        oauth: Some(true),
        ..AuthConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    ));
    engine.sync(None).await.unwrap();
    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    let addr = serve(engine, auth.clone());
    (addr, tmp, auth)
}

/// A legacy-era `initialize` handshake, optionally presenting `bearer` as the
/// `Authorization` header verbatim.
async fn post_initialize(addr: &std::net::SocketAddr, bearer: Option<&str>) -> reqwest::Response {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "oauth-grants-test", "version": "0.0.0"},
        },
    })
    .to_string();
    let mut request = reqwest::Client::new()
        .post(format!("http://{addr}/"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body);
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    request.send().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_grant_is_refused_at_the_mcp_gate_at_once() {
    let (addr, _guard, auth) = serve_with_mcp_oauth().await;
    auth.add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let origin = format!("http://{addr}");
    let (_, grant) = grant_for(&auth, "ada", &origin).await;

    let opened = post_initialize(&addr, Some(&grant.access_token)).await;
    assert_eq!(
        opened.status(),
        200,
        "a live grant's access token opens a session at the gate"
    );
    drop(opened);

    // Revoke through the real route, as ada, under a real session and CSRF
    // token - proving the wiring end to end rather than calling the store
    // directly the way `tests/mcp_auth.rs`'s own OAuth gate tests do.
    let session = login(addr, "ada", "pw12345678").await;
    let client = SessionClient {
        addr,
        session: Some(session),
    };
    let revoked = client
        .delete(&format!("/api/v1/me/oauth-grants/{}", grant.id))
        .await;
    assert_eq!(revoked.status(), 204);

    // The very next request carrying the same access token is refused: there
    // is no cache between the store and the gate to wait out, only a fresh
    // lookup that now finds nothing.
    let refused = post_initialize(&addr, Some(&grant.access_token)).await;
    assert_eq!(
        refused.status(),
        401,
        "revoked at once, on the next request"
    );
}

/// `GET /auth/me`'s `oauth` field is the signal Fluid's connected-clients
/// card gates its whole render on (`Profile.tsx`'s `OauthGrantsCard`), so it
/// has to follow the setting this daemon actually came up with. The off side
/// of that (a default instance never turning it on) is pinned in
/// `rest_api.rs`'s `me_reports_capabilities_without_an_identity`; this is the
/// on side, over the one fixture in this file where `auth.oauth` is really
/// serving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_probes_oauth_flag_follows_the_setting() {
    let (addr, _guard, _auth) = serve_with_mcp_oauth().await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/auth/me"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["oauth"], true, "{body}");
}
