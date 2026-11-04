//! Creating the instance's GitHub App from the browser, through GitHub's
//! manifest flow, driven end to end: an admin asks for the manifest form,
//! GitHub creates the App and sends the browser back with a code, and the
//! callback converts the code into the App's credentials and stores them.
//!
//! The callback is a GET that plants a GitHub App, so what is pinned here is
//! what keeps a forged one out: the state is bound to the admin session that
//! asked for the form, it works once, it lives an hour, and an App already
//! configured is never overwritten. No secret ever leaves the server.
//!
//! Nothing reaches the internet. `FakeGithub` serves the conversion, `GET
//! /app` the save validates with and `GET /app/installations`, on github.com
//! paths and under `/api/v3` for an Enterprise Server. The browser is a
//! `reqwest` client with redirects off and its cookies carried by hand.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use crystalline_core::config::{
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_remote::AppStore;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{Engine, EnvOverlay};

const PEM: &str = include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem");
const PASSWORD: &str = "correct horse battery staple";
const SLUG: &str = "crystalline-kb";
const CLIENT_SECRET: &str = "conv-client-secret-5d1e";
const WEBHOOK_SECRET: &str = "conv-webhook-secret-5d1e";
/// The one code the fake GitHub converts.
const CODE: &str = "c0de5d1e";

// --- the fake GitHub ----------------------------------------------------------

#[derive(Default)]
struct GithubState {
    /// How many conversions were asked for.
    conversions: AtomicUsize,
    /// Sent as `X-GitHub-Enterprise-Version` on a 404 when set.
    ghes_version: Mutex<Option<&'static str>>,
    /// While set, the conversion endpoint is missing (an old GHES).
    no_conversions: std::sync::atomic::AtomicBool,
}

struct FakeGithub {
    url: String,
    state: Arc<GithubState>,
}

impl FakeGithub {
    async fn start() -> FakeGithub {
        let state = Arc::new(GithubState::default());
        let mut router = axum::Router::new();
        for prefix in ["", "/api/v3"] {
            router = router
                .route(
                    &format!("{prefix}/app-manifests/{{code}}/conversions"),
                    post(gh_conversion),
                )
                .route(&format!("{prefix}/app"), get(gh_app))
                .route(
                    &format!("{prefix}/app/installations"),
                    get(gh_installations),
                );
        }
        let router = router.with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        FakeGithub {
            url: format!("http://{addr}"),
            state,
        }
    }

    fn conversions(&self) -> usize {
        self.state.conversions.load(Ordering::SeqCst)
    }
}

async fn gh_conversion(
    State(state): State<Arc<GithubState>>,
    Path(code): Path<String>,
) -> Response {
    state.conversions.fetch_add(1, Ordering::SeqCst);
    let version = *state.ghes_version.lock().unwrap();
    if state.no_conversions.load(Ordering::SeqCst) || code != CODE {
        let mut response = (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": "Not Found"})),
        )
            .into_response();
        if let Some(version) = version {
            response
                .headers_mut()
                .insert("X-GitHub-Enterprise-Version", version.parse().unwrap());
        }
        return response;
    }
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": 7,
            "slug": SLUG,
            "owner": {"login": "acme"},
            "client_id": "Iv23.created",
            "client_secret": CLIENT_SECRET,
            "webhook_secret": WEBHOOK_SECRET,
            "pem": PEM,
            "html_url": format!("https://github.com/apps/{SLUG}"),
        })),
    )
        .into_response()
}

async fn gh_app() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": 7,
        "slug": SLUG,
        "owner": {"login": "acme"},
        "html_url": format!("https://github.com/apps/{SLUG}"),
    }))
}

async fn gh_installations() -> Json<serde_json::Value> {
    Json(serde_json::json!([]))
}

// --- the instance -------------------------------------------------------------

type Cookies = Vec<(String, String)>;

struct Ctx {
    addr: SocketAddr,
    client: reqwest::Client,
    clock: Arc<Mutex<Instant>>,
    engine: Arc<Engine>,
    tmp: tempfile::TempDir,
}

