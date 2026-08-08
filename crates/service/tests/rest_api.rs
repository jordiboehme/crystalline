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

/// The startup-effective auth settings a test varies. Everything else is
/// the shared fixture below.
#[derive(Default)]
struct AuthOptions {
    /// `auth.anonymous`: serve a request that carries no identity.
    anonymous: bool,
    /// `auth.trusted_header`: the header a trusted proxy names the user in.
    trusted_header: Option<&'static str>,
    /// `auth.max_users`: how many accounts trusted-header provisioning may
    /// mint in total. `None` leaves the default cap (100) in place.
    max_users: Option<u32>,
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
            max_users: opts.max_users,
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
    // A registered domain that holds nothing, so "this domain does not exist"
    // and "this domain is empty" are both reachable and visibly different. It
    // is virtual because a file domain always has its MANIFEST indexed and so
    // is never truly empty. Registered after `eng`, and the config keeps
    // insertion order, so the listing's first domain is still `eng`.
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
    /// The title this engram declares a `relates_to` relation onto, if any, so
    /// the fixture domain holds a real edge for the context endpoint to walk.
    relates_to: Option<&'static str>,
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
            relates_to,
            ..
        } = *self;
        let slug = title.to_ascii_lowercase();
        let relation = relates_to
            .map(|target| format!("\n- relates_to [[{target}]]\n"))
            .unwrap_or_default();
        format!(
            "---\ntype: {engram_type}\ntitle: {title}\npermalink: {permalink}\ntags:\n  - eng\n  - {tag}\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\nA rule about {slug}.\n{relation}"
        )
    }
}

