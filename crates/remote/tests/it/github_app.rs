//! Offline mock-server tests for `GithubApp`: the installation lookup, the
//! installation tokens and their cache, the bot identity and the GitHub
//! Enterprise Server base url. Every test spins up a throwaway axum server on
//! an ephemeral localhost port, mirroring `tests/it/github_client.rs`, so the
//! suite never touches the real network.
//!
//! The App's clock is injected and pinned, so the token expiry is decided by
//! the test and never by a sleep.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, TimeZone, Utc};
use crystalline_remote::github::app::{Collaborator, GithubApp, RepoRole, RepoVisibility};
use crystalline_remote::{AppCredentials, CommitIdentity, RemoteError};
use tokio::net::TcpListener;

/// The throwaway test key of `tests/fixtures`; it belongs to no App.
const PEM: &str = include_str!("../fixtures/app-key-pkcs1.pem");

const SLUG: &str = "crystalline-acme";

/// Starts `router` on an ephemeral localhost port and returns its base url,
/// same pattern as `tests/it/github_client.rs`.
async fn spawn(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

/// The instant every test's clock starts at.
fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap()
}

/// The host a loopback base url stands for, as `AppCredentials.host` holds
/// it: the bare `host:port`.
fn host_of(base: &str) -> String {
    base.trim_start_matches("http://").to_string()
}

fn creds(host: &str, slug: Option<&str>) -> AppCredentials {
    AppCredentials {
        app_id: 7,
        client_id: "Iv23.test".to_string(),
        client_secret: None,
        private_key: PEM.to_string(),
        webhook_secret: None,
        slug: slug.map(str::to_string),
        host: host.to_string(),
    }
}

/// A settable clock shared between a test and its App.
#[derive(Clone)]
struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    fn new() -> TestClock {
        TestClock(Arc::new(Mutex::new(t0())))
    }

    fn set(&self, at: DateTime<Utc>) {
        *self.0.lock().unwrap() = at;
    }

    fn install(&self, app: GithubApp) -> GithubApp {
        let clock = self.0.clone();
        app.with_clock(Arc::new(move || *clock.lock().unwrap()))
    }
}

/// What the stand-in GitHub saw and how it answers.
#[derive(Default)]
struct Mock {
    /// `owner/name` -> installation id; a repository missing here answers 404.
    installations: HashMap<String, u64>,
    /// How long after `t0` a minted token expires.
    lifetime: Option<Duration>,
    /// Sent as `X-GitHub-Enterprise-Version` on every answer when set.
    ghes_version: Option<&'static str>,
    /// Every request, as `(METHOD raw-path, headers)`.
    seen: Mutex<Vec<(String, HeaderMap)>>,
    /// Token POSTs per installation.
    minted: Mutex<HashMap<u64, usize>>,
}

impl Mock {
    fn record(&self, method: &str, uri: &Uri, headers: &HeaderMap) {
        self.seen
            .lock()
            .unwrap()
            .push((format!("{method} {}", uri.path()), headers.clone()));
    }

    fn count(&self, prefix: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(line, _)| line.starts_with(prefix))
            .count()
    }

    fn headers_of(&self, prefix: &str) -> HeaderMap {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|(line, _)| line.starts_with(prefix))
            .map(|(_, h)| h.clone())
            .unwrap_or_else(|| panic!("no request starting with {prefix}"))
    }

    fn minted(&self, id: u64) -> usize {
        self.minted.lock().unwrap().get(&id).copied().unwrap_or(0)
    }

    fn answer(&self, status: StatusCode, body: serde_json::Value) -> Response {
        let mut response = (status, Json(body)).into_response();
        if let Some(version) = self.ghes_version {
            response
                .headers_mut()
                .insert("X-GitHub-Enterprise-Version", version.parse().unwrap());
        }
        response
    }
}

fn not_found() -> serde_json::Value {
    serde_json::json!({"message": "Not Found", "documentation_url": "https://docs.github.com/rest"})
}

async fn app_handler(State(mock): State<Arc<Mock>>, uri: Uri, headers: HeaderMap) -> Response {
    mock.record("GET", &uri, &headers);
    mock.answer(
        StatusCode::OK,
        serde_json::json!({
            "id": 7,
            "slug": SLUG,
            "owner": {"login": "acme"},
            "html_url": format!("https://github.com/apps/{SLUG}"),
        }),
    )
}

async fn installation_handler(
    State(mock): State<Arc<Mock>>,
    Path((owner, name)): Path<(String, String)>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    mock.record("GET", &uri, &headers);
    match mock.installations.get(&format!("{owner}/{name}")) {
        Some(id) => mock.answer(StatusCode::OK, serde_json::json!({"id": id})),
        None => mock.answer(StatusCode::NOT_FOUND, not_found()),
    }
}

async fn token_handler(
    State(mock): State<Arc<Mock>>,
    Path(id): Path<u64>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    mock.record("POST", &uri, &headers);
    // Long enough that concurrent callers really overlap the mint.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let n = {
        let mut minted = mock.minted.lock().unwrap();
        let n = minted.entry(id).or_default();
        *n += 1;
        *n
    };
    let expires_at = t0() + mock.lifetime.unwrap_or(Duration::hours(1));
    mock.answer(
        StatusCode::CREATED,
        serde_json::json!({
            "token": format!("ghs_{id}_{n}"),
            "expires_at": expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }),
    )
}

