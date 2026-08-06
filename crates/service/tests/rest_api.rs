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
    // A handful of engrams, so the listing and tree endpoints have real
    // structure to report: one at the root, one a folder down, one two down.
    for engram in FIXTURE_ENGRAMS {
        let path = dir.join(engram.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, engram.markdown()).unwrap();
    }
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

/// One fixture engram: where it lives, what it is called, and the frontmatter
/// that lets a filtered listing tell it apart from its siblings.
struct FixtureEngram {
    /// The domain-relative path the file is written at.
    path: &'static str,
    /// The engram title.
    title: &'static str,
    /// The permalink it carries, folder-shaped for the nested ones the way a
    /// real domain writes them.
    permalink: &'static str,
    /// The engram `type`.
    engram_type: &'static str,
    /// The tag it carries beside the shared `eng` one.
    tag: &'static str,
}

impl FixtureEngram {
    /// This engram's markdown, in the shape a sync indexes: the frontmatter the
    /// format layer requires plus a body, so the fixture domain holds real rows
    /// rather than empty files.
    fn markdown(&self) -> String {
        let FixtureEngram {
            title,
            permalink,
            engram_type,
            tag,
            ..
        } = *self;
        let slug = title.to_ascii_lowercase();
        format!(
            "---\ntype: {engram_type}\ntitle: {title}\npermalink: {permalink}\ntags:\n  - eng\n  - {tag}\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\nA rule about {slug}.\n"
        )
    }
}

/// The seeded engrams: one at the root, one a folder down, one two down. The
/// nested two carry folder-shaped permalinks, so the detail route is exercised
/// on a permalink with slashes in it, and the types and tags differ so a
/// filtered listing has something to select on.
const FIXTURE_ENGRAMS: [FixtureEngram; 3] = [
    FixtureEngram {
        path: "alpha.md",
        title: "Alpha",
        permalink: "alpha",
        engram_type: "engram",
        tag: "root",
    },
    FixtureEngram {
        path: "notes/beta.md",
        title: "Beta",
        permalink: "notes/beta",
        engram_type: "guide",
        tag: "nested",
    },
    FixtureEngram {
        path: "notes/deep/gamma.md",
        title: "Gamma",
        permalink: "notes/deep/gamma",
        engram_type: "engram",
        tag: "nested",
    },
];

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

impl Fixture {
    /// The bytes of one seeded engram as they sit on disk, the source of truth
    /// a response is checked against.
    fn engram_bytes(&self, path: &str) -> Vec<u8> {
        std::fs::read(self._tmp.path().join("eng").join(path)).unwrap()
    }
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

/// The fixture with `auth.anonymous` on: the shortest way to reach a data
/// route with an identity the guard accepts.
async fn serve_anonymous() -> Fixture {
    serve_with_auth(AuthOptions {
        anonymous: true,
        ..AuthOptions::default()
    })
    .await
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

/// The `path` of every engram a tree response lists, in the order it listed
/// them.
fn engram_paths(tree: &serde_json::Value) -> Vec<String> {
    tree["engrams"]
        .as_array()
        .expect("a tree response carries an engrams array")
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect()
}

/// The `permalink` of every hit a listing returned, in the order it returned
/// them.
fn hit_permalinks(page: &serde_json::Value) -> Vec<String> {
    page["hits"]
        .as_array()
        .expect("a listing carries a hits array")
        .iter()
        .map(|h| h["permalink"].as_str().unwrap().to_string())
        .collect()
}

/// The lowercase hex SHA-256 digest of `bytes`, computed here rather than
/// asked of the server: the point of the ETag assertions is that the validator
/// the API sends is the digest of the markdown, independently arrived at.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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
/// authenticate rather than being told which paths exist. Every data route is
/// checked, not just one: a route registered below the guard layer instead of
/// above it would serve its payload to anybody, and only asking it would say so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_routes_401_without_identity_when_not_anonymous() {
    let fixture = serve_with_auth(AuthOptions::default()).await;
    for path in [
        "/api/v1/domains",
        "/api/v1/domains/eng/tree",
        "/api/v1/domains/eng/manifest",
        "/api/v1/domains/eng/engrams",
        "/api/v1/domains/eng/engrams/alpha",
        "/api/v1/domains/eng/engrams/notes/deep/gamma",
    ] {
        let resp = get(fixture.addr, path).await;
        assert_eq!(resp.status(), 401, "{path} must be guarded");
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], 401);
    }
}