struct Setup {
    public_url: bool,
    /// The API url under `/api/v3`, as for an Enterprise Server.
    ghes: bool,
    read_only: bool,
    /// Environment variables the overlay reads (an App from the environment).
    env: Vec<(String, String)>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            public_url: true,
            ghes: false,
            read_only: false,
            env: Vec::new(),
        }
    }
}

impl Ctx {
    async fn start(github: &FakeGithub) -> Ctx {
        Ctx::with(github, Setup::default()).await
    }

    async fn with(github: &FakeGithub, setup: Setup) -> Ctx {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("eng");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: Eng\npermalink: MANIFEST\ntags: []\nstatus: stable\n\
             recorded_at: 2026-01-01T00:00:00Z\n---\n\n# Eng\n",
        )
        .unwrap();
        let api_url = if setup.ghes {
            format!("{}/api/v3", github.url)
        } else {
            github.url.clone()
        };
        let mut config = GlobalConfig {
            github: Some(GitHubConfig {
                api_url: Some(api_url),
                ..GitHubConfig::default()
            }),
            service: Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                public_url: setup.public_url.then(|| format!("http://{addr}")),
                ..ServiceConfig::default()
            }),
            auth: Some(AuthConfig {
                oauth: Some(false),
                ..AuthConfig::default()
            }),
            ..GlobalConfig::default()
        };
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        let config_path = tmp.path().join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &config).unwrap();
        let overlay = EnvOverlay::from_vars(setup.env.clone()).unwrap();
        let clock = Arc::new(Mutex::new(Instant::now()));
        let read = clock.clone();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(
                Arc::new(tokio::sync::Mutex::new(store)),
                config,
                None,
                Some(config_path),
            )
            .with_state_dir(tmp.path().to_path_buf())
            .with_token_store_dir(tmp.path().join("tokens"))
            .with_env_overlay(overlay)
            .with_read_only(setup.read_only)
            .with_link_clock(Arc::new(move || *read.lock().unwrap())),
        );
        engine.sync(None).await.unwrap();
        let auth = Arc::new(
            AuthStore::open(&tmp.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        for (name, role) in [
            ("root", Role::Admin),
            ("rita", Role::Admin),
            ("ann", Role::Editor),
            ("vera", Role::Viewer),
        ] {
            auth.add_user(name, name, None, role, PASSWORD)
                .await
                .unwrap();
        }
        let router = http_router(
            engine.clone(),
            Arc::new(AtomicUsize::new(0)),
            &[],
            auth.clone(),
            None,
        )
        .unwrap();
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Ctx {
            addr,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            clock,
            engine,
            tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}/api/v1{path}", self.addr)
    }

    /// Moves the link clock forward by `by`.
    fn advance(&self, by: Duration) {
        *self.clock.lock().unwrap() += by;
    }

    async fn login(&self, name: &str) -> Cookies {
        let response = self
            .client
            .post(self.url("/auth/login"))
            .json(&serde_json::json!({ "name": name, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "the login of {name} should succeed");
        response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|raw| raw.split(';').next())
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .collect()
    }

    async fn get(&self, path: &str, cookies: &Cookies) -> reqwest::Response {
        let mut request = self.client.get(self.url(path));
        if !cookies.is_empty() {
            request = request.header(reqwest::header::COOKIE, cookie_header(cookies));
        }
        request.send().await.unwrap()
    }

    async fn csrf(&self, cookies: &Cookies) -> String {
        let body: serde_json::Value = self.get("/auth/me", cookies).await.json().await.unwrap();
        body["csrf"]
            .as_str()
            .expect("a session has a token")
            .to_string()
    }

    /// `POST /settings/github/app/manifest` with the session's CSRF token.
    async fn ask_manifest(&self, cookies: &Cookies, body: serde_json::Value) -> reqwest::Response {
        let csrf = self.csrf(cookies).await;
        self.client
            .post(self.url("/settings/github/app/manifest"))
            .header(reqwest::header::COOKIE, cookie_header(cookies))
            .header("x-csrf-token", csrf)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// Asks for the form and hands back the answer, which must be a 200.
    async fn manifest(&self, cookies: &Cookies) -> serde_json::Value {
        let response = self.ask_manifest(cookies, serde_json::json!({})).await;
        assert_eq!(response.status(), 200, "the manifest should be handed out");
        response.json().await.unwrap()
    }

    /// The callback as GitHub's redirect makes the browser call it.
    async fn created(&self, cookies: &Cookies, code: &str, state: &str) -> reqwest::Response {
        self.get(
            &format!("/github/app/created?code={code}&state={state}"),
            cookies,
        )
        .await
    }

    /// The App the file store holds.
    fn saved(&self) -> Option<crystalline_remote::AppCredentials> {
        AppStore::file_fallback(&self.tmp.path().join("tokens"))
            .load()
            .unwrap()
    }
}

fn cookie_header(cookies: &Cookies) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn state_of(answer: &serde_json::Value) -> String {
    answer["state"].as_str().expect("a state").to_string()
}

// --- the tests ----------------------------------------------------------------

#[tokio::test]
async fn the_manifest_form_is_bound_for_githubs_new_app_page() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;

    let answer = ctx.manifest(&root).await;
    let state = state_of(&answer);
    assert_eq!(state.len(), 64, "32 random bytes, hex");
    assert_eq!(
        answer["post_url"],
        format!("{}/settings/apps/new?state={state}", github.url)
    );
    let manifest = &answer["manifest"];
    let base = format!("http://{}", ctx.addr);
    assert_eq!(
        manifest["redirect_url"],
        format!("{base}/api/v1/github/app/created")
    );
    // The App's callback is exactly the one the link flow sends GitHub.
    assert_eq!(
        manifest["callback_urls"],
        serde_json::json!([format!("{base}/api/v1/github/link/callback")])
    );
    assert_eq!(
        manifest["name"],
        format!("Crystalline 127.0.0.1:{}", ctx.addr.port())
    );
    // A loopback address is not reachable from GitHub: no webhook.
    assert!(manifest.get("hook_attributes").is_none(), "{manifest}");
    assert_eq!(
        manifest["default_permissions"].as_object().unwrap().len(),
        4
    );
}

#[tokio::test]
async fn the_conversion_stores_credentials_and_returns_no_secret() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let state = state_of(&ctx.manifest(&root).await);

    let response = ctx.created(&root, CODE, &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/settings/github?app=created");
    assert_eq!(github.conversions(), 1);

    // Stored through the path `github app set` takes: the slug from GitHub,
    // the host from github.api_url, every secret, the webhook's too.
    let saved = ctx.saved().expect("the App is stored");
    assert_eq!(saved.app_id, 7);
    assert_eq!(saved.client_id, "Iv23.created");
    assert_eq!(saved.client_secret.as_deref(), Some(CLIENT_SECRET));
    assert_eq!(saved.webhook_secret.as_deref(), Some(WEBHOOK_SECRET));
    assert_eq!(saved.private_key, PEM);
    assert_eq!(saved.slug.as_deref(), Some(SLUG));
    assert_eq!(saved.host, github.url.trim_start_matches("http://"));
    // The slot took it: the webhook endpoint verifies with its secret now.
    assert_eq!(
        ctx.engine.github_webhook_secret().as_deref(),
        Some(WEBHOOK_SECRET)
    );
    // Creating the App is the intent to use GitHub.
    assert!(ctx.engine.github_enabled());

    let status = ctx.get("/settings/github", &root).await;
    assert_eq!(status.status(), 200);
    let text = status.text().await.unwrap();
    for secret in [CLIENT_SECRET, WEBHOOK_SECRET, "PRIVATE KEY"] {
        assert!(!text.contains(secret), "a secret left the server: {text}");
    }
    let status: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(status["app"]["app_id"], 7);
    assert_eq!(status["app"]["slug"], SLUG);
    assert_eq!(
        status["app"]["install_url"],
        format!("{}/apps/{SLUG}/installations/new", github.url)
    );
    assert_eq!(status["app"]["webhook_secret_set"], true);
}

#[tokio::test]
async fn a_created_callback_with_a_foreign_state_is_refused() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let rita = ctx.login("rita").await;
    let state = state_of(&ctx.manifest(&root).await);

    // Another admin's browser with root's state: refused, and the state is
    // spent by that try.
    let response = ctx.created(&rita, CODE, &state).await;
    assert_eq!(response.status(), 403);
    let page = response.text().await.unwrap();
    assert!(page.contains("not valid"), "{page}");
    assert!(!page.contains("root"), "the page names nobody: {page}");
    let response = ctx.created(&root, CODE, &state).await;
    assert_eq!(response.status(), 403, "spent by the foreign try");

    // Root's own second session is another session too.
    let root_again = ctx.login("root").await;
    let state = state_of(&ctx.manifest(&root).await);
    let response = ctx.created(&root_again, CODE, &state).await;
    assert_eq!(response.status(), 403);

    // A state nobody minted, and no state at all.
    let response = ctx.created(&root, CODE, &"ab".repeat(32)).await;
    assert_eq!(response.status(), 403);
    let response = ctx
        .get(&format!("/github/app/created?code={CODE}"), &root)
        .await;
    assert_eq!(response.status(), 403);

    assert_eq!(github.conversions(), 0, "nothing reached GitHub");
    assert!(ctx.saved().is_none(), "nothing was stored");
}