async fn user_handler(
    State(mock): State<Arc<Mock>>,
    Path(login): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    mock.record("GET", &uri, &headers);
    mock.answer(
        StatusCode::OK,
        serde_json::json!({"login": login, "id": 41898282, "type": "Bot"}),
    )
}

async fn fallback(State(mock): State<Arc<Mock>>, uri: Uri, headers: HeaderMap) -> Response {
    mock.record("UNROUTED", &uri, &headers);
    (StatusCode::IM_A_TEAPOT, "no such route in the mock").into_response()
}

/// The stand-in GitHub, its routes under `prefix` (`""` for github.com,
/// `"/api/v3"` for an Enterprise Server).
fn router(mock: Arc<Mock>, prefix: &str) -> Router {
    Router::new()
        .route(&format!("{prefix}/app"), get(app_handler))
        .route(
            &format!("{prefix}/repos/{{owner}}/{{name}}/installation"),
            get(installation_handler),
        )
        .route(
            &format!("{prefix}/app/installations/{{id}}/access_tokens"),
            post(token_handler),
        )
        .route(&format!("{prefix}/users/{{login}}"), get(user_handler))
        .fallback(fallback)
        .with_state(mock)
}

fn mock_with(installations: &[(&str, u64)]) -> Mock {
    Mock {
        installations: installations
            .iter()
            .map(|(repo, id)| (repo.to_string(), *id))
            .collect(),
        ..Mock::default()
    }
}

/// Serves `mock` on github.com-shaped paths and returns it, the base url and
/// an App with a pinned clock pointed at it.
async fn serve(mock: Mock) -> (Arc<Mock>, String, Arc<GithubApp>, TestClock) {
    let mock = Arc::new(mock);
    let base = spawn(router(mock.clone(), "")).await;
    let clock = TestClock::new();
    let app = GithubApp::new(Some(base.clone()), creds(&host_of(&base), Some(SLUG))).unwrap();
    (mock, base, Arc::new(clock.install(app)), clock)
}

fn jwt_issuer(authorization: &str) -> String {
    let jwt = authorization
        .strip_prefix("Bearer ")
        .expect("a bearer token");
    let claims = jwt.split('.').nth(1).expect("a three-part JWT");
    let claims: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap();
    claims["iss"].as_str().unwrap().to_string()
}

// --- tokens ----------------------------------------------------------------

#[tokio::test]
async fn the_installation_token_request_carries_a_bearer_jwt_and_api_version() {
    let (mock, _base, app, _clock) = serve(mock_with(&[("acme/knowledge", 11)])).await;

    let token = app.token_for_repo("acme/knowledge").await.unwrap();
    assert_eq!(token, "ghs_11_1");

    for prefix in [
        "GET /repos/acme/knowledge/installation",
        "POST /app/installations/11/access_tokens",
    ] {
        let headers = mock.headers_of(prefix);
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert_eq!(jwt_issuer(auth), "Iv23.test", "{prefix}");
        assert_eq!(
            headers.get("x-github-api-version").unwrap(),
            "2022-11-28",
            "{prefix}"
        );
        assert_eq!(
            headers.get("accept").unwrap(),
            "application/vnd.github+json",
            "{prefix}"
        );
        assert!(headers.get("user-agent").is_some(), "{prefix}");
    }
}

#[tokio::test]
async fn two_installations_get_two_tokens() {
    let (mock, _base, app, _clock) = serve(mock_with(&[
        ("acme/knowledge", 11),
        ("acme/handbook", 11),
        ("globex/notes", 22),
    ]))
    .await;

    let acme = app.token_for_repo("acme/knowledge").await.unwrap();
    let handbook = app.token_for_repo("acme/handbook").await.unwrap();
    let globex = app.token_for_repo("globex/notes").await.unwrap();

    assert_eq!(acme, "ghs_11_1");
    assert_eq!(handbook, acme, "one installation, one token");
    assert_eq!(globex, "ghs_22_1");
    assert_eq!(mock.minted(11), 1);
    assert_eq!(mock.minted(22), 1);
    assert_eq!(app.installation_for("acme/knowledge").await.unwrap(), 11);
    assert_eq!(app.installation_for("globex/notes").await.unwrap(), 22);
    assert_eq!(
        mock.count("GET /repos/acme/knowledge/installation"),
        1,
        "the lookup is cached"
    );
}

#[tokio::test]
async fn a_token_is_reused_until_five_minutes_before_expiry() {
    // Expires an hour out: the second call reuses the first token.
    let (mock, _base, app, clock) = serve(mock_with(&[])).await;
    assert_eq!(app.installation_token(11).await.unwrap(), "ghs_11_1");
    assert_eq!(app.installation_token(11).await.unwrap(), "ghs_11_1");
    assert_eq!(mock.minted(11), 1);

    // Still reused a second before the five-minute margin...
    clock.set(t0() + Duration::minutes(55) - Duration::seconds(1));
    assert_eq!(app.installation_token(11).await.unwrap(), "ghs_11_1");
    assert_eq!(mock.minted(11), 1);
    // ...and renewed once the margin is reached.
    clock.set(t0() + Duration::minutes(55));
    assert_eq!(app.installation_token(11).await.unwrap(), "ghs_11_2");
    assert_eq!(mock.minted(11), 2);

    // Expires four minutes out: inside the margin, so every call mints.
    let (mock, _base, app, _clock) = serve(Mock {
        lifetime: Some(Duration::minutes(4)),
        ..Mock::default()
    })
    .await;
    assert_eq!(app.installation_token(11).await.unwrap(), "ghs_11_1");
    assert_eq!(app.installation_token(11).await.unwrap(), "ghs_11_2");
    assert_eq!(mock.minted(11), 2);
}

