//! The self-service MCP token surface at `/api/v1/me/mcp-tokens`: an account
//! issues, lists, rotates and revokes the tokens its own agents authenticate
//! with, and never anybody else's.
//!
//! Driven through the production router construction
//! (`crystalline_service::daemon::http_router`) over a live loopback listener,
//! the same way `rest_api.rs` drives the rest of the surface, so the mount
//! point and the guard are exercised rather than a hand-built sub-router.
//!
//! Every fixture here serves with `auth.anonymous` on, which is what makes the
//! anonymous assertion mean something: with it off, a request carrying no
//! identity is refused by the guard before any handler runs, and the test
//! would pass whether or not this surface refuses the anonymous viewer itself.

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

    async fn post_json(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.request(reqwest::Method::POST, path)
            .json(&body)
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

    /// The anonymous viewer on the same instance.
    fn as_anonymous(&self) -> SessionClient {
        SessionClient {
            addr: self.client.addr,
            session: None,
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

/// A response's `Cache-Control`, which the two token-carrying replies set.
fn no_store(resp: &reqwest::Response) -> Option<&str> {
    resp.headers().get("cache-control")?.to_str().ok()
}

/// The body of a response, as JSON.
async fn body(resp: reqwest::Response) -> serde_json::Value {
    resp.json().await.unwrap()
}

#[tokio::test]
async fn editor_issues_lists_rotates_and_revokes_own_tokens() {
    let ctx = RestCtx::with_user("ada", Role::Editor).await;

    let resp = ctx
        .post_json("/api/v1/me/mcp-tokens", json!({"label": "laptop"}))
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        no_store(&resp),
        Some("no-store"),
        "the one response carrying a live credential is never stored"
    );
    let issued = body(resp).await;
    let token = issued["token"]
        .as_str()
        .expect("the token is returned once");
    assert!(token.starts_with("cmt_"), "{token}");
    assert_eq!(issued["label"], "laptop");
    let id = issued["id"].as_i64().expect("the row id comes back");

    let listed = body(ctx.get("/api/v1/me/mcp-tokens").await).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"].as_i64().unwrap(), id);

    let resp = ctx
        .post_json(&format!("/api/v1/me/mcp-tokens/{id}/rotate"), json!({}))
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        no_store(&resp),
        Some("no-store"),
        "and neither is the rotation's"
    );
    let rotated = body(resp).await;
    let rotated_id = rotated["id"].as_i64().unwrap();
    assert_ne!(rotated_id, id, "rotation issues a new row, not the old one");
    assert_ne!(
        rotated["token"].as_str().unwrap(),
        token,
        "and a new secret with it"
    );
    assert_eq!(
        rotated["label"], "laptop",
        "the label rides along, so the row stays recognizable"
    );

    // The OLD id is gone: rotation already removed it.
    assert_eq!(
        ctx.delete(&format!("/api/v1/me/mcp-tokens/{id}"))
            .await
            .status(),
        404
    );
    assert_eq!(
        ctx.delete(&format!("/api/v1/me/mcp-tokens/{rotated_id}"))
            .await
            .status(),
        204
    );
    let listed = body(ctx.get("/api/v1/me/mcp-tokens").await).await;
    assert!(
        listed.as_array().unwrap().is_empty(),
        "the revoked token is gone: {listed}"
    );
}

#[tokio::test]
async fn a_viewer_account_issues_its_own_and_anonymous_cannot() {
    let ctx = RestCtx::with_user("eve", Role::Viewer).await;

    let resp = ctx
        .post_json("/api/v1/me/mcp-tokens", json!({"label": "x"}))
        .await;
    assert_eq!(resp.status(), 200);
    let issued = body(resp).await;
    assert!(
        issued["token"].as_str().unwrap().starts_with("cmt_"),
        "a viewer's agent acts as the viewer and is read-only by construction"
    );

    // The anonymous viewer is served by the guard (this instance has
    // `auth.anonymous` on) and refused here, by this surface, in its own
    // words: it has no account, so there is nothing to issue a token for.
    let refused = ctx
        .as_anonymous()
        .post_json("/api/v1/me/mcp-tokens", json!({"label": "x"}))
        .await;
    assert_eq!(refused.status(), 401);
    let detail = body(refused).await["detail"].as_str().unwrap().to_string();
    assert!(
        detail.contains("anonymous"),
        "the refusal is this surface's own, not the guard's: {detail}"
    );
    assert_eq!(
        ctx.as_anonymous()
            .get("/api/v1/me/mcp-tokens")
            .await
            .status(),
        401,
        "and it has no listing either"
    );
}

#[tokio::test]
async fn a_listing_carries_no_token_material() {
    let ctx = RestCtx::with_user("ada", Role::Editor).await;
    let issued = body(
        ctx.post_json("/api/v1/me/mcp-tokens", json!({"label": "ci"}))
            .await,
    )
    .await;
    let token = issued["token"].as_str().unwrap().to_string();

    let resp = ctx.get("/api/v1/me/mcp-tokens").await;
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(
        !text.contains(&token),
        "the token is shown once, at issuance: {text}"
    );
    assert!(
        !text.contains("cmt_") && !text.contains("token_hash"),
        "and no token material of any shape is listed: {text}"
    );
    let listed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut fields: Vec<&str> = listed[0]
        .as_object()
        .expect("a row is an object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, ["created_at", "id", "label", "last_used"]);
}

#[tokio::test]
async fn one_account_never_sees_or_revokes_another_accounts_tokens() {
    let ctx = RestCtx::with_user("ada", Role::Editor).await;
    let bob = ctx.other_user("bob", Role::Admin).await;

    let issued = body(
        ctx.post_json("/api/v1/me/mcp-tokens", json!({"label": "ada's"}))
            .await,
    )
    .await;
    let id = issued["id"].as_i64().unwrap();

    let bobs = body(bob.get("/api/v1/me/mcp-tokens").await).await;
    assert!(
        bobs.as_array().unwrap().is_empty(),
        "an account's listing is its own, admin or not: {bobs}"
    );
    // Not 403: naming another account's token id would be a way to probe for
    // it, so a token that is not the caller's own is simply not there.
    assert_eq!(
        bob.delete(&format!("/api/v1/me/mcp-tokens/{id}"))
            .await
            .status(),
        404
    );
    assert_eq!(
        bob.post_json(&format!("/api/v1/me/mcp-tokens/{id}/rotate"), json!({}))
            .await
            .status(),
        404
    );

    let mine = body(ctx.get("/api/v1/me/mcp-tokens").await).await;
    assert_eq!(
        mine.as_array().unwrap().len(),
        1,
        "ada's token is untouched"
    );
    assert_eq!(mine[0]["id"].as_i64().unwrap(), id);
}

#[tokio::test]
async fn an_empty_label_is_refused_and_an_unknown_id_is_not_found() {
    let ctx = RestCtx::with_user("ada", Role::Editor).await;
    assert_eq!(
        ctx.post_json("/api/v1/me/mcp-tokens", json!({"label": "  "}))
            .await
            .status(),
        422
    );
    assert_eq!(ctx.delete("/api/v1/me/mcp-tokens/4242").await.status(), 404);
    assert_eq!(
        ctx.post_json("/api/v1/me/mcp-tokens/4242/rotate", json!({}))
            .await
            .status(),
        404
    );
}