#[tokio::test]
async fn a_state_works_once() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let state = state_of(&ctx.manifest(&root).await);
    assert_eq!(ctx.created(&root, CODE, &state).await.status(), 303);
    let response = ctx.created(&root, CODE, &state).await;
    assert_eq!(response.status(), 403, "a replay is refused");
    assert_eq!(github.conversions(), 1);
}

#[tokio::test]
async fn a_state_lives_an_hour() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let late = state_of(&ctx.manifest(&root).await);
    let state = state_of(&ctx.manifest(&root).await);

    ctx.advance(Duration::from_secs(59 * 60));
    let response = ctx.created(&root, CODE, &state).await;
    assert_eq!(response.status(), 303, "59 minutes is inside the hour");

    // The App is stored now; drop it so only the clock decides the next one.
    ctx.engine.remove_github_app().await.unwrap();
    ctx.advance(Duration::from_secs(2 * 60));
    let response = ctx.created(&root, CODE, &late).await;
    assert_eq!(response.status(), 403, "61 minutes is past it");
    assert_eq!(github.conversions(), 1);
}

#[tokio::test]
async fn a_created_callback_without_a_session_or_by_a_non_admin_is_refused() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let ann = ctx.login("ann").await;
    let state = state_of(&ctx.manifest(&root).await);

    let response = ctx.created(&Vec::new(), CODE, &state).await;
    assert_eq!(response.status(), 401, "a page, not a raw guard answer");
    assert!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"))
    );
    let response = ctx.created(&ann, CODE, &state).await;
    assert_eq!(response.status(), 403);
    // Neither spent the state: it was never proved whose it was.
    assert_eq!(ctx.created(&root, CODE, &state).await.status(), 303);
}