#[tokio::test]
async fn concurrent_requests_for_one_installation_mint_one_token() {
    let (mock, _base, app, _clock) = serve(mock_with(&[])).await;

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let app = app.clone();
        set.spawn(async move { app.installation_token(11).await });
    }
    let tokens: Vec<String> = set
        .join_all()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(tokens.len(), 8);
    assert!(tokens.iter().all(|t| t == "ghs_11_1"), "{tokens:?}");
    assert_eq!(mock.minted(11), 1);
}

#[tokio::test]
async fn evicting_forces_a_fresh_lookup_and_a_fresh_token() {
    let (mock, _base, app, _clock) = serve(mock_with(&[("acme/knowledge", 11)])).await;
    app.token_for_repo("acme/knowledge").await.unwrap();

    app.evict_repo("acme/knowledge");
    assert_eq!(
        app.token_for_repo("acme/knowledge").await.unwrap(),
        "ghs_11_1",
        "evicting the repository keeps the installation's token"
    );
    assert_eq!(mock.count("GET /repos/acme/knowledge/installation"), 2);

    app.evict_installation(11);
    assert_eq!(
        app.token_for_repo("acme/knowledge").await.unwrap(),
        "ghs_11_2"
    );
    assert_eq!(mock.count("GET /repos/acme/knowledge/installation"), 3);
    assert_eq!(mock.minted(11), 2);
}

// --- not installed and the install url --------------------------------------

#[tokio::test]
async fn a_repository_without_installation_is_app_not_installed_with_the_install_url() {
    // GitHub Enterprise Server sends its version header on every answer, a
    // 404 for a missing installation included: that must not read as a
    // missing endpoint.
    let mock = Arc::new(Mock {
        ghes_version: Some("3.14.2"),
        ..Mock::default()
    });
    let base = spawn(router(mock.clone(), "")).await;
    // No slug saved: the install url needs one, so `GET /app` supplies it.
    let app = GithubApp::new(Some(base.clone()), creds(&host_of(&base), None)).unwrap();

    let err = app.token_for_repo("acme/missing").await.unwrap_err();
    let install_url = format!("{base}/apps/{SLUG}/installations/new");
    match &err {
        RemoteError::AppNotInstalled {
            repo,
            install_url: url,
        } => {
            assert_eq!(repo, "acme/missing");
            assert_eq!(url, &install_url);
        }
        other => panic!("expected AppNotInstalled, got {other:?}"),
    }
    assert_eq!(
        err.to_string(),
        format!(
            "the GitHub App is not installed on acme/missing: install it at {install_url}, then try again"
        )
    );
    assert_eq!(mock.count("POST"), 0, "no token is minted for it");

    assert_eq!(app.install_url().await.unwrap(), install_url);
    assert_eq!(mock.count("GET /app"), 1, "the slug is looked up once");
}

#[tokio::test]
async fn install_url_uses_the_saved_slug_without_a_request() {
    let (mock, base, app, _clock) = serve(Mock::default()).await;
    assert_eq!(
        app.install_url().await.unwrap(),
        format!("{base}/apps/{SLUG}/installations/new")
    );
    assert_eq!(mock.count(""), 0);
}

// --- GitHub Enterprise Server ------------------------------------------------

#[tokio::test]
async fn ghes_base_url_is_used_for_every_app_call() {
    let mock = Arc::new(mock_with(&[("acme/knowledge", 11)]));
    let base = spawn(router(mock.clone(), "/api/v3")).await;
    // Pinned, so the minted token (which expires an hour after `t0`) is
    // fresh whatever the wall clock says.
    let app = TestClock::new().install(
        GithubApp::new(
            Some(format!("{base}/api/v3")),
            creds(&host_of(&base), Some(SLUG)),
        )
        .unwrap(),
    );

    let info = app.app_info().await.unwrap();
    assert_eq!(info.slug, SLUG);
    assert_eq!(
        app.token_for_repo("acme/knowledge").await.unwrap(),
        "ghs_11_1"
    );
    app.bot_identity("acme/knowledge").await.unwrap();
    assert!(app.token_for_repo("acme/missing").await.is_err());

    assert_eq!(mock.count("UNROUTED"), 0, "every call stays under /api/v3");
    assert_eq!(mock.count("GET /api/v3/app"), 1);
    assert_eq!(
        mock.count("POST /api/v3/app/installations/11/access_tokens"),
        1
    );
    assert_eq!(mock.count("GET /api/v3/users/"), 1);
    // The web pages live on the bare host, not under /api/v3.
    assert_eq!(
        app.install_url().await.unwrap(),
        format!("{base}/apps/{SLUG}/installations/new")
    );
}

#[tokio::test]
async fn a_missing_endpoint_on_ghes_reports_the_version_header() {
    let app_router = Router::new().route(
        "/api/v3/app",
        get(|| async {
            (
                StatusCode::NOT_FOUND,
                [("X-GitHub-Enterprise-Version", "3.14.2")],
                Json(not_found()),
            )
        }),
    );
    let base = spawn(app_router).await;
    let app = GithubApp::new(
        Some(format!("{base}/api/v3")),
        creds(&host_of(&base), Some(SLUG)),
    )
    .unwrap();

    let err = app.app_info().await.unwrap_err();
    match &err {
        RemoteError::EndpointMissing {
            endpoint,
            ghes_version,
        } => {
            assert_eq!(endpoint, "GET /app");
            assert_eq!(ghes_version.as_deref(), Some("3.14.2"));
        }
        other => panic!("expected EndpointMissing, got {other:?}"),
    }
    let text = err.to_string();
    assert!(
        text.contains("GET /app") && text.contains("3.14.2"),
        "{text}"
    );
}