/// A logged-in caller passes the guard and is served the data itself: the
/// session cookie alone is enough to read the domain listing.
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
    assert_eq!(resp.status(), 200, "past the guard, into the handler");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["domains"][0]["name"], "eng");
}

/// With `auth.anonymous` on, an unidentified caller is served as a viewer, so
/// the guard lets the request through to the data itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_access_passes_the_guard() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/domains").await;
    assert_eq!(resp.status(), 200, "past the guard, into the handler");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["domains"][0]["name"], "eng");
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

    // A data route is served with the header alone: no cookie, no login.
    let resp = client()
        .get(format!("http://{}/api/v1/domains", fixture.addr))
        .header("remote-user", "bob")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "past the guard, into the handler");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["domains"][0]["name"], "eng");
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

/// Behind a TLS-terminating proxy the cookie must be `Secure` even though the
/// `Host` says loopback: nginx's default `proxy_pass` rewrites `Host` to the
/// upstream's own address, so the forwarded protocol is the only signal left
/// that the browser is on the far side of TLS.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forwarded_https_login_gets_a_secure_cookie() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    for (header, value) in [
        ("x-forwarded-proto", "https"),
        ("forwarded", "for=192.0.2.1;proto=https"),
    ] {
        let resp = client()
            .post(format!("http://{}/api/v1/auth/login", fixture.addr))
            .header(header, value)
            .json(&serde_json::json!({"name": "ada", "password": "s3cret"}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let raw = set_cookie(&resp).expect("login sets the session cookie");
        assert!(
            raw.contains("Secure"),
            "{header} must force the Secure flag: {raw}"
        );
    }
}

/// Session fixation: a token the caller arrives holding is retired by a
/// successful login rather than left live beside the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logging_in_retires_the_session_that_was_presented() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (planted, _) = login(fixture.addr, "ada", "s3cret").await;

    let resp = client()
        .post(format!("http://{}/api/v1/auth/login", fixture.addr))
        .header("cookie", format!("fluid_session={planted}"))
        .json(&serde_json::json!({"name": "ada", "password": "s3cret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let fresh = session_cookie(&resp).unwrap();
    assert_ne!(fresh, planted, "a login always mints a new token");

    let me: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("cookie", format!("fluid_session={planted}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(me["user"].is_null(), "the presented session is dead: {me}");

    let still: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("cookie", format!("fluid_session={fresh}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(still["user"]["name"], "ada");
}

/// The domain listing is the engine's own value, verbatim: every registered
/// domain with its counts, plus the routing block a client needs to know what
/// each domain is for. `include_routing` is always on, so `behavior` and
/// `when_to_use` are part of the contract rather than an option a caller has to
/// ask for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domains_lists_every_domain_with_its_routing_bullets() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/domains").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["behavior"].as_array().is_some_and(|b| !b.is_empty()),
        "the listing carries the behavior rules: {body}"
    );
    let domains = body["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "one registered domain: {body}");
    assert_eq!(domains[0]["name"], "eng");
    assert_eq!(domains[0]["kind"], "file");
    assert_eq!(
        domains[0]["engrams"], 4,
        "the MANIFEST and the three seeded engrams: {body}"
    );
    assert!(
        domains[0]["when_to_use"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b.as_str().unwrap().contains("Route here for eng")),
        "the routing bullets come from the MANIFEST: {body}"
    );
}

/// The tree endpoint is `browse_domain` behind a query string: the defaults
/// list the root one level deep, `path` descends, `depth` widens and `glob`
/// filters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domain_tree_walks_folders_and_filters_by_glob() {
    let fixture = serve_anonymous().await;

    let root: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/tree")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(root["domain"], "eng");
    assert_eq!(root["path"], "/");
    assert_eq!(root["folders"].as_array().unwrap(), &["notes"]);
    let paths = engram_paths(&root);
    assert!(paths.contains(&"alpha.md".to_string()), "{paths:?}");
    assert!(paths.contains(&"MANIFEST.md".to_string()), "{paths:?}");
    assert!(
        !paths.contains(&"notes/beta.md".to_string()),
        "one level deep by default: {paths:?}"
    );

    let notes: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/tree?path=notes&depth=2")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(notes["path"], "notes");
    assert_eq!(notes["folders"].as_array().unwrap(), &["deep"]);
    let paths = engram_paths(&notes);
    assert!(paths.contains(&"notes/beta.md".to_string()), "{paths:?}");
    assert!(
        paths.contains(&"notes/deep/gamma.md".to_string()),
        "depth 2 reaches the nested folder: {paths:?}"
    );

    let globbed: serde_json::Value = get(
        fixture.addr,
        "/api/v1/domains/eng/tree?depth=3&glob=notes/**",
    )
    .await
    .json()
    .await
    .unwrap();
    let paths = engram_paths(&globbed);
    assert_eq!(
        paths,
        vec![
            "notes/beta.md".to_string(),
            "notes/deep/gamma.md".to_string()
        ],
        "the glob filters the whole walk"
    );
}

/// The manifest endpoint hands back the domain's MANIFEST markdown as written,
/// so a client can render or edit the source rather than a reduction of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn domain_manifest_returns_the_markdown_source() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/domains/eng/manifest").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["domain"], "eng");
    let markdown = body["markdown"].as_str().expect("markdown is a string");
    assert!(
        markdown.starts_with("---\n"),
        "the frontmatter is part of the source: {markdown}"
    );
    assert!(markdown.contains("## When to Use"), "{markdown}");
    assert!(
        markdown.contains("Route here for eng questions"),
        "{markdown}"
    );
}

/// The listing is `search_engrams` with no query behind a query string: the
/// filters select, and the page envelope the engine already writes comes
/// through unchanged so a client can page without a second shape to learn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engram_list_filters_and_carries_the_page_envelope() {
    let fixture = serve_anonymous().await;

    let all: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/engrams")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        all["total"], 4,
        "the MANIFEST and the three seeded engrams: {all}"
    );
    assert_eq!(all["page"], 1, "the envelope names the page: {all}");
    assert_eq!(all["limit"], 10, "and the page size: {all}");
    assert_eq!(all["count"], 4, "and what this page holds: {all}");

    // A comma-separated tag list is split, and every tag has to match: `eng` is
    // on all three, `nested` on two, so the intersection is the nested pair.
    let tagged: serde_json::Value =
        get(fixture.addr, "/api/v1/domains/eng/engrams?tags=eng,nested")
            .await
            .json()
            .await
            .unwrap();
    assert_eq!(tagged["total"], 2, "only the tagged pair: {tagged}");
    assert_eq!(
        hit_permalinks(&tagged),
        vec!["notes/beta".to_string(), "notes/deep/gamma".to_string()],
        "{tagged}"
    );

    let typed: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/engrams?type=guide")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(hit_permalinks(&typed), vec!["notes/beta".to_string()]);

    let statused: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/engrams?status=draft")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(statused["total"], 0, "nothing is a draft here: {statused}");

    // Paging: two pages of two over the same filtered set, disjoint and
    // covering it.
    let first: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/engrams?limit=2&page=1")
        .await
        .json()
        .await
        .unwrap();
    let second: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/engrams?limit=2&page=2")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(second["page"], 2);
    assert_eq!(second["limit"], 2);
    assert_eq!(second["total"], 4, "the total spans the pages: {second}");
    let mut paged = hit_permalinks(&first);
    paged.extend(hit_permalinks(&second));
    assert_eq!(paged.len(), 4, "two pages of two: {first} {second}");
    let mut unique = paged.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 4, "the pages do not overlap: {paged:?}");
}

/// The detail route answers with the engram itself - its frontmatter, its
/// markdown and the graph the engine resolves around it - plus an `ETag` a
/// later conditional write can present. The validator is the SHA-256 of the
/// markdown, quoted as RFC 9110 requires of a strong one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engram_detail_carries_the_source_and_a_strong_etag() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/domains/eng/engrams/alpha").await;
    assert_eq!(resp.status(), 200);
    let etag = resp
        .headers()
        .get("etag")
        .expect("the detail response carries an ETag")
        .to_str()
        .unwrap()
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();

    assert_eq!(body["domain"], "eng");
    assert_eq!(body["permalink"], "alpha");
    assert_eq!(body["title"], "Alpha");
    assert_eq!(body["url"], "crystalline://eng/alpha");
    assert_eq!(
        body["frontmatter"]["title"], "Alpha",
        "the frontmatter is part of the answer: {body}"
    );
    assert_eq!(body["frontmatter"]["permalink"], "alpha");

    let on_disk = fixture.engram_bytes("alpha.md");
    assert_eq!(
        body["content"].as_str().unwrap().as_bytes(),
        on_disk.as_slice(),
        "the content is the markdown as written: {body}"
    );
    assert_eq!(
        etag,
        format!("\"{}\"", sha256_hex(&on_disk)),
        "the ETag is the quoted SHA-256 of that markdown"
    );
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && !etag.starts_with("W/"),
        "a strong validator, quoted: {etag}"
    );
}