/// The seeded engrams: one at the root, one a folder down, one two down. The
/// nested two carry folder-shaped permalinks, so the detail route is exercised
/// on a permalink with slashes in it, and the types and tags differ so a
/// filtered listing has something to select on. Beta relates to Alpha, so the
/// domain holds one edge and the context endpoint has a neighborhood to return.
const FIXTURE_ENGRAMS: [FixtureEngram; 3] = [
    FixtureEngram {
        path: "alpha.md",
        title: "Alpha",
        permalink: "alpha",
        engram_type: "engram",
        tag: "root",
        relates_to: None,
    },
    FixtureEngram {
        path: "notes/beta.md",
        title: "Beta",
        permalink: "notes/beta",
        engram_type: "guide",
        tag: "nested",
        relates_to: Some("Alpha"),
    },
    FixtureEngram {
        path: "notes/deep/gamma.md",
        title: "Gamma",
        permalink: "notes/deep/gamma",
        engram_type: "engram",
        tag: "nested",
        relates_to: None,
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

/// The same fixture with one admin account, `root` / `rootpw`, already added
/// and logged in. Returns the fixture beside that session's cookie and CSRF
/// token, which every admin-route request needs.
async fn serve_as_admin(opts: AuthOptions) -> (Fixture, String, String) {
    let fixture = serve_with_auth(opts).await;
    fixture
        .auth
        .add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();
    let (token, csrf) = login(fixture.addr, "root", "rootpw").await;
    (fixture, token, csrf)
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

/// A request carrying a session's cookie and its CSRF token, the shape every
/// mutating request from the browser client has. The caller adds a body and
/// sends it.
fn as_session(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    token: &str,
    csrf: &str,
) -> reqwest::RequestBuilder {
    client()
        .request(method, format!("http://{addr}{path}"))
        .header("cookie", format!("fluid_session={token}"))
        .header("x-csrf-token", csrf)
}

/// The field names one user object carries, which is the whole contract: the
/// six columns the CLI's `users list --json` prints and nothing else.
fn user_fields(user: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = user
        .as_object()
        .expect("a user is an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The `name` of every account in a `{"users": [...]}` envelope.
fn user_names(body: &serde_json::Value) -> Vec<String> {
    body["users"]
        .as_array()
        .expect("the listing carries a users array")
        .iter()
        .map(|u| u["name"].as_str().unwrap().to_string())
        .collect()
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

/// The `permalink` of every node a context or graph response returned, in the
/// order it returned them: the anchors first, then the neighborhood by rank.
fn node_permalinks(context: &serde_json::Value) -> Vec<String> {
    context["nodes"]
        .as_array()
        .expect("a context response carries a nodes array")
        .iter()
        .map(|n| n["permalink"].as_str().unwrap().to_string())
        .collect()
}

/// The `name` of every entry in one of a vocabulary response's count lists.
fn names(list: &serde_json::Value) -> Vec<String> {
    list.as_array()
        .expect("a vocabulary list is an array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
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
    assert!(
        body["csrf"].is_null(),
        "the anonymous viewer has no session and so no token: {body}"
    );
}

/// The probe reissues the session's CSRF token, and it is the same one login
/// minted.
///
/// This is what makes a reload survivable. The session cookie is `HttpOnly`, so
/// a refreshed page still holds a live session but has lost the token login
/// handed it in a response body; without this it could neither log out nor
/// write, and its only way back would be logging in again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn me_reissues_the_sessions_csrf_token() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (token, csrf) = login(fixture.addr, "ada", "s3cret").await;
    let body: serde_json::Value = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["user"]["name"], "ada");
    assert_eq!(
        body["csrf"], csrf,
        "the probe hands back the token login minted: {body}"
    );

    // And the reissued token is usable, which is the whole point: a browser
    // that only ever saw this response can still send a mutating request.
    let resp = client()
        .post(format!("http://{}/api/v1/auth/logout", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .header("x-csrf-token", body["csrf"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "the reissued token passes the CSRF check"
    );
}

/// A request carrying no session gets no token, so a client cannot mistake an
/// unauthenticated probe for a usable one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn me_carries_no_csrf_token_without_a_session() {
    let (addr, _guard) = serve_test_router_with_fixture().await;
    let body: serde_json::Value = get(addr, "/api/v1/auth/me").await.json().await.unwrap();
    assert!(body["csrf"].is_null(), "{body}");
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
        // The document describing this API is guarded like the data it
        // describes: it is deliberately not in `PUBLIC_PATHS`, because handing
        // an unauthenticated caller every path and parameter would undo what
        // answering 401 ahead of routing is for. Tooling reads the committed
        // `crates/service/openapi/fluid-v1.json` instead, so nothing needs it
        // open.
        "/api/v1/openapi.json",
        // A path nothing mounts, which must answer 401 rather than 404. The
        // published document claims an unauthenticated caller never learns
        // which paths exist, and that claim rests on the fallback being
        // registered above the guard layer: without this line, moving it below
        // would start mapping the API out for anybody and no test would say so.
        "/api/v1/nope",
        "/api/v1/domains",
        "/api/v1/domains/eng/tree",
        "/api/v1/domains/eng/manifest",
        "/api/v1/domains/eng/engrams",
        "/api/v1/domains/eng/engrams/alpha",
        "/api/v1/domains/eng/engrams/notes/deep/gamma",
        "/api/v1/search",
        "/api/v1/vocabulary",
        "/api/v1/context",
        "/api/v1/activity",
        "/api/v1/graph",
        "/api/v1/users",
        "/api/v1/users/ada",
    ] {
        let resp = get(fixture.addr, path).await;
        assert_eq!(resp.status(), 401, "{path} must be guarded");
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], 401);
    }
}

/// The served document is the same one the snapshot pins, so a client that does
/// reach the route reads exactly what the committed artifact says.
///
/// Asserted over the wire rather than only in `openapi_snapshot.rs`, because
/// that test never mounts a router: this is what says the route is wired to
/// `openapi_document` and not to some second, drifting construction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openapi_route_serves_the_document_the_snapshot_pins() {
    let fixture = serve_with_ada(AuthOptions::default()).await;
    let (token, _) = login(fixture.addr, "ada", "s3cret").await;
    let resp = client()
        .get(format!("http://{}/api/v1/openapi.json", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let served: serde_json::Value = resp.json().await.unwrap();
    let generated = serde_json::to_value(crystalline_service::rest::openapi_document()).unwrap();
    assert_eq!(served, generated, "the route serves the document verbatim");
    assert_eq!(served["info"]["version"], "v1");
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
    let probe = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("remote-user", "Bob")
        .send()
        .await
        .unwrap();
    assert!(
        session_cookie(&probe).is_some(),
        "the probe mints a session for a trusted-header identity too"
    );
    let me: serde_json::Value = probe.json().await.unwrap();
    assert_eq!(me["user"]["name"], "bob", "the name is folded by the store");
    assert_eq!(me["user"]["display"], "Bob");
    assert_eq!(me["user"]["role"], "viewer");
    assert!(
        me["csrf"].as_str().is_some_and(|tok| !tok.is_empty()),
        "one CSRF rule for every identity mode: the probe hands a \
         trusted-header identity the token its mutating requests must echo. \
         See `check_csrf`. Body: {me}"
    );

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

/// A trusted-header value with internal whitespace cannot normalize into a
/// login name (see `auth_store::normalize_name`). Before this task that
/// refusal fell through the generic `anyhow` conversion and answered `500`;
/// the caller cannot fix the proxy's header, so it must be a `403` naming the
/// problem, not an opaque server error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_header_name_with_spaces_is_refused_as_403() {
    let fixture = serve_with_auth(AuthOptions {
        trusted_header: Some("remote-user"),
        ..AuthOptions::default()
    })
    .await;
    let resp = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("remote-user", "ada lovelace")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("whitespace"),
        "the message must be actionable, not opaque: {body}"
    );
    assert!(
        fixture.auth.list_users().await.unwrap().is_empty(),
        "no account was minted for a name that cannot normalize"
    );
}

/// `auth.max_users` bounds trusted-header provisioning: once the cap is
/// reached, a request naming a new identity is refused `403` rather than
/// minting past it, while an account that already exists keeps resolving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_header_provisioning_is_capped() {
    let fixture = serve_with_auth(AuthOptions {
        trusted_header: Some("remote-user"),
        max_users: Some(1),
        ..AuthOptions::default()
    })
    .await;

    let first = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("remote-user", "ada")
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200, "the first account is under the cap");

    let refused = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("remote-user", "bob")
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = refused.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("auth.max_users"),
        "the refusal names the setting: {body}"
    );

    // The existing account still resolves, whatever the count-vs-cap state.
    let still = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("remote-user", "ada")
        .send()
        .await
        .unwrap();
    assert_eq!(still.status(), 200);

    let names: Vec<String> = fixture
        .auth
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.name)
        .collect();
    assert_eq!(names, vec!["ada".to_string()]);
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
    assert_eq!(
        domains.len(),
        2,
        "the seeded domain and the empty one: {body}"
    );
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
    // A row says what state its engram is in, not only where it lives: a tree
    // is what a navigation sidebar is drawn from, and fading a retired engram
    // there would otherwise cost a second request per folder.
    let alpha = root["engrams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == "alpha.md")
        .unwrap_or_else(|| panic!("the root holds alpha: {root}"));
    assert_eq!(alpha["status"], "current", "{alpha}");
    assert_eq!(alpha["type"], "engram", "{alpha}");
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

/// The two states a client must be able to tell apart, which is why the
/// listing resolves the domain in its path rather than passing it to the
/// search filter and reporting whatever comes back: a domain nobody
/// registered is missing, and a registered domain that holds nothing is
/// empty. A filter that selects nothing is the empty case too - the path
/// segment names a resource, the query only narrows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_domain_lists_empty_and_an_unknown_one_is_a_404() {
    let fixture = serve_anonymous().await;

    let empty = get(fixture.addr, "/api/v1/domains/void/engrams").await;
    assert_eq!(empty.status(), 200, "a registered domain answers");
    let body: serde_json::Value = empty.json().await.unwrap();
    assert_eq!(body["total"], 0, "and holds nothing: {body}");
    assert!(hit_permalinks(&body).is_empty(), "{body}");

    let filtered: serde_json::Value = get(fixture.addr, "/api/v1/domains/eng/engrams?tags=nothing")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        filtered["total"], 0,
        "a filter that selects nothing is empty, not missing: {filtered}"
    );

    let unknown = get(fixture.addr, "/api/v1/domains/ghost/engrams").await;
    assert_eq!(unknown.status(), 404, "an unregistered domain is missing");
    assert_eq!(
        unknown.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = unknown.json().await.unwrap();
    assert_eq!(body["title"], "not found");
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("ghost"), "{detail}");
    assert!(detail.contains("eng"), "the valid set is named: {detail}");
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
        "/api/v1/domains/ghost/engrams",
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

/// Search is `search_engrams` behind a query string: the text selects on the
/// body rather than on a title, every filter narrows the same way the MCP tool's
/// does, and the engine's page envelope comes through unchanged.
///
/// The word searched for lives only in the fixture bodies, so a hit proves the
/// content was matched rather than the permalink the URL already carries. The
/// fixture has no embeddings and the test engine no provider, so hybrid resolves
/// to its text fallback and the response says so in `mode`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_finds_engrams_by_their_content() {
    let fixture = serve_anonymous().await;

    let resp = get(fixture.addr, "/api/v1/search?q=rule").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["mode"], "text",
        "hybrid falls back to text with no embeddings to search: {body}"
    );
    assert_eq!(body["page"], 1, "the page envelope is the engine's: {body}");
    assert_eq!(body["limit"], 10, "{body}");
    assert_eq!(
        body["total"], 3,
        "the three bodies, not the MANIFEST: {body}"
    );
    let mut found = hit_permalinks(&body);
    found.sort();
    assert_eq!(
        found,
        vec![
            "alpha".to_string(),
            "notes/beta".to_string(),
            "notes/deep/gamma".to_string()
        ],
        "{body}"
    );

    // Every filter, mapped one for one onto the search parameters.
    let typed: serde_json::Value = get(fixture.addr, "/api/v1/search?q=rule&type=guide")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(hit_permalinks(&typed), vec!["notes/beta".to_string()]);

    let tagged: serde_json::Value = get(fixture.addr, "/api/v1/search?q=rule&tags=eng,nested")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        tagged["total"], 2,
        "a comma list is split and every tag must match: {tagged}"
    );

    let statused: serde_json::Value = get(fixture.addr, "/api/v1/search?q=rule&status=draft")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(statused["total"], 0, "nothing is a draft here: {statused}");

    let recent: serde_json::Value = get(fixture.addr, "/api/v1/search?q=rule&after=2026-06-01")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        recent["total"], 0,
        "everything was recorded before that: {recent}"
    );

    let titled: serde_json::Value = get(fixture.addr, "/api/v1/search?q=Alpha&search_type=title")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        titled["mode"], "title",
        "the mode asked for is used: {titled}"
    );
    assert_eq!(hit_permalinks(&titled), vec!["alpha".to_string()]);

    let bounded: serde_json::Value = get(
        fixture.addr,
        "/api/v1/search?q=rule&search_type=text&min_similarity=0.5&limit=2&page=2",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(bounded["page"], 2, "{bounded}");
    assert_eq!(bounded["limit"], 2, "{bounded}");
    assert_eq!(bounded["total"], 3, "the total spans the pages: {bounded}");
    assert_eq!(bounded["count"], 1, "the tail of three: {bounded}");
}

