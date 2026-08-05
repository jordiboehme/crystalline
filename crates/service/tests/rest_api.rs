//! Drives the REST surface `serve --http` mounts at `/api/v1` over a live TCP
//! listener, through the production router construction
//! (`crystalline_service::daemon::http_router`) rather than a hand-built
//! sub-router, so a regression in the mount point or in the nesting order
//! against the MCP fallback service fails here.

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

/// The two startup-effective auth settings a test varies. Everything else is
/// the shared fixture below.
#[derive(Default)]
struct AuthOptions {
    /// `auth.anonymous`: serve a request that carries no identity.
    anonymous: bool,
    /// `auth.trusted_header`: the header a trusted proxy names the user in.
    trusted_header: Option<&'static str>,
}

/// Build the same kind of engine the other service integration tests use: a
/// real temp-directory domain (files are the source of truth) synced into an
/// in-memory Turso store, response format pinned to plain JSON so assertions
/// don't have to account for TOON framing, and `opts` in the auth block.
async fn build_engine(opts: AuthOptions) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        auth: Some(AuthConfig {
            trusted_header: opts.trusted_header.map(str::to_string),
            anonymous: Some(opts.anonymous),
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

/// Bind `http_router` on an ephemeral loopback port and serve it on a
/// background task for the duration of the test.
fn serve_test_router(engine: Arc<Engine>, auth: Arc<AuthStore>) -> std::net::SocketAddr {
    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// A served router plus the pieces a test needs to reach behind it. The temp
/// directory owns both the domain and the auth database, so it must outlive
/// every request.
struct Fixture {
    addr: std::net::SocketAddr,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

/// Serve the production router over a fixture engine. The returned guard owns
/// the domain's temp directory and must outlive the requests.
async fn serve_test_router_with_fixture() -> (std::net::SocketAddr, tempfile::TempDir) {
    let fixture = serve_with_auth(AuthOptions::default()).await;
    (fixture.addr, fixture._tmp)
}

/// The auth fixture: the production router over an engine carrying `opts`, with
/// an auth database in the same temp directory the test can seed accounts in.
async fn serve_with_auth(opts: AuthOptions) -> Fixture {
    let (tmp, engine) = build_engine(opts).await;
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    let addr = serve_test_router(engine, auth.clone());
    Fixture {
        addr,
        auth,
        _tmp: tmp,
    }
}

/// The same fixture with one viewer account, `ada` / `s3cret`, already added.
async fn serve_with_ada(opts: AuthOptions) -> Fixture {
    let fixture = serve_with_auth(opts).await;
    fixture
        .auth
        .add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();
    fixture
}

/// A client with proxy discovery disabled: the target is loopback, where a
/// system proxy must never be consulted anyway, and reqwest's platform proxy
/// lookup can block for a minute on a machine with a managed network
/// configuration.
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// GET a path off the test server.
async fn get(addr: std::net::SocketAddr, path: &str) -> reqwest::Response {
    client()
        .get(format!("http://{addr}{path}"))
        .send()
        .await
        .unwrap()
}

/// Log in as `name`, returning the session cookie value and the CSRF token.
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({"name": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login must succeed");
    let cookie = session_cookie(&resp).expect("login must set the session cookie");
    let body: serde_json::Value = resp.json().await.unwrap();
    let csrf = body["csrf"].as_str().expect("login returns a csrf token");
    (cookie, csrf.to_string())
}

/// The `fluid_session` value out of a response's `Set-Cookie`, or `None` when
/// the response carries no session cookie.
fn session_cookie(resp: &reqwest::Response) -> Option<String> {
    let raw = set_cookie(resp)?;
    let value = raw.split(';').next()?.trim();
    value.strip_prefix("fluid_session=").map(str::to_string)
}

/// The raw `Set-Cookie` header for `fluid_session`, attributes and all.
fn set_cookie(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .map(str::to_string)
}

/// The capability probe answers without an identity, which is what lets the UI
/// decide whether to show a login form: no user, and `anonymous` false because
/// this instance does not serve unidentified callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn me_reports_capabilities_without_an_identity() {
    let (addr, _guard) = serve_test_router_with_fixture().await;
    let resp = get(addr, "/api/v1/auth/me").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["user"].is_null());
    assert_eq!(body["anonymous"], false);
    assert_eq!(body["version"], crystalline_core::VERSION);
    assert!(body["read_only"].is_boolean());
}

/// With `auth.anonymous` on, the same probe reports that the caller is being
/// served anonymously, so the UI can browse instead of prompting for a login.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn me_reports_anonymous_access_when_it_is_enabled() {
    let fixture = serve_with_auth(AuthOptions {
        anonymous: true,
        ..AuthOptions::default()
    })
    .await;
    let body: serde_json::Value = get(fixture.addr, "/api/v1/auth/me")
        .await
        .json()
        .await
        .unwrap();
    assert!(body["user"].is_null());
    assert_eq!(body["anonymous"], true);
}

/// An unknown path under the mount answers in problem+json rather than falling
/// through to the MCP transport. Reached with an identity: the guard runs ahead
/// of routing, so an unauthenticated caller is answered 401 and never learns
/// which paths exist (see `data_routes_401_without_identity_when_not_anonymous`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_api_path_is_problem_json() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (token, _) = login(fixture.addr, "ada", "s3cret").await;
    let resp = client()
        .get(format!("http://{}/api/v1/nope", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "not found");
}

/// The REST mount must not shadow what the liveness probe and the MCP
/// transport already own: `/api/v1` nests ahead of the fallback service, and
/// everything outside it keeps its old behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_probe_still_answers_beside_the_rest_mount() {
    let (addr, _guard) = serve_test_router_with_fixture().await;
    let resp = get(addr, "/health").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

/// The login round trip: credentials in, a session cookie out, and the next
/// request identified as that account without sending anything but the cookie.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_sets_cookie_and_me_identifies() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let resp = client()
        .post(format!("http://{}/api/v1/auth/login", fixture.addr))
        .json(&serde_json::json!({"name": "ada", "password": "s3cret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let raw = set_cookie(&resp).expect("login sets the session cookie");
    assert!(raw.contains("HttpOnly"), "cookie must be HttpOnly: {raw}");
    assert!(raw.contains("SameSite=Lax"), "cookie must be Lax: {raw}");
    assert!(raw.contains("Path=/"), "cookie must cover the API: {raw}");
    assert!(
        !raw.contains("Secure"),
        "a loopback request gets a cookie usable over plain http: {raw}"
    );
    let token = session_cookie(&resp).unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["user"]["name"], "ada");
    assert_eq!(body["user"]["role"], "viewer");
    assert!(body["csrf"].as_str().is_some_and(|c| !c.is_empty()));
    assert!(
        !body["user"].as_object().unwrap().contains_key("pass_hash"),
        "no password material may reach the client"
    );

    let me: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["name"], "ada");
    assert_eq!(me["anonymous"], false);
}

/// A name that differs only in case is the same account, and the stored name is
/// the folded one the store keys on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_folds_the_name_case() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (token, _) = login(fixture.addr, "  AdA ", "s3cret").await;
    let me: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["name"], "ada");
}