/// A permalink is a path, not a segment: the route captures the rest of the URL
/// so an engram two folders down is reachable by the permalink it carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engram_detail_resolves_a_permalink_with_folders() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/domains/eng/engrams/notes/deep/gamma").await;
    assert_eq!(resp.status(), 200, "a nested permalink resolves");
    let etag = resp.headers()["etag"].to_str().unwrap().to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["permalink"], "notes/deep/gamma");
    assert_eq!(body["path"], "notes/deep/gamma.md");
    assert_eq!(
        etag,
        format!(
            "\"{}\"",
            sha256_hex(&fixture.engram_bytes("notes/deep/gamma.md"))
        )
    );
}

/// An engram nobody wrote is a 404 problem detail, the same shape every other
/// failure on this surface has, and the engine's own message says what was
/// looked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_permalink_is_a_404_problem_detail() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/domains/eng/engrams/notes/ghost").await;
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    assert!(
        resp.headers().get("etag").is_none(),
        "a failure carries no validator"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "not found");
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("notes/ghost"), "{detail}");
    assert!(detail.contains("eng"), "{detail}");
}

/// A domain nobody registered is a 404 problem detail that names the domains
/// that do exist, the same answer the engine's other verbs give.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_domain_is_a_404_problem_detail() {
    let fixture = serve_anonymous().await;
    for path in [
        "/api/v1/domains/ghost/tree",
        "/api/v1/domains/ghost/manifest",
    ] {
        let resp = get(fixture.addr, path).await;
        assert_eq!(resp.status(), 404, "{path} must be a 404");
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], 404);
        assert_eq!(body["title"], "not found");
        let detail = body["detail"].as_str().unwrap();
        assert!(detail.contains("ghost"), "{detail}");
        assert!(detail.contains("eng"), "the valid set is named: {detail}");
    }
}