#[tokio::test]
async fn an_app_already_configured_is_never_overwritten() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let first = state_of(&ctx.manifest(&root).await);
    let second = state_of(&ctx.manifest(&root).await);
    assert_eq!(ctx.created(&root, CODE, &first).await.status(), 303);
    let before = ctx.saved().unwrap();

    // No new form is handed out while an App is configured.
    let response = ctx.ask_manifest(&root, serde_json::json!({})).await;
    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("crystalline github app remove"), "{detail}");

    // And a state handed out before is refused without converting.
    let response = ctx.created(&root, CODE, &second).await;
    assert_eq!(response.status(), 409);
    let page = response.text().await.unwrap();
    assert!(page.contains("already"), "{page}");
    assert_eq!(github.conversions(), 1, "the second code was not converted");
    assert_eq!(ctx.saved().unwrap(), before, "the stored App is unchanged");
}

#[tokio::test]
async fn the_manifest_needs_an_admin_a_public_url_and_a_valid_org() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let response = ctx.ask_manifest(&ann, serde_json::json!({})).await;
    assert_eq!(response.status(), 403);

    let root = ctx.login("root").await;
    for org in ["acme/evil", "-acme", "acme?x=1", ""] {
        let response = ctx
            .ask_manifest(&root, serde_json::json!({ "org": org }))
            .await;
        assert_eq!(response.status(), 422, "{org}");
    }

    let ctx = Ctx::with(
        &github,
        Setup {
            public_url: false,
            ..Setup::default()
        },
    )
    .await;
    let root = ctx.login("root").await;
    let response = ctx.ask_manifest(&root, serde_json::json!({})).await;
    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("service.public_url"),
        "{body}"
    );
}