/// Every way a login can fail answers the same 401 problem detail, so a caller
/// cannot tell an unknown name from a wrong password.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_login_is_an_indistinguishable_401() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let mut bodies = Vec::new();
    for (name, password) in [("ada", "wrong"), ("ghost", "s3cret"), ("", "s3cret")] {
        let resp = client()
            .post(format!("http://{}/api/v1/auth/login", fixture.addr))
            .json(&serde_json::json!({"name": name, "password": password}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401, "login as {name:?} must be refused");
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        assert!(
            set_cookie(&resp).is_none(),
            "a refused login sets no session cookie"
        );
        bodies.push(resp.text().await.unwrap());
    }
    assert!(
        bodies.windows(2).all(|w| w[0] == w[1]),
        "every refusal must read the same: {bodies:?}"
    );
}

/// The guard runs ahead of routing, so an unauthenticated caller is told to
/// authenticate rather than being told which paths exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_routes_401_without_identity_when_not_anonymous() {
    let fixture = serve_with_auth(AuthOptions::default()).await;
    let resp = get(fixture.addr, "/api/v1/domains").await;
    assert_eq!(resp.status(), 401);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 401);
}

/// A logged-in caller passes the guard: the request reaches routing, where this
/// path is simply unknown until the data routes land.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_routes_pass_the_guard_with_a_session() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (token, _) = login(fixture.addr, "ada", "s3cret").await;
    let resp = client()
        .get(format!("http://{}/api/v1/domains", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "past the guard, into routing");
}