#[tokio::test]
async fn a_saved_app_is_refused_against_another_host() {
    // A github.com App against an Enterprise Server url.
    let err = GithubApp::new(
        Some("https://ghe.example.com/api/v3".to_string()),
        creds("github.com", Some(SLUG)),
    )
    .unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, RemoteError::AppCredential(_)), "{err:?}");
    assert!(
        text.contains("github.com") && text.contains("ghe.example.com"),
        "{text}"
    );

    // An Enterprise Server App against github.com.
    for api_url in [None, Some("https://api.github.com".to_string())] {
        let err = GithubApp::new(api_url, creds("ghe.example.com", Some(SLUG))).unwrap_err();
        let text = err.to_string();
        assert!(matches!(err, RemoteError::AppCredential(_)), "{err:?}");
        assert!(
            text.contains("github.com") && text.contains("ghe.example.com"),
            "{text}"
        );
    }

    // The same host, spelled differently, is accepted.
    GithubApp::new(
        Some("https://GHE.example.com/api/v3/".to_string()),
        creds("ghe.example.com", Some(SLUG)),
    )
    .unwrap();
    GithubApp::new(None, creds("github.com", Some(SLUG))).unwrap();
}

#[tokio::test]
async fn a_refused_jwt_is_an_app_credential_error() {
    let refusing = Router::new().route(
        "/app",
        get(|| async {
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"message": "A JSON web token could not be decoded"})),
            )
        }),
    );
    let base = spawn(refusing).await;
    let app = GithubApp::new(Some(base.clone()), creds(&host_of(&base), Some(SLUG))).unwrap();
    let err = app.app_info().await.unwrap_err();
    assert!(matches!(err, RemoteError::AppCredential(_)), "{err:?}");
}

// --- the App itself ----------------------------------------------------------

#[tokio::test]
async fn app_info_reads_the_app() {
    let (_mock, _base, app, _clock) = serve(Mock::default()).await;
    let info = app.app_info().await.unwrap();
    assert_eq!(info.id, 7);
    assert_eq!(info.slug, SLUG);
    assert_eq!(info.owner_login, "acme");
    assert_eq!(info.html_url, format!("https://github.com/apps/{SLUG}"));
}

#[tokio::test]
async fn bot_identity_is_slug_bot_with_the_noreply_email() {
    let (mock, base, app, _clock) = serve(mock_with(&[("acme/kb", 11)])).await;

    let bot = app.bot_identity("acme/kb").await.unwrap();
    // The loopback forge stands for an Enterprise Server at 127.0.0.1, so
    // the address takes that host; github.com is pinned in the unit tests.
    let host = host_of(&base);
    let host = host.split(':').next().unwrap();
    assert_eq!(
        bot,
        CommitIdentity {
            name: "crystalline-acme[bot]".to_string(),
            email: format!("41898282+crystalline-acme[bot]@users.noreply.{host}"),
        }
    );
    assert_eq!(app.bot_identity("acme/kb").await.unwrap(), bot);
    assert_eq!(mock.count("GET /users/"), 1, "the identity is cached");
    assert_eq!(
        mock.count("GET /users/crystalline-acme%5Bbot%5D"),
        1,
        "the brackets are percent-encoded"
    );
}

#[tokio::test]
async fn bot_identity_is_read_with_the_repositorys_installation_token() {
    let (mock, _base, app, _clock) = serve(mock_with(&[("acme/kb", 11)])).await;

    app.bot_identity("acme/kb").await.unwrap();
    let headers = mock.headers_of("GET /users/");
    assert_eq!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer ghs_11_1",
        "a private-mode Enterprise Server or the anonymous rate limit must not refuse it"
    );
}

// --- the checks `doctor` runs ------------------------------------------------

#[tokio::test]
async fn the_probe_reads_githubs_clock_even_when_the_jwt_is_refused() {
    let refusing = Router::new().route(
        "/app",
        get(|| async {
            (
                StatusCode::UNAUTHORIZED,
                [("Date", "Wed, 28 Oct 2026 12:02:00 GMT")],
                Json(serde_json::json!({"message": "'Issued at' claim must be before now"})),
            )
        }),
    );
    let base = spawn(refusing).await;
    let app = GithubApp::new(Some(base.clone()), creds(&host_of(&base), Some(SLUG))).unwrap();

    let probe = app.probe().await;
    assert!(
        matches!(probe.info, Err(RemoteError::AppCredential(_))),
        "{:?}",
        probe.info
    );
    assert_eq!(probe.server_date, Some(t0() + Duration::seconds(120)));
}

#[tokio::test]
async fn the_probe_answers_the_app_and_githubs_clock() {
    let answering = Router::new().route(
        "/app",
        get(|| async {
            (
                StatusCode::OK,
                [("Date", "Wed, 28 Oct 2026 12:00:00 GMT")],
                Json(serde_json::json!({"id": 7, "slug": SLUG, "owner": {"login": "acme"}})),
            )
        }),
    );
    let base = spawn(answering).await;
    let app = GithubApp::new(Some(base.clone()), creds(&host_of(&base), None)).unwrap();

    let probe = app.probe().await;
    assert_eq!(probe.info.unwrap().slug, SLUG);
    assert_eq!(probe.server_date, Some(t0()));
    assert_eq!(app.known_slug().as_deref(), Some(SLUG), "the slug is kept");
}