#[tokio::test]
async fn the_org_form_posts_to_the_org_settings_page() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let response = ctx
        .ask_manifest(&root, serde_json::json!({ "org": "acme-corp" }))
        .await;
    assert_eq!(response.status(), 200);
    let answer: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        answer["post_url"],
        format!(
            "{}/organizations/acme-corp/settings/apps/new?state={}",
            github.url,
            state_of(&answer)
        )
    );
}

#[tokio::test]
async fn ghes_manifest_posts_to_the_ghes_web_host() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(
        &github,
        Setup {
            ghes: true,
            ..Setup::default()
        },
    )
    .await;
    let root = ctx.login("root").await;
    let answer = ctx.manifest(&root).await;
    let state = state_of(&answer);
    // The web host, never the API host under /api/v3.
    assert_eq!(
        answer["post_url"],
        format!("{}/settings/apps/new?state={state}", github.url)
    );
    // The conversion goes to the API host.
    assert_eq!(ctx.created(&root, CODE, &state).await.status(), 303);
    assert_eq!(github.conversions(), 1);
    assert!(ctx.saved().is_some());
}

#[tokio::test]
async fn a_ghes_without_the_conversion_endpoint_names_its_version() {
    for (version, endpoint_missing) in [("2.14.0", true), ("3.9.0", false)] {
        let page = ghes_conversion_refused(version).await;
        assert!(page.contains(version), "{page}");
        assert_eq!(
            page.contains("does not offer"),
            endpoint_missing,
            "{version}: {page}"
        );
        // From 3.0 the endpoint is there: the code was used or is too old.
        assert_eq!(page.contains("an hour old"), !endpoint_missing, "{page}");
        // GitHub created the App either way; the page says what to do.
        assert!(
            page.contains("delete it in your GitHub developer settings"),
            "{page}"
        );
    }
}

/// The page a GHES of `version` that answers the conversion 404 leads to.
async fn ghes_conversion_refused(version: &'static str) -> String {
    let github = FakeGithub::start().await;
    *github.state.ghes_version.lock().unwrap() = Some(version);
    github.state.no_conversions.store(true, Ordering::SeqCst);
    let ctx = Ctx::with(
        &github,
        Setup {
            ghes: true,
            ..Setup::default()
        },
    )
    .await;
    let root = ctx.login("root").await;
    let state = state_of(&ctx.manifest(&root).await);
    let response = ctx.created(&root, "secretcode", &state).await;
    assert_eq!(response.status(), 502);
    let page = response.text().await.unwrap();
    assert!(!page.contains("secretcode"), "the code is never echoed");
    assert!(ctx.saved().is_none());
    page
}

#[tokio::test]
async fn a_callback_without_a_code_says_to_delete_the_app() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let state = state_of(&ctx.manifest(&root).await);
    let response = ctx
        .get(&format!("/github/app/created?state={state}"), &root)
        .await;
    assert_eq!(response.status(), 502);
    let page = response.text().await.unwrap();
    assert!(
        page.contains("delete it in your GitHub developer settings"),
        "{page}"
    );
}

#[tokio::test]
async fn an_instance_without_an_app_answers_its_status_as_before() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let status: serde_json::Value = ctx
        .get("/settings/github", &root)
        .await
        .json()
        .await
        .unwrap();
    assert!(
        status.get("app").is_none(),
        "no App, no key: the answer is what it was before: {status}"
    );
}

#[tokio::test]
async fn read_only_hands_out_no_form() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(
        &github,
        Setup {
            read_only: true,
            ..Setup::default()
        },
    )
    .await;
    let root = ctx.login("root").await;
    let response = ctx.ask_manifest(&root, serde_json::json!({})).await;
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn the_setup_landing_goes_to_the_settings_page() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let response = ctx
        .get(
            "/github/app/setup?installation_id=5&setup_action=install",
            &Vec::new(),
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/settings/github?app=installed");
}