/// A domain in the query string is a filter, not a resource: it narrows what is
/// searched and an unmatched name narrows it to nothing. That is what separates
/// it from a domain in a path segment, which names a resource and 404s when
/// nobody registered it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_filter_narrows_rather_than_404s() {
    let fixture = serve_anonymous().await;

    let scoped: serde_json::Value = get(fixture.addr, "/api/v1/search?q=rule&domains=eng")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(scoped["total"], 3, "{scoped}");

    let elsewhere: serde_json::Value = get(fixture.addr, "/api/v1/search?q=rule&domains=void")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        elsewhere["total"], 0,
        "the other registered domain holds nothing: {elsewhere}"
    );

    let unknown = get(fixture.addr, "/api/v1/search?q=rule&domains=ghost").await;
    assert_eq!(
        unknown.status(),
        200,
        "an unregistered name in a filter selects nothing, it does not 404"
    );
    let body: serde_json::Value = unknown.json().await.unwrap();
    assert_eq!(body["total"], 0, "{body}");

    // The list is split on commas, so both names are asked for at once.
    let both: serde_json::Value = get(fixture.addr, "/api/v1/search?q=rule&domains=eng,void")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(both["total"], 3, "{both}");

    let activity: serde_json::Value = get(fixture.addr, "/api/v1/activity?domains=ghost")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        activity["count"], 0,
        "the same rule on activity: {activity}"
    );

    let vocab = get(fixture.addr, "/api/v1/vocabulary?domain=ghost").await;
    assert_eq!(vocab.status(), 200, "and on the vocabulary");
    let body: serde_json::Value = vocab.json().await.unwrap();
    assert!(body["tags"].as_array().unwrap().is_empty(), "{body}");
}