#[tokio::test]
async fn installations_name_the_accounts_the_app_is_installed_on() {
    let listing = Router::new().route(
        "/app/installations",
        get(|| async {
            Json(serde_json::json!([
                {"id": 1, "account": {"login": "acme"}},
                {"id": 2, "account": {"login": "octo-org"}},
            ]))
        }),
    );
    let base = spawn(listing).await;
    let app = GithubApp::new(Some(base.clone()), creds(&host_of(&base), Some(SLUG))).unwrap();

    assert_eq!(
        app.installations().await.unwrap(),
        vec!["acme".to_string(), "octo-org".to_string()]
    );
}

// --- repository roles, users by id and collaborators -------------------------

/// A stand-in GitHub for the lookups: the installation and token routes of
/// `router`, plus whatever `extra` adds.
async fn serve_with(
    mock: Mock,
    extra: Router<Arc<Mock>>,
) -> (Arc<Mock>, String, Arc<GithubApp>, TestClock) {
    let mock = Arc::new(mock);
    let routes = extra
        .route(
            "/repos/{owner}/{name}/installation",
            get(installation_handler),
        )
        .route("/app/installations/{id}/access_tokens", post(token_handler))
        .with_state(mock.clone());
    let base = spawn(routes).await;
    let clock = TestClock::new();
    let app = GithubApp::new(Some(base.clone()), creds(&host_of(&base), Some(SLUG))).unwrap();
    (mock, base, Arc::new(clock.install(app)), clock)
}

/// `GET /repos/{o}/{r}/collaborators/{login}/permission`: the login picks the
/// answer. The first field is the legacy `permission`, the second `role_name`.
async fn permission_handler(
    State(mock): State<Arc<Mock>>,
    Path((_owner, _name, login)): Path<(String, String, String)>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    mock.record("GET", &uri, &headers);
    let (permission, role_name) = match login.as_str() {
        "u-admin" => ("admin", "admin"),
        "u-maintain" => ("write", "maintain"),
        "u-write" => ("write", "write"),
        "u-triage" => ("read", "triage"),
        "u-read" => ("read", "read"),
        "docs" => ("write", "docs-lead"),
        "u-none" => ("none", "none"),
        "forbidden" => {
            return mock.answer(
                StatusCode::FORBIDDEN,
                serde_json::json!({"message": "Resource not accessible by integration"}),
            );
        }
        "throttled" => {
            let mut response = mock.answer(
                StatusCode::TOO_MANY_REQUESTS,
                serde_json::json!({"message": "slow down"}),
            );
            response
                .headers_mut()
                .insert("retry-after", "30".parse().unwrap());
            return response;
        }
        "limited" => {
            let mut response = mock.answer(
                StatusCode::FORBIDDEN,
                serde_json::json!({"message": "API rate limit exceeded"}),
            );
            response
                .headers_mut()
                .insert("x-ratelimit-remaining", "0".parse().unwrap());
            response
                .headers_mut()
                .insert("x-ratelimit-reset", "1793188800".parse().unwrap());
            return response;
        }
        _ => return mock.answer(StatusCode::NOT_FOUND, not_found()),
    };
    mock.answer(
        StatusCode::OK,
        serde_json::json!({"permission": permission, "role_name": role_name, "user": {"login": login}}),
    )
}

fn permission_routes() -> Router<Arc<Mock>> {
    Router::new().route(
        "/repos/{owner}/{name}/collaborators/{login}/permission",
        get(permission_handler),
    )
}

#[tokio::test]
async fn repo_role_maps_every_builtin_role() {
    let (mock, _base, app, _clock) =
        serve_with(mock_with(&[("acme/kb", 11)]), permission_routes()).await;

    for (login, role) in [
        ("u-admin", RepoRole::Admin),
        ("u-maintain", RepoRole::Maintain),
        ("u-write", RepoRole::Write),
        ("u-triage", RepoRole::Triage),
        ("u-read", RepoRole::Read),
    ] {
        assert_eq!(
            app.repo_role("acme/kb", login).await.unwrap(),
            role,
            "{login}"
        );
    }
    let headers = mock.headers_of("GET /repos/acme/kb/collaborators/");
    assert_eq!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer ghs_11_1"
    );
    assert_eq!(headers.get("x-github-api-version").unwrap(), "2022-11-28");
    assert_eq!(mock.minted(11), 1, "one installation token serves them all");
}

#[tokio::test]
async fn a_custom_role_maps_by_its_base_permission() {
    let (_mock, _base, app, _clock) =
        serve_with(mock_with(&[("acme/kb", 11)]), permission_routes()).await;
    assert_eq!(
        app.repo_role("acme/kb", "docs").await.unwrap(),
        RepoRole::Write
    );
}