/// The environment of an App that comes from the variables.
fn env_app(client_secret: Option<&str>) -> Vec<(String, String)> {
    let mut vars = vec![
        ("CRYSTALLINE_GITHUB_APP_ID".to_string(), "7".to_string()),
        (
            "CRYSTALLINE_GITHUB_APP_CLIENT_ID".to_string(),
            "Iv23.env".to_string(),
        ),
        (
            "CRYSTALLINE_GITHUB_APP_PRIVATE_KEY".to_string(),
            PEM.to_string(),
        ),
    ];
    if let Some(secret) = client_secret {
        vars.push((
            "CRYSTALLINE_GITHUB_APP_CLIENT_SECRET".to_string(),
            secret.to_string(),
        ));
    }
    vars
}

/// The status JSON once the App has settled (a saved or environment App
/// loads in the background first and reads as loading until then).
async fn settled_app(ctx: &Ctx, cookies: &Cookies) -> (String, serde_json::Value) {
    let started = Instant::now();
    loop {
        let text = ctx
            .get("/settings/github", cookies)
            .await
            .text()
            .await
            .unwrap();
        let status: serde_json::Value = serde_json::from_str(&text).unwrap();
        if status["app"]["loading"] == false || started.elapsed() > Duration::from_secs(20) {
            return (text, status);
        }
        ctx.engine.preload_github_app();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_view_says_whether_an_app_from_the_environment_has_a_client_secret() {
    let github = FakeGithub::start().await;
    for (secret, set) in [(Some("env-client-secret-value"), true), (None, false)] {
        let ctx = Ctx::with(
            &github,
            Setup {
                env: env_app(secret),
                ..Setup::default()
            },
        )
        .await;
        let root = ctx.login("root").await;
        let (text, status) = settled_app(&ctx, &root).await;
        assert_eq!(status["app"]["source"], "environment", "{text}");
        assert_eq!(status["app"]["client_secret_set"], set, "{text}");
        assert_eq!(status["app"]["client_id"], "Iv23.env", "{text}");
        assert!(!text.contains("env-client-secret-value"), "{text}");
        assert!(!text.contains("PRIVATE KEY"), "{text}");
    }
}

#[tokio::test]
async fn the_view_says_whether_a_saved_app_has_a_client_secret() {
    let github = FakeGithub::start().await;
    for (secret, set) in [(Some("saved-client-secret-value"), true), (None, false)] {
        let ctx = Ctx::start(&github).await;
        AppStore::file_fallback(&ctx.tmp.path().join("tokens"))
            .save(&crystalline_remote::AppCredentials {
                app_id: 9,
                client_id: "Iv23.saved".to_string(),
                private_key: PEM.to_string(),
                client_secret: secret.map(str::to_string),
                webhook_secret: None,
                slug: Some(SLUG.to_string()),
                host: github.url.trim_start_matches("http://").to_string(),
            })
            .unwrap();
        ctx.engine.reset_github_app();
        let root = ctx.login("root").await;
        let (text, status) = settled_app(&ctx, &root).await;
        assert_eq!(status["app"]["source"], "file", "{text}");
        assert_eq!(status["app"]["client_secret_set"], set, "{text}");
        assert_eq!(status["app"]["client_id"], "Iv23.saved", "{text}");
        assert!(!text.contains("saved-client-secret-value"), "{text}");
    }
}

#[tokio::test]
async fn the_status_with_the_client_secret_state_is_for_admins_only() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(
        &github,
        Setup {
            env: env_app(Some("env-client-secret-value")),
            ..Setup::default()
        },
    )
    .await;
    for name in ["ann", "vera"] {
        let cookies = ctx.login(name).await;
        let response = ctx.get("/settings/github", &cookies).await;
        assert_eq!(response.status(), 403, "{name}");
        let text = response.text().await.unwrap();
        assert!(!text.contains("client_secret_set"), "{name}: {text}");
    }
}