/// With `auth.anonymous` on, an unidentified caller is served as a viewer, so
/// the guard lets the request through to routing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_access_passes_the_guard() {
    let fixture = serve_with_auth(AuthOptions {
        anonymous: true,
        ..AuthOptions::default()
    })
    .await;
    let resp = get(fixture.addr, "/api/v1/domains").await;
    assert_eq!(resp.status(), 404, "past the guard, into routing");
}

/// The trusted-header path: the proxy has already authenticated the caller, so
/// the header names the account and one is provisioned at viewer on first sight.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_header_maps_identity() {
    let fixture = serve_with_auth(AuthOptions {
        trusted_header: Some("remote-user"),
        ..AuthOptions::default()
    })
    .await;
    let me: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("remote-user", "Bob")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["name"], "bob", "the name is folded by the store");
    assert_eq!(me["user"]["display"], "Bob");
    assert_eq!(me["user"]["role"], "viewer");

    let users = fixture.auth.list_users().await.unwrap();
    assert_eq!(users.len(), 1, "the account was provisioned once");
    assert_eq!(users[0].name, "bob");

    // A data route is reachable with the header alone: no cookie, no login.
    let resp = client()
        .get(format!("http://{}/api/v1/domains", fixture.addr))
        .header("remote-user", "bob")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "past the guard, into routing");
}

/// The header is only believed when it is configured: an instance that has not
/// been told to trust a proxy ignores whatever a client sends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unconfigured_trusted_header_is_ignored() {
    let fixture = serve_with_auth(AuthOptions::default()).await;
    let resp = client()
        .get(format!("http://{}/api/v1/domains", fixture.addr))
        .header("remote-user", "bob")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    assert!(fixture.auth.list_users().await.unwrap().is_empty());
}

/// A disabled account is refused even when the proxy vouches for it: the store
/// hands back disabled accounts as ordinary users, so the refusal is here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_account_is_refused_on_the_trusted_header() {
    let fixture = serve_with_auth(AuthOptions {
        trusted_header: Some("remote-user"),
        ..AuthOptions::default()
    })
    .await;
    fixture
        .auth
        .add_user("bob", "Bob", None, Role::Editor, "pw")
        .await
        .unwrap();
    fixture.auth.set_disabled("bob", true).await.unwrap();
    let resp = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("remote-user", "bob")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
}

/// Logout is a mutating request, so it carries the CSRF token the session was
/// issued with; without it the session survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logout_requires_csrf() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (token, csrf) = login(fixture.addr, "ada", "s3cret").await;

    let refused = client()
        .post(format!("http://{}/api/v1/auth/logout", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.headers()["content-type"],
        "application/problem+json"
    );

    let wrong = client()
        .post(format!("http://{}/api/v1/auth/logout", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .header("x-csrf-token", "not the token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 403);

    // The refusals left the session alone.
    let still: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(still["user"]["name"], "ada");

    let ok = client()
        .post(format!("http://{}/api/v1/auth/logout", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .header("x-csrf-token", &csrf)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    assert!(
        set_cookie(&ok).is_some_and(|c| c.contains("Max-Age=0") || c.contains("fluid_session=;")),
        "logout clears the cookie"
    );

    let me: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(me["user"].is_null(), "the session is gone: {me}");

    // The revoked cookie no longer opens a data route either.
    let resp = client()
        .get(format!("http://{}/api/v1/domains", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// Login itself is exempt from the CSRF check (there is no session yet to take
/// a token from), which is the one hole the middleware must leave open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_needs_no_csrf_token() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (_token, _csrf) = login(fixture.addr, "ada", "s3cret").await;
}

/// Concurrent failed logins all get an answer: the semaphore that caps argon2
/// memory queues them rather than dropping or deadlocking any.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_failed_logins_all_answer() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let mut tasks = Vec::new();
    for i in 0..12 {
        let addr = fixture.addr;
        tasks.push(tokio::spawn(async move {
            client()
                .post(format!("http://{addr}/api/v1/auth/login"))
                .json(&serde_json::json!({"name": format!("ghost{i}"), "password": "nope"}))
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), 401);
    }
}