#[tokio::test]
async fn permission_none_is_none_and_an_unknown_login_is_user_not_found() {
    let (_mock, _base, app, _clock) =
        serve_with(mock_with(&[("acme/kb", 11)]), permission_routes()).await;
    assert_eq!(
        app.repo_role("acme/kb", "u-none").await.unwrap(),
        RepoRole::None
    );
    match app.repo_role("acme/kb", "stranger").await.unwrap_err() {
        RemoteError::UserNotFound { repo, login } => {
            assert_eq!(repo, "acme/kb");
            assert_eq!(login, "stranger");
        }
        other => panic!("expected UserNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn a_forbidden_permission_lookup_is_an_error_not_none() {
    let (_mock, _base, app, _clock) =
        serve_with(mock_with(&[("acme/kb", 11)]), permission_routes()).await;
    let err = app.repo_role("acme/kb", "forbidden").await.unwrap_err();
    assert!(
        matches!(err, RemoteError::Api { status: 403, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_429_on_the_permission_lookup_is_rate_limited() {
    let (_mock, _base, app, _clock) =
        serve_with(mock_with(&[("acme/kb", 11)]), permission_routes()).await;
    let err = app.repo_role("acme/kb", "throttled").await.unwrap_err();
    assert!(matches!(err, RemoteError::RateLimited { .. }), "{err:?}");
}

#[tokio::test]
async fn rate_limit_on_the_permission_lookup_is_rate_limited() {
    let (_mock, _base, app, _clock) =
        serve_with(mock_with(&[("acme/kb", 11)]), permission_routes()).await;
    let err = app.repo_role("acme/kb", "limited").await.unwrap_err();
    match err {
        RemoteError::RateLimited { reset } => {
            assert_eq!(reset, Some(Utc.timestamp_opt(1793188800, 0).unwrap()));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn a_renewed_installation_token_is_tried_once_after_a_401() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded = seen.clone();
    let routes = Router::new().route(
        "/repos/{owner}/{name}/collaborators/{login}/permission",
        get(move |headers: HeaderMap| {
            let recorded = recorded.clone();
            async move {
                let auth = headers["authorization"].to_str().unwrap().to_string();
                recorded.lock().unwrap().push(auth.clone());
                if auth == "Bearer ghs_11_1" {
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({"message": "Bad credentials"})),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"permission": "read", "role_name": "read"})),
                    )
                }
            }
        }),
    );
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
    assert_eq!(
        app.repo_role("acme/kb", "anyone").await.unwrap(),
        RepoRole::Read
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["Bearer ghs_11_1".to_string(), "Bearer ghs_11_2".to_string()]
    );
}

fn user_by_id_routes() -> Router<Arc<Mock>> {
    Router::new().route(
        "/user/{id}",
        get(|State(mock): State<Arc<Mock>>, Path(id): Path<u64>, uri: Uri, headers: HeaderMap| async move {
            mock.record("GET", &uri, &headers);
            if id == 583231 {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({"login": "octocat-renamed", "id": id})),
                )
            } else {
                (StatusCode::NOT_FOUND, Json(not_found()))
            }
        }),
    )
}

fn installations_route(ids: &'static [u64]) -> Router<Arc<Mock>> {
    Router::new().route(
        "/app/installations",
        get(move || async move {
            Json(
                ids.iter()
                    .map(|id| serde_json::json!({"id": id, "account": {"login": "acme"}}))
                    .collect::<Vec<_>>(),
            )
        }),
    )
}

#[tokio::test]
async fn a_renamed_login_resolves_through_the_id() {
    // No repository was touched yet: the installation is found through the
    // App's own listing and its token is what reaches /user/{id}.
    let routes = user_by_id_routes().merge(installations_route(&[22, 11]));
    let (mock, _base, app, _clock) = serve_with(mock_with(&[]), routes).await;
    assert_eq!(app.login_for_id(583231).await.unwrap(), "octocat-renamed");
    assert!(app.login_for_id(1).await.is_err());
    let headers = mock.headers_of("GET /user/");
    assert_eq!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer ghs_11_1",
        "the lowest installation, never anonymous"
    );
    assert_eq!(mock.minted(11), 1);
}

#[tokio::test]
async fn an_installation_entry_without_an_id_is_skipped() {
    let routes = user_by_id_routes().merge(installations_route(&[0, 11]));
    let (mock, _base, app, _clock) = serve_with(mock_with(&[]), routes).await;
    assert_eq!(app.login_for_id(583231).await.unwrap(), "octocat-renamed");
    assert_eq!(mock.minted(0), 0);
    assert_eq!(mock.minted(11), 1);
}

#[tokio::test]
async fn login_for_id_without_an_installation_is_app_not_installed() {
    let routes = user_by_id_routes().merge(installations_route(&[]));
    let (mock, _base, app, _clock) = serve_with(mock_with(&[]), routes).await;
    let err = app.login_for_id(583231).await.unwrap_err();
    assert!(
        matches!(err, RemoteError::AppNotInstalled { .. }),
        "{err:?}"
    );
    assert_eq!(mock.count("GET /user/"), 0, "never asked anonymously");
}

#[tokio::test]
async fn login_for_id_picks_again_when_the_installation_is_gone() {
    // Installation 11 is listed but its token mint 404s; 22 works.
    let routes = user_by_id_routes()
        .merge(installations_route(&[11, 22]))
        .route(
            "/app/installations/11/access_tokens",
            post(|| async { (StatusCode::NOT_FOUND, Json(not_found())) }),
        );
    let (mock, _base, app, _clock) = serve_with(mock_with(&[]), routes).await;
    assert_eq!(app.login_for_id(583231).await.unwrap(), "octocat-renamed");
    assert_eq!(mock.minted(22), 1);
}