/// A value the engine refuses is a 422 problem detail carrying the engine's own
/// message, so the caller is told which values do work rather than being handed
/// a bare status.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_search_type_is_a_422_carrying_the_engines_message() {
    let fixture = serve_anonymous().await;
    let resp = get(fixture.addr, "/api/v1/search?q=rule&search_type=nope").await;
    assert_eq!(resp.status(), 422);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 422);
    assert_eq!(body["title"], "invalid request");
    let detail = body["detail"].as_str().unwrap();
    assert!(
        detail.contains("unknown search_type 'nope'"),
        "the engine's own message arrives verbatim: {detail}"
    );
    assert!(
        detail.contains("hybrid, text, semantic, title or permalink"),
        "and it names what does work: {detail}"
    );
}

/// The vocabulary endpoint hands back what the domains are written in: the tags
/// in use with their counts, the observation categories and the relation types.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vocabulary_lists_the_tags_and_relation_types_in_use() {
    let fixture = serve_anonymous().await;

    let resp = get(fixture.addr, "/api/v1/vocabulary").await;
    assert_eq!(resp.status(), 200);
    let all: serde_json::Value = resp.json().await.unwrap();
    assert!(
        all["domain"].is_null(),
        "no domain was asked for, so none is echoed: {all}"
    );
    let tags = names(&all["tags"]);
    for tag in ["eng", "root", "nested", "manifest"] {
        assert!(tags.contains(&tag.to_string()), "{tag} missing from {all}");
    }
    let eng = all["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "eng")
        .expect("the shared tag is listed");
    assert_eq!(eng["engrams"], 3, "it tags the three seeded engrams: {all}");
    assert!(
        names(&all["relation_types"]).contains(&"relates_to".to_string()),
        "the one relation the fixture declares: {all}"
    );

    let scoped: serde_json::Value = get(fixture.addr, "/api/v1/vocabulary?domain=void")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(scoped["domain"], "void", "the scope is echoed: {scoped}");
    assert!(
        scoped["tags"].as_array().unwrap().is_empty(),
        "the empty domain is written in nothing yet: {scoped}"
    );
}

/// Context walks the graph out from a `crystalline://` anchor: the anchor comes
/// back as a seed node, its neighbors come back beside it, and the edges between
/// them say how they are related.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn context_returns_the_anchor_and_its_neighborhood() {
    let fixture = serve_anonymous().await;

    let resp = get(
        fixture.addr,
        "/api/v1/context?anchor=crystalline://eng/alpha",
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["anchor"], "crystalline://eng/alpha");
    assert_eq!(body["depth"], 1, "one hop by default: {body}");
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["permalink"], "alpha", "the anchor leads: {body}");
    assert_eq!(nodes[0]["seed"], true, "and is marked as the seed: {body}");
    assert!(
        node_permalinks(&body).contains(&"notes/beta".to_string()),
        "the engram relating to it comes with: {body}"
    );
    let edges = body["edges"].as_array().unwrap();
    assert!(
        edges.iter().any(|e| e["rel_type"] == "relates_to"),
        "the edge says how they are related: {body}"
    );

    // `max_related` caps the neighborhood, `domains` filters it, and `depth` is
    // passed through to the traversal.
    let alone: serde_json::Value = get(
        fixture.addr,
        "/api/v1/context?anchor=crystalline://eng/alpha&max_related=0&depth=2",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(alone["depth"], 2, "{alone}");
    assert_eq!(
        node_permalinks(&alone),
        vec!["alpha".to_string()],
        "the seed alone: {alone}"
    );

    let filtered: serde_json::Value = get(
        fixture.addr,
        "/api/v1/context?anchor=crystalline://eng/alpha&domains=void",
    )
    .await
    .json()
    .await
    .unwrap();
    assert!(
        node_permalinks(&filtered).is_empty(),
        "a domain filter that matches nothing keeps nothing: {filtered}"
    );
}

/// The three ways an anchor can be wrong, each answered in the same problem+json
/// contract: absent is a rejected request, unparseable is one the server
/// understood and refused, and one pointing at nothing is missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_context_anchor_is_a_problem_detail() {
    let fixture = serve_anonymous().await;

    let missing = get(fixture.addr, "/api/v1/context").await;
    assert_eq!(missing.status(), 400, "the anchor is required");
    assert_eq!(
        missing.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = missing.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("anchor"),
        "the detail names the parameter: {body}"
    );

    let malformed = get(fixture.addr, "/api/v1/context?anchor=alpha").await;
    assert_eq!(malformed.status(), 422);
    let body: serde_json::Value = malformed.json().await.unwrap();
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("not a crystalline:// URL"),
        "the engine's own message: {body}"
    );

    let unknown = get(
        fixture.addr,
        "/api/v1/context?anchor=crystalline://eng/ghost",
    )
    .await;
    assert_eq!(unknown.status(), 404);
    let body: serde_json::Value = unknown.json().await.unwrap();
    assert_eq!(body["title"], "not found");
    assert!(body["detail"].as_str().unwrap().contains("ghost"), "{body}");
}

/// Activity is the recency window behind a query string: the timeframe decides
/// what is recent enough to report, and the type and domain filters narrow it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_respects_the_timeframe_and_its_filters() {
    let fixture = serve_anonymous().await;

    // The fixture was recorded on a fixed past date, so a one-day window reaches
    // nothing and a wide one reaches everything: the window is what changes the
    // answer.
    let today = get(fixture.addr, "/api/v1/activity?timeframe=1d").await;
    assert_eq!(today.status(), 200);
    let body: serde_json::Value = today.json().await.unwrap();
    assert_eq!(body["timeframe"], "1d", "the window is echoed: {body}");
    assert_eq!(body["count"], 0, "nothing was recorded yesterday: {body}");

    let ever: serde_json::Value = get(fixture.addr, "/api/v1/activity?timeframe=20y")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        ever["count"], 4,
        "the MANIFEST and the three seeded engrams: {ever}"
    );
    let permalinks: Vec<&str> = ever["engrams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["permalink"].as_str().unwrap())
        .collect();
    assert!(permalinks.contains(&"alpha"), "{ever}");

    let typed: serde_json::Value = get(fixture.addr, "/api/v1/activity?timeframe=20y&types=guide")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(typed["count"], 1, "only the guide: {typed}");
    assert_eq!(typed["engrams"][0]["permalink"], "notes/beta");

    let multi: serde_json::Value = get(
        fixture.addr,
        "/api/v1/activity?timeframe=20y&types=guide,manifest&domains=eng",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(multi["count"], 2, "a comma list is split: {multi}");

    let default: serde_json::Value = get(fixture.addr, "/api/v1/activity")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        default["timeframe"], "7d",
        "the engine's own default, not a second one here: {default}"
    );
}