/// axum's own extractor rejections are rendered in the same problem+json
/// contract as everything else, so a client never has to parse a plain-text
/// body it was not expecting: a query parameter of the wrong type is a 400
/// problem detail rather than `Failed to deserialize query string` in text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_query_parameter_is_a_problem_detail() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/domains/eng/tree?depth=deep").await;
    assert_eq!(resp.status(), 400);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 400);
    assert_eq!(body["title"], "invalid request");
    assert!(
        body["detail"].as_str().unwrap().contains("query"),
        "the detail says what was wrong: {body}"
    );
}

/// A method a route does not serve is a 405 problem detail rather than axum's
/// empty default, and it still carries the `Allow` header the HTTP spec asks
/// for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_method_is_a_405_problem_detail() {
    let fixture = serve_anonymous().await;
    let resp = client()
        .post(format!("http://{}/api/v1/domains", fixture.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    assert!(
        resp.headers()
            .get("allow")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("GET")),
        "a 405 names the methods that do work: {:?}",
        resp.headers()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 405);
    assert_eq!(body["title"], "method not allowed");
}

/// The same contract on the way in: a login body axum cannot deserialize is a
/// problem detail, whether it is unparseable, the wrong shape or sent without a
/// JSON content type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_request_body_is_a_problem_detail() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let url = format!("http://{}/api/v1/auth/login", fixture.addr);
    let cases = [
        // Unparseable JSON.
        (
            client()
                .post(&url)
                .header("content-type", "application/json")
                .body("{"),
            400,
        ),
        // Parseable, but not the shape the handler takes.
        (
            client().post(&url).json(&serde_json::json!({"name": 7})),
            422,
        ),
        // No JSON content type at all.
        (client().post(&url).body("name=ada"), 415),
    ];
    for (request, expected) in cases {
        let resp = request.send().await.unwrap();
        assert_eq!(resp.status(), expected);
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], expected);
        assert!(
            !body["detail"].as_str().unwrap().is_empty(),
            "the detail says what was wrong: {body}"
        );
    }
}