#[tokio::test]
async fn a_transferred_repository_follows_the_301() {
    let routes = Router::new()
        .route(
            "/repos/{owner}/{name}/collaborators/{login}/permission",
            get(|| async {
                (
                    StatusCode::MOVED_PERMANENTLY,
                    [(
                        "Location",
                        "/repositories/99/collaborators/u-admin/permission",
                    )],
                )
            }),
        )
        .route(
            "/repositories/{id}/collaborators/{login}/permission",
            get(
                |State(mock): State<Arc<Mock>>, uri: Uri, headers: HeaderMap| async move {
                    mock.record("GET", &uri, &headers);
                    Json(serde_json::json!({"permission": "admin", "role_name": "admin"}))
                },
            ),
        );
    let (mock, _base, app, _clock) = serve_with(mock_with(&[("acme/old", 11)]), routes).await;
    assert_eq!(
        app.repo_role("acme/old", "u-admin").await.unwrap(),
        RepoRole::Admin
    );
    let headers = mock.headers_of("GET /repositories/99/");
    assert_eq!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer ghs_11_1",
        "the bearer survives the hop"
    );
}

#[tokio::test]
async fn a_chain_of_redirects_is_an_api_error_not_offline() {
    let routes = Router::new().route(
        "/repos/{owner}/{name}/collaborators/{login}/permission",
        get(|| async {
            (
                StatusCode::MOVED_PERMANENTLY,
                [("Location", "/repos/acme/old/collaborators/x/permission")],
            )
        }),
    );
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/old", 11)]), routes).await;
    let err = app.repo_role("acme/old", "x").await.unwrap_err();
    assert!(matches!(err, RemoteError::Api { .. }), "{err:?}");
}

/// A collaborator listing of `total` users, 100 a page, with a `Link:
/// rel="next"` on every page but the last. `next_host` overrides the host the
/// link names.
fn listing_routes(
    total: u64,
    next_host: Option<&'static str>,
    queries: Arc<Mutex<Vec<HashMap<String, String>>>>,
) -> Router<Arc<Mock>> {
    Router::new().route(
        "/repos/{owner}/{name}/collaborators",
        get(
            move |Query(query): Query<HashMap<String, String>>, headers: HeaderMap| {
                let queries = queries.clone();
                async move {
                    queries.lock().unwrap().push(query.clone());
                    let page: u64 = query.get("page").map_or(1, |p| p.parse().unwrap());
                    let from = (page - 1) * 100 + 1;
                    let to = (page * 100).min(total);
                    let users: Vec<_> = (from..=to)
                        .map(|n| {
                            let (role, permissions) = match n {
                                1 => (
                                    "admin",
                                    serde_json::json!({"admin": true, "maintain": true, "push": true, "triage": true, "pull": true}),
                                ),
                                // A custom role: maintain is set, but the
                                // legacy base is write.
                                2 => (
                                    "docs-lead",
                                    serde_json::json!({"admin": false, "maintain": true, "push": true, "triage": true, "pull": true}),
                                ),
                                _ => (
                                    "read",
                                    serde_json::json!({"admin": false, "maintain": false, "push": false, "triage": false, "pull": true}),
                                ),
                            };
                            serde_json::json!({"id": n, "login": format!("user{n}"), "role_name": role, "permissions": permissions})
                        })
                        .collect();
                    let mut response = Json(users).into_response();
                    if to < total {
                        let host = next_host
                            .map(str::to_string)
                            .unwrap_or_else(|| headers["host"].to_str().unwrap().to_string());
                        let link = format!(
                            "<http://{host}/repos/acme/kb/collaborators?affiliation=all&per_page=100&page={}>; rel=\"next\"",
                            page + 1
                        );
                        response
                            .headers_mut()
                            .insert("link", link.parse().unwrap());
                    }
                    response
                }
            },
        ),
    )
}

#[tokio::test]
async fn collaborators_walk_every_page() {
    let queries = Arc::new(Mutex::new(Vec::new()));
    let routes = listing_routes(101, None, queries.clone());
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;

    let all = app.collaborators("acme/kb").await.unwrap();
    assert_eq!(all.len(), 101);
    assert_eq!(
        all[0],
        Collaborator {
            id: 1,
            login: "user1".to_string(),
            role: RepoRole::Admin
        }
    );
    assert_eq!(
        all[1].role,
        RepoRole::Write,
        "a custom role maps by its legacy base: maintain and triage are ignored"
    );
    assert_eq!(all[100].id, 101);
    assert_eq!(all[100].role, RepoRole::Read);

    let queries = queries.lock().unwrap();
    assert_eq!(queries.len(), 2, "no next link ends the walk");
    assert!(
        queries
            .iter()
            .all(|q| q["affiliation"] == "all" && q["per_page"] == "100")
    );
}

#[tokio::test]
async fn a_next_link_to_another_host_is_an_error() {
    let routes = listing_routes(101, Some("evil.example.com"), Arc::default());
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
    let err = app.collaborators("acme/kb").await.unwrap_err();
    assert!(matches!(err, RemoteError::Api { .. }), "{err:?}");
}

#[tokio::test]
async fn an_endless_listing_stops_at_the_page_cap() {
    // Every page links to the next one, forever.
    let queries = Arc::new(Mutex::new(Vec::new()));
    let routes = listing_routes(u64::MAX / 2, None, queries.clone());
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
    let err = app.collaborators("acme/kb").await.unwrap_err();
    assert!(matches!(err, RemoteError::Api { .. }), "{err:?}");
    assert_eq!(queries.lock().unwrap().len(), 1000);
}