/// The graph endpoint answers the same neighborhood `/context` walks, in the
/// shape a renderer draws: nodes carrying what labels and styles them, edges
/// carrying their direction and relation type, and a flag saying whether the
/// node cap cut anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_returns_the_nodes_and_typed_edges_around_an_anchor() {
    let fixture = serve_anonymous().await;

    let resp = get(fixture.addr, "/api/v1/graph?anchor=crystalline://eng/alpha").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let permalinks = node_permalinks(&body);
    assert_eq!(permalinks[0], "alpha", "the anchor leads: {body}");
    assert!(
        permalinks.contains(&"notes/beta".to_string()),
        "the engram relating to it comes with: {body}"
    );
    assert_eq!(body["truncated"], false, "nothing was cut: {body}");

    // What a client draws a node with, all of it in the node itself.
    let anchor = &body["nodes"][0];
    assert_eq!(anchor["domain"], "eng");
    assert_eq!(anchor["title"], "Alpha");
    assert_eq!(anchor["status"], "current");
    assert_eq!(anchor["type"], "engram");
    let anchor_id = anchor["id"].as_i64().expect("a node carries its id");

    // The edge points from Beta, which declared the relation, at the anchor.
    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1, "the one relation the fixture holds: {body}");
    assert_eq!(edges[0]["rel_type"], "relates_to");
    assert_eq!(edges[0]["to"], anchor_id, "and it points at Alpha: {body}");

    // The node cap is a real bound, and a cut slice says it was cut.
    let capped: serde_json::Value = get(
        fixture.addr,
        "/api/v1/graph?anchor=crystalline://eng/alpha&max_nodes=1",
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(
        node_permalinks(&capped),
        vec!["alpha".to_string()],
        "the anchor alone: {capped}"
    );
    assert_eq!(capped["truncated"], true, "{capped}");
    assert!(
        capped["edges"].as_array().unwrap().is_empty(),
        "an edge whose other end was cut is cut too: {capped}"
    );

    // A second hop is served rather than refused, clamped or not.
    let deep = get(
        fixture.addr,
        "/api/v1/graph?anchor=crystalline://eng/alpha&depth=9",
    )
    .await;
    assert_eq!(
        deep.status(),
        200,
        "an over-deep request is clamped, not refused"
    );
}

/// The three ways an anchor can be wrong, answered here exactly as `/context`
/// answers them: a rejected request, one the server understood and refused, and
/// one pointing at nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_graph_anchor_is_a_problem_detail() {
    let fixture = serve_anonymous().await;

    let missing = get(fixture.addr, "/api/v1/graph").await;
    assert_eq!(missing.status(), 400, "the anchor is required");
    assert_eq!(
        missing.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = missing.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("anchor"),
        "the detail names the parameter: {body}"
    );

    let malformed = get(fixture.addr, "/api/v1/graph?anchor=alpha").await;
    assert_eq!(malformed.status(), 422);
    let body: serde_json::Value = malformed.json().await.unwrap();
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("not a crystalline:// URL"),
        "the engine's own message: {body}"
    );

    let unknown = get(fixture.addr, "/api/v1/graph?anchor=crystalline://eng/ghost").await;
    assert_eq!(unknown.status(), 404);
    assert_eq!(
        unknown.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = unknown.json().await.unwrap();
    assert_eq!(body["title"], "not found");
    assert!(body["detail"].as_str().unwrap().contains("ghost"), "{body}");
}

/// Every user route is admin-only, and the refusal is about the role rather
/// than about the CSRF token: each request below carries a valid one, so a
/// viewer and an editor are refused on what they are, not on what they sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_routes_are_refused_to_a_viewer_and_an_editor() {
    for role in [Role::Viewer, Role::Editor] {
        let fixture = serve_with_auth(AuthOptions::default()).await;
        fixture
            .auth
            .add_user("ada", "Ada", None, role, "s3cret")
            .await
            .unwrap();
        let (token, csrf) = login(fixture.addr, "ada", "s3cret").await;
        let cases = [
            (reqwest::Method::GET, "/api/v1/users", None),
            (
                reqwest::Method::POST,
                "/api/v1/users",
                Some(serde_json::json!({"name": "bob", "role": "viewer", "password": "hunter2"})),
            ),
            (
                reqwest::Method::PATCH,
                "/api/v1/users/ada",
                Some(serde_json::json!({"role": "admin"})),
            ),
            (reqwest::Method::DELETE, "/api/v1/users/ada", None),
        ];
        for (method, path, body) in cases {
            let mut request = as_session(fixture.addr, method.clone(), path, &token, &csrf);
            if let Some(body) = &body {
                request = request.json(body);
            }
            let resp = request.send().await.unwrap();
            assert_eq!(
                resp.status(),
                403,
                "a {role} must not reach {method} {path}"
            );
            assert_eq!(resp.headers()["content-type"], "application/problem+json");
            let detail: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(detail["status"], 403);
        }
        let users = fixture.auth.list_users().await.unwrap();
        assert_eq!(users.len(), 1, "nothing a {role} sent changed anything");
        assert_eq!(users[0].role, role, "including its own row");
    }
}

/// The admin round trip: create an account, see it in the listing, edit it,
/// and remove it again. The response carries the account as stored - the name
/// folded, no password material - and the listing is the `{"users": [...]}`
/// envelope the CLI's `users list --json` already prints.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_creates_lists_edits_and_removes_an_account() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;

    let created = as_session(addr, reqwest::Method::POST, "/api/v1/users", &token, &csrf)
        .json(&serde_json::json!({
            "name": "  BoB ",
            "display": "Bob",
            "email": "bob@example.com",
            "role": "editor",
            "password": "hunter2",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let body: serde_json::Value = created.json().await.unwrap();
    assert_eq!(
        body["user"]["name"], "bob",
        "the store folds the name: {body}"
    );
    assert_eq!(body["user"]["display"], "Bob");
    assert_eq!(body["user"]["email"], "bob@example.com");
    assert_eq!(body["user"]["role"], "editor");
    assert_eq!(body["user"]["disabled"], false);
    assert_eq!(
        user_fields(&body["user"]),
        vec!["disabled", "display", "email", "last_seen", "name", "role"],
        "no password material may reach the client: {body}"
    );

    let listed = as_session(addr, reqwest::Method::GET, "/api/v1/users", &token, &csrf)
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    let body: serde_json::Value = listed.json().await.unwrap();
    assert_eq!(
        user_names(&body),
        vec!["bob".to_string(), "root".to_string()],
        "every account, by name: {body}"
    );
    assert_eq!(
        user_fields(&body["users"][0]),
        vec!["disabled", "display", "email", "last_seen", "name", "role"],
        "the listing carries no hashes either: {body}"
    );

    let patched = as_session(
        addr,
        reqwest::Method::PATCH,
        "/api/v1/users/bob",
        &token,
        &csrf,
    )
    .json(&serde_json::json!({"role": "viewer", "disabled": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(patched.status(), 200);
    let body: serde_json::Value = patched.json().await.unwrap();
    assert_eq!(body["user"]["role"], "viewer", "{body}");
    assert_eq!(body["user"]["disabled"], true, "{body}");

    // A password change lands too, which the store only proves by accepting it
    // at login - so the account is re-enabled and asked to log in with it.
    let repaired = as_session(
        addr,
        reqwest::Method::PATCH,
        "/api/v1/users/bob",
        &token,
        &csrf,
    )
    .json(&serde_json::json!({"disabled": false, "password": "corrected horse"}))
    .send()
    .await
    .unwrap();
    assert_eq!(repaired.status(), 200);
    let (bob_token, _) = login(addr, "bob", "corrected horse").await;
    let me: serde_json::Value = client()
        .get(format!("http://{addr}/api/v1/auth/me"))
        .header("cookie", format!("fluid_session={bob_token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["name"], "bob", "the new password works: {me}");

    let removed = as_session(
        addr,
        reqwest::Method::DELETE,
        "/api/v1/users/bob",
        &token,
        &csrf,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(removed.status(), 204);
    assert!(
        removed.bytes().await.unwrap().is_empty(),
        "204 carries no body"
    );

    let body: serde_json::Value =
        as_session(addr, reqwest::Method::GET, "/api/v1/users", &token, &csrf)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(user_names(&body), vec!["root".to_string()], "{body}");
}

/// An admin cannot lock itself out by hand: deleting or disabling its own
/// account is refused as a conflict, and the refusal reads differently from the
/// last-admin one - here another admin exists, so the installation is in no
/// danger and only the caller's own session is being protected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_cannot_delete_or_disable_itself() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;
    fixture
        .auth
        .add_user("ada", "Ada", None, Role::Admin, "s3cret")
        .await
        .unwrap();

    for (method, path, body) in [
        (reqwest::Method::DELETE, "/api/v1/users/root", None),
        (
            reqwest::Method::PATCH,
            "/api/v1/users/root",
            Some(serde_json::json!({"disabled": true})),
        ),
        // The same account by another spelling: the comparison folds, so a
        // capitalized path is not a way around the check.
        (reqwest::Method::DELETE, "/api/v1/users/ROOT", None),
    ] {
        let mut request = as_session(addr, method.clone(), path, &token, &csrf);
        if let Some(body) = &body {
            request = request.json(body);
        }
        let resp = request.send().await.unwrap();
        assert_eq!(resp.status(), 409, "{method} {path} must be refused");
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["title"], "conflict");
        let detail = body["detail"].as_str().unwrap();
        assert!(
            detail.contains("your own account"),
            "the refusal says whose account it is: {body}"
        );
        assert!(
            !detail.contains("last admin"),
            "and it is not the last-admin refusal: {body}"
        );
    }

    let users = fixture.auth.list_users().await.unwrap();
    let root = users.iter().find(|u| u.name == "root").unwrap();
    assert!(!root.disabled, "the account is untouched and still enabled");
    assert_eq!(root.role, Role::Admin);
}

/// The store's last-admin guard reaches the wire as a 409 carrying its own
/// message. There is deliberately no escape hatch: an installation must never
/// be lockable out of its own user management over HTTP, and the CLI is the
/// recovery path. Self-demotion is the way to ask for it, since any other admin
/// caller would itself be a remaining admin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demoting_the_last_admin_is_refused_with_the_stores_message() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;

    let refused = as_session(
        addr,
        reqwest::Method::PATCH,
        "/api/v1/users/root",
        &token,
        &csrf,
    )
    .json(&serde_json::json!({"role": "viewer"}))
    .send()
    .await
    .unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(
        refused.headers()["content-type"],
        "application/problem+json"
    );
    let body: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(body["title"], "conflict");
    let detail = body["detail"].as_str().unwrap();
    assert!(
        detail.contains("last admin"),
        "the store's own words: {body}"
    );
    assert!(
        detail.contains("add or enable another admin first"),
        "including what to do about it: {body}"
    );

    let users = fixture.auth.list_users().await.unwrap();
    assert_eq!(users[0].role, Role::Admin, "the demotion did not land");

    // With a second admin in place the same request goes through, so what
    // refused it was the guard and not the route.
    fixture
        .auth
        .add_user("ada", "Ada", None, Role::Admin, "s3cret")
        .await
        .unwrap();
    let ok = as_session(
        addr,
        reqwest::Method::PATCH,
        "/api/v1/users/root",
        &token,
        &csrf,
    )
    .json(&serde_json::json!({"role": "viewer"}))
    .send()
    .await
    .unwrap();
    assert_eq!(ok.status(), 200);
    let body: serde_json::Value = ok.json().await.unwrap();
    assert_eq!(body["user"]["role"], "viewer", "{body}");
}

/// A mutating admin request from a cookie session carries the CSRF token like
/// every other one: without it the middleware refuses before the handler runs,
/// and no account is created.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creating_an_account_needs_the_csrf_token() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;
    let body = serde_json::json!({"name": "bob", "role": "viewer", "password": "hunter2"});

    let refused = client()
        .post(format!("http://{addr}/api/v1/users"))
        .header("cookie", format!("fluid_session={token}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.headers()["content-type"],
        "application/problem+json"
    );
    assert_eq!(
        fixture.auth.list_users().await.unwrap().len(),
        1,
        "the refused request created nothing"
    );

    let ok = as_session(addr, reqwest::Method::POST, "/api/v1/users", &token, &csrf)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 201, "with the token it goes through");
}

/// The settlement, from the other side: a trusted-header admin that has not
/// called `/auth/me` carries no token, and every mutating request it sends is
/// refused - the JSON one it means to send as much as the form-shaped one a
/// cross-site page could. Before this task the header alone was enough and what
/// kept a cross-site form off these routes was the JSON content type the API
/// demands; that argument is now a second line of defence rather than the only
/// one, so a request that clears it is still refused without the token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_header_mutation_without_a_token_is_refused() {
    let fixture = serve_with_auth(AuthOptions {
        trusted_header: Some("remote-user"),
        ..AuthOptions::default()
    })
    .await;
    let addr = fixture.addr;
    fixture
        .auth
        .ensure_user("proxy", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    fixture.auth.set_role("proxy", Role::Admin).await.unwrap();
    let url = format!("http://{addr}/api/v1/users");

    for content_type in [
        "application/x-www-form-urlencoded",
        "text/plain",
        "application/json",
    ] {
        let resp = client()
            .post(&url)
            .header("remote-user", "proxy")
            .header("content-type", content_type)
            .body(r#"{"name":"bob","role":"admin","password":"hunter2"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            403,
            "a {content_type} body must not be acted on without the token"
        );
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body["detail"].as_str().unwrap().contains("/auth/me"),
            "the refusal says where the token comes from: {body}"
        );
    }
    let names: Vec<String> = fixture
        .auth
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.name)
        .collect();
    assert_eq!(names, vec!["proxy".to_string()], "nothing was created");
}

/// The settlement end to end: a trusted-header admin is minted a session by
/// the probe, and only the minted token authorizes a mutation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_header_identity_is_minted_a_csrf_token_by_the_probe() {
    let fixture = serve_with_auth(AuthOptions {
        trusted_header: Some("x-forwarded-user"),
        ..AuthOptions::default()
    })
    .await;
    fixture
        .auth
        .add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();

    // Without the probe: refused, told where the token comes from.
    let refused = client()
        .post(format!("http://{}/api/v1/users", fixture.addr))
        .header("x-forwarded-user", "root")
        .json(&serde_json::json!({"name": "bob", "role": "viewer", "password": "pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);

    // The probe mints a session and hands the token back.
    let probe = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("x-forwarded-user", "root")
        .send()
        .await
        .unwrap();
    let cookie = session_cookie(&probe).expect("the probe set a session cookie");
    let body: serde_json::Value = probe.json().await.unwrap();
    let csrf = body["csrf"].as_str().expect("the probe carries the token");

    // Header + cookie + token: the mutation goes through.
    let created = client()
        .post(format!("http://{}/api/v1/users", fixture.addr))
        .header("x-forwarded-user", "root")
        .header("cookie", format!("fluid_session={cookie}"))
        .header("x-csrf-token", csrf)
        .json(&serde_json::json!({"name": "bob", "role": "viewer", "password": "pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);

    // A second probe with the cookie reissues the same token, not a new session.
    let again = client()
        .get(format!("http://{}/api/v1/auth/me", fixture.addr))
        .header("x-forwarded-user", "root")
        .header("cookie", format!("fluid_session={cookie}"))
        .send()
        .await
        .unwrap();
    assert!(
        session_cookie(&again).is_none(),
        "no second mint while the session lives"
    );
    let again: serde_json::Value = again.json().await.unwrap();
    assert_eq!(again["csrf"].as_str().unwrap(), csrf);
}

/// The anonymous viewer never reaches these routes, which is what keeps the
/// CSRF-less anonymous identity off every mutating admin path: it has no
/// account, so it is told to authenticate rather than being served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_anonymous_viewer_cannot_reach_the_user_routes() {
    let fixture = serve_anonymous().await;
    let addr = fixture.addr;

    let listed = get(addr, "/api/v1/users").await;
    assert_eq!(listed.status(), 401);
    assert_eq!(listed.headers()["content-type"], "application/problem+json");

    let created = client()
        .post(format!("http://{addr}/api/v1/users"))
        .json(&serde_json::json!({"name": "bob", "role": "admin", "password": "hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 401);
    assert!(
        fixture.auth.list_users().await.unwrap().is_empty(),
        "nothing was created"
    );
}

/// The ways a user request can be wrong, each answered with the status a client
/// can branch on: a name already taken is a conflict, an account nobody created
/// is a 404, and a request the server understood but cannot act on is a 422.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_request_failures_are_classified() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;

    let duplicate = as_session(addr, reqwest::Method::POST, "/api/v1/users", &token, &csrf)
        .json(&serde_json::json!({"name": "Root", "role": "admin", "password": "hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 409, "the name is taken");
    let body: serde_json::Value = duplicate.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("root"),
        "the refusal names the account: {body}"
    );
    assert!(
        !body["detail"].as_str().unwrap().contains("UNIQUE"),
        "in product copy rather than in the database's words: {body}"
    );

    for (path, body, expected) in [
        (
            "/api/v1/users",
            serde_json::json!({"name": "  ", "role": "viewer", "password": "hunter2"}),
            422,
        ),
        (
            "/api/v1/users",
            serde_json::json!({"name": "bob", "role": "viewer", "password": ""}),
            422,
        ),
        (
            "/api/v1/users",
            serde_json::json!({"name": "bob", "role": "wizard", "password": "hunter2"}),
            422,
        ),
    ] {
        let resp = as_session(addr, reqwest::Method::POST, path, &token, &csrf)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), expected, "POST {body} must be refused");
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
    }

    for (method, body) in [
        (
            reqwest::Method::PATCH,
            Some(serde_json::json!({"role": "viewer"})),
        ),
        (reqwest::Method::DELETE, None),
    ] {
        let mut request = as_session(addr, method.clone(), "/api/v1/users/ghost", &token, &csrf);
        if let Some(body) = &body {
            request = request.json(body);
        }
        let resp = request.send().await.unwrap();
        assert_eq!(resp.status(), 404, "{method} on an unknown account");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["title"], "not found");
        assert!(body["detail"].as_str().unwrap().contains("ghost"), "{body}");
    }

    let nothing = as_session(
        addr,
        reqwest::Method::PATCH,
        "/api/v1/users/root",
        &token,
        &csrf,
    )
    .json(&serde_json::json!({}))
    .send()
    .await
    .unwrap();
    assert_eq!(nothing.status(), 422, "a patch that changes nothing");
    let body: serde_json::Value = nothing.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("role"),
        "the detail names what it takes: {body}"
    );
}

/// An admin resetting an account's password signs that account out. A session
/// never presents a password again, so a reset that left the old cookie live
/// would evict nobody - which is the whole point of resetting one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_password_reset_revokes_the_targets_sessions() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;
    fixture
        .auth
        .add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();
    let (ada, _) = login(addr, "ada", "s3cret").await;
    let signed_in = client()
        .get(format!("http://{addr}/api/v1/domains"))
        .header("cookie", format!("fluid_session={ada}"))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_in.status(), 200, "the session works to begin with");

    let reset = as_session(
        addr,
        reqwest::Method::PATCH,
        "/api/v1/users/ada",
        &token,
        &csrf,
    )
    .json(&serde_json::json!({"password": "corrected horse"}))
    .send()
    .await
    .unwrap();
    assert_eq!(reset.status(), 200);

    let after = client()
        .get(format!("http://{addr}/api/v1/domains"))
        .header("cookie", format!("fluid_session={ada}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        401,
        "the cookie from before the reset is dead"
    );
    // And the account is usable again with the password the admin set.
    login(addr, "ada", "corrected horse").await;
}

/// Disabling revokes rather than hides: re-enabling the account must not hand
/// back the cookies it held, or disabling a compromised account and enabling it
/// again would restore the intruder's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_and_re_enabling_does_not_restore_the_old_sessions() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;
    fixture
        .auth
        .add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();
    let (ada, _) = login(addr, "ada", "s3cret").await;

    for disabled in [true, false] {
        let resp = as_session(
            addr,
            reqwest::Method::PATCH,
            "/api/v1/users/ada",
            &token,
            &csrf,
        )
        .json(&serde_json::json!({ "disabled": disabled }))
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), 200, "setting disabled={disabled}");
    }

    let after = client()
        .get(format!("http://{addr}/api/v1/domains"))
        .header("cookie", format!("fluid_session={ada}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        401,
        "the session was deleted, not hidden while the flag was set"
    );
    // The account is enabled again, so a fresh login works.
    login(addr, "ada", "s3cret").await;
}

/// Creating accounts is argon2 work like logging in, and it goes through the
/// same limiter. Asserted as liveness rather than as timing: every concurrent
/// request is answered and every account exists afterwards, which is what a
/// limiter that queues (rather than drops or deadlocks) looks like from
/// outside. The bound itself is asserted on the mechanism in
/// `rest::tests::the_login_limiter_caps_every_caller_that_hashes`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_account_creations_all_answer() {
    let (fixture, token, csrf) = serve_as_admin(AuthOptions::default()).await;
    let addr = fixture.addr;

    let mut tasks = Vec::new();
    for n in 0..12 {
        let (token, csrf) = (token.clone(), csrf.clone());
        tasks.push(tokio::spawn(async move {
            as_session(addr, reqwest::Method::POST, "/api/v1/users", &token, &csrf)
                .json(&serde_json::json!({
                    "name": format!("user{n}"),
                    "role": "viewer",
                    "password": "hunter2",
                }))
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), 201, "every creation is answered");
    }
    assert_eq!(
        fixture.auth.list_users().await.unwrap().len(),
        13,
        "twelve new accounts beside the admin"
    );
}