#[tokio::test]
async fn repository_visibility_reads_the_private_field() {
    for (private, public) in [(true, false), (false, true)] {
        let routes = Router::new().route(
            "/repos/{owner}/{name}",
            get(move || async move { Json(serde_json::json!({"private": private})) }),
        );
        let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
        assert_eq!(app.repository_public("acme/kb").await.unwrap(), public);
    }
}

/// An internal repository (GitHub Enterprise) answers `private: true` and is
/// read by every member of the enterprise, whom the collaborator listing
/// does not name: it is told apart, so no caller takes it for private.
#[tokio::test]
async fn repository_visibility_tells_an_internal_repository_apart() {
    for (body, want) in [
        (
            serde_json::json!({"private": true, "visibility": "internal"}),
            RepoVisibility::Internal,
        ),
        (
            serde_json::json!({"private": true, "visibility": "private"}),
            RepoVisibility::Private,
        ),
        (
            serde_json::json!({"private": true}),
            RepoVisibility::Private,
        ),
        (
            serde_json::json!({"private": false, "visibility": "public"}),
            RepoVisibility::Public,
        ),
        (
            serde_json::json!({"private": false}),
            RepoVisibility::Public,
        ),
        // The visibility word wins over the older flag when both are there.
        (
            serde_json::json!({"private": false, "visibility": "internal"}),
            RepoVisibility::Internal,
        ),
        (
            serde_json::json!({"private": false, "visibility": "private"}),
            RepoVisibility::Private,
        ),
    ] {
        let routes = Router::new().route(
            "/repos/{owner}/{name}",
            get(move || {
                let body = body.clone();
                async move { Json(body) }
            }),
        );
        let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
        assert_eq!(app.repository_visibility("acme/kb").await.unwrap(), want);
        assert_eq!(
            app.repository_public("acme/kb").await.unwrap(),
            want == RepoVisibility::Public
        );
    }
}

/// A visibility word this client does not know is an error, never a guess.
#[tokio::test]
async fn an_unknown_visibility_word_is_an_error() {
    let routes = Router::new().route(
        "/repos/{owner}/{name}",
        get(|| async { Json(serde_json::json!({"private": true, "visibility": "secret"})) }),
    );
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
    assert!(app.repository_visibility("acme/kb").await.is_err());
    assert!(app.repository_public("acme/kb").await.is_err());
}

/// What GitHub says a repository is: its numeric id and its current
/// `owner/name`, read with the repository's installation token. A renamed
/// repository answers under its new name through GitHub's redirect.
#[tokio::test]
async fn repository_identity_reads_the_id_and_the_current_name() {
    let routes = Router::new()
        .route(
            "/repos/{owner}/{name}",
            get(|Path((owner, name)): Path<(String, String)>| async move {
                if name == "kb" {
                    return (
                        StatusCode::MOVED_PERMANENTLY,
                        [("location", format!("/repos/{owner}/handbook"))],
                    )
                        .into_response();
                }
                Json(serde_json::json!({"id": 55, "full_name": format!("{owner}/{name}")}))
                    .into_response()
            }),
        )
        .route(
            "/repositories/{id}",
            get(|Path(id): Path<u64>| async move {
                Json(serde_json::json!({"id": id, "full_name": "acme/handbook"}))
            }),
        );
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
    assert_eq!(
        app.repository_identity("acme/kb").await.unwrap(),
        (55, "acme/handbook".to_string())
    );
    assert_eq!(
        app.repository_by_id("acme/kb", 55).await.unwrap(),
        (55, "acme/handbook".to_string())
    );
}

/// The repositories the client has looked an installation up for, so an
/// uninstalled installation's domains can all be closed.
#[tokio::test]
async fn repositories_of_an_installation_are_the_ones_looked_up_for_it() {
    let (_mock, _base, app, _clock) = serve_with(
        mock_with(&[("acme/kb", 11), ("acme/vault", 11), ("other/x", 12)]),
        Router::new(),
    )
    .await;
    for repo in ["acme/kb", "acme/vault", "other/x"] {
        app.installation_for(repo).await.unwrap();
    }
    let mut repos = app.repositories_of_installation(11);
    repos.sort();
    assert_eq!(repos, vec!["acme/kb".to_string(), "acme/vault".to_string()]);
    assert!(app.repositories_of_installation(99).is_empty());
}

/// One `GET /repos/{o}/{r}` answers everything the governor checks: who may
/// read it, its id, its owner's id and its current name.
#[tokio::test]
async fn repository_facts_read_the_ids_the_name_and_the_visibility() {
    let routes = Router::new().route(
        "/repos/{owner}/{name}",
        get(|| async {
            Json(serde_json::json!({
                "id": 55, "full_name": "acme/handbook", "private": false,
                "visibility": "public", "owner": {"id": 7, "login": "acme"},
            }))
        }),
    );
    let (_mock, _base, app, _clock) = serve_with(mock_with(&[("acme/kb", 11)]), routes).await;
    let facts = app.repository_facts("acme/kb").await.unwrap();
    assert_eq!(facts.visibility, RepoVisibility::Public);
    assert_eq!(
        facts.ids,
        Some(crystalline_remote::state::RepositoryIds {
            repository: 55,
            owner: 7
        })
    );
    assert_eq!(facts.full_name.as_deref(), Some("acme/handbook"));
}
