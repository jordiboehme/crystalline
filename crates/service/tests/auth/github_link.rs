//! The GitHub App's web flow, driven end to end: a signed-in person starts a
//! link from the browser, GitHub sends the browser back to the callback with a
//! code, and the link lands on the account of the session that started it -
//! never on another one.
//!
//! Nothing here reaches the internet. `FakeGithub` is a small axum server on
//! an ephemeral loopback port serving the four GitHub endpoints the flow
//! touches: the token endpoint the code is exchanged at, `GET /user` the
//! engine proves the identity with, and the revoke an unlink sends. A
//! loopback API url is its own OAuth base, so one server covers both.
//!
//! The browser is a `reqwest` client with redirects switched off and its
//! cookies carried by hand, as in `auth::oidc`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use crystalline_core::config::{
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, OidcConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_remote::{AppLookup, AppStore};
use crystalline_service::daemon::http_router;
use crystalline_service::engine::ConfigureAction;
use crystalline_service::mcp::McpServer;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{Engine, EnvOverlay};
use rmcp::model::CallToolRequestParams;

const CLIENT_ID: &str = "Iv23.weblink";
const CLIENT_SECRET: &str = "shhh-the-app-client-secret";
const PEM: &str = include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem");
const PASSWORD: &str = "correct horse battery staple";

/// The sentence the store refuses a GitHub account linked elsewhere with.
const ID_TAKEN: &str = "this GitHub account is already linked to another Crystalline account; an admin can unlink it there";

/// The sentence an unlink answers with when GitHub could not revoke the
/// token.
const NOT_REVOKED: &str = "unlinked here; GitHub could not be reached to revoke the token, revoke it in your GitHub settings";

// --- the fake GitHub ----------------------------------------------------------

#[derive(Default)]
struct GithubState {
    /// Code -> the access token the exchange hands out for it.
    codes: Mutex<HashMap<String, String>>,
    /// Access token -> the GitHub user (id, login) it belongs to.
    users: Mutex<HashMap<String, (u64, String)>>,
    /// Every body the token endpoint was sent.
    exchanges: Mutex<Vec<serde_json::Value>>,
    /// The access tokens revoked through the App.
    revoked: Mutex<Vec<String>>,
    /// While set, the revoke answers 500.
    revoke_broken: std::sync::atomic::AtomicBool,
    /// How many times the token endpoint was asked.
    token_hits: AtomicUsize,
}

struct FakeGithub {
    url: String,
    state: Arc<GithubState>,
}

impl FakeGithub {
    async fn start() -> FakeGithub {
        let state = Arc::new(GithubState::default());
        let router = axum::Router::new()
            .route("/login/oauth/access_token", post(gh_token))
            .route("/user", get(gh_user))
            .route("/applications/{client_id}/token", delete(gh_revoke))
            .with_state(state.clone());
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

    /// Makes `code` exchange for `token`, which belongs to GitHub user
    /// `id`/`login`.
    fn grant(&self, code: &str, token: &str, id: u64, login: &str) {
        self.state
            .codes
            .lock()
            .unwrap()
            .insert(code.to_string(), token.to_string());
        self.state
            .users
            .lock()
            .unwrap()
            .insert(token.to_string(), (id, login.to_string()));
    }

    fn token_hits(&self) -> usize {
        self.state.token_hits.load(Ordering::SeqCst)
    }

    fn revoked(&self) -> Vec<String> {
        self.state.revoked.lock().unwrap().clone()
    }
}

async fn gh_token(
    State(state): State<Arc<GithubState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.token_hits.fetch_add(1, Ordering::SeqCst);
    state.exchanges.lock().unwrap().push(body.clone());
    let wanted = body["client_id"] == CLIENT_ID && body["client_secret"] == CLIENT_SECRET;
    let code = body["code"].as_str().unwrap_or_default();
    match state.codes.lock().unwrap().remove(code) {
        Some(token) if wanted => Json(serde_json::json!({
            "access_token": token,
            "token_type": "bearer",
            "expires_in": 28800,
            "refresh_token": format!("ghr_{token}"),
            "refresh_token_expires_in": 15_897_600,
        })),
        _ => Json(serde_json::json!({
            "error": "bad_verification_code",
            "error_description": "The code passed is incorrect or expired.",
        })),
    }
}

async fn gh_user(State(state): State<Arc<GithubState>>, headers: HeaderMap) -> Response {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    match state.users.lock().unwrap().get(&token) {
        Some((id, login)) => Json(serde_json::json!({ "id": id, "login": login })).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn gh_revoke(
    State(state): State<Arc<GithubState>>,
    Path(client_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> StatusCode {
    if state.revoke_broken.load(Ordering::SeqCst) || client_id != CLIENT_ID {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    if let Some(token) = body["access_token"].as_str() {
        state.revoked.lock().unwrap().push(token.to_string());
    }
    StatusCode::NO_CONTENT
}

// --- the instance -------------------------------------------------------------

type Cookies = Vec<(String, String)>;

struct Ctx {
    addr: SocketAddr,
    client: reqwest::Client,
    auth: Arc<AuthStore>,
    clock: Arc<Mutex<Instant>>,
    engine: Arc<Engine>,
    _tmp: tempfile::TempDir,
}

struct Setup {
    public_url: bool,
    client_secret: bool,
    app: bool,
    /// `auth.mcp` on, so an agent session over HTTP is an account.
    mcp_auth: bool,
    /// A single sign-on provider configured (never reached: the tests only
    /// read where the browser is sent).
    sso: bool,
    /// The App's private key is not a key, so the App is configured and
    /// cannot be used.
    bad_key: bool,
    /// No App in the environment, and the keychain that holds the saved App
    /// cannot be read.
    keychain_locked: bool,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            public_url: true,
            client_secret: true,
            app: true,
            mcp_auth: false,
            sso: false,
            bad_key: false,
            keychain_locked: false,
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
        let mut config = GlobalConfig {
            github: Some(GitHubConfig {
                enabled: Some(true),
                api_url: Some(github.url.clone()),
                share_identity: Some("personal".to_string()),
                ..GitHubConfig::default()
            }),
            service: Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                public_url: setup.public_url.then(|| format!("http://{addr}")),
                ..ServiceConfig::default()
            }),
            auth: Some(AuthConfig {
                mcp: Some(setup.mcp_auth),
                oauth: Some(false),
                oidc: setup.sso.then(|| OidcConfig {
                    issuer: Some("https://idp.example".to_string()),
                    client_id: Some("crystalline".to_string()),
                    client_secret: Some("idp-secret".to_string()),
                    ..OidcConfig::default()
                }),
                ..AuthConfig::default()
            }),
            ..GlobalConfig::default()
        };
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        let config_path = tmp.path().join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &config).unwrap();
        let mut env = vec![
            ("CRYSTALLINE_GITHUB_APP_ID", "7"),
            ("CRYSTALLINE_GITHUB_APP_CLIENT_ID", CLIENT_ID),
            (
                "CRYSTALLINE_GITHUB_APP_PRIVATE_KEY",
                if setup.bad_key { "not a key" } else { PEM },
            ),
        ];
        if !setup.app || setup.keychain_locked {
            env.clear();
        }
        if setup.app && !setup.keychain_locked && setup.client_secret {
            env.push(("CRYSTALLINE_GITHUB_APP_CLIENT_SECRET", CLIENT_SECRET));
        }
        let overlay = EnvOverlay::from_vars(
            env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let clock = Arc::new(Mutex::new(Instant::now()));
        let read = clock.clone();
        let store = TursoStore::open_in_memory().await.unwrap();
        let mut engine = Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            config,
            None,
            Some(config_path),
        )
        .with_state_dir(tmp.path().to_path_buf())
        .with_token_store_dir(tmp.path().join("tokens"))
        .with_env_overlay(overlay)
        .with_link_clock(Arc::new(move || *read.lock().unwrap()));
        if setup.keychain_locked {
            engine = engine.with_app_store_lookup(locked_keychain(tmp.path().join("tokens")));
        }
        let engine = Arc::new(engine);
        engine.sync(None).await.unwrap();
        let auth = Arc::new(
            AuthStore::open(&tmp.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        for (name, role) in [
            ("ann", Role::Editor),
            ("bob", Role::Editor),
            ("vic", Role::Viewer),
            ("root", Role::Admin),
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
            auth,
            clock,
            engine,
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}/api/v1{path}", self.addr)
    }

    /// Moves the link clock forward by `by`.
    fn advance(&self, by: Duration) {
        let mut now = self.clock.lock().unwrap();
        *now += by;
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

    /// `POST /me/github-link/start` with the session's CSRF token.
    async fn start_link(&self, cookies: &Cookies) -> reqwest::Response {
        let csrf = self.csrf(cookies).await;
        self.client
            .post(self.url("/me/github-link/start"))
            .header(reqwest::header::COOKIE, cookie_header(cookies))
            .header("x-csrf-token", csrf)
            .send()
            .await
            .unwrap()
    }

    /// Starts a link and hands back the state the authorize url carries.
    async fn started_state(&self, cookies: &Cookies) -> String {
        let response = self.start_link(cookies).await;
        assert_eq!(response.status(), 200, "the start should be accepted");
        let body: serde_json::Value = response.json().await.unwrap();
        let authorize = reqwest::Url::parse(body["authorize_url"].as_str().unwrap()).unwrap();
        authorize
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .expect("the authorize url carries a state")
    }

    /// The callback as GitHub's redirect makes the browser call it.
    async fn callback(&self, cookies: &Cookies, code: &str, state: &str) -> reqwest::Response {
        self.get(
            &format!("/github/link/callback?code={code}&state={state}"),
            cookies,
        )
        .await
    }

    async fn delete(&self, path: &str, cookies: &Cookies) -> reqwest::Response {
        let csrf = self.csrf(cookies).await;
        self.client
            .delete(self.url(path))
            .header(reqwest::header::COOKIE, cookie_header(cookies))
            .header("x-csrf-token", csrf)
            .send()
            .await
            .unwrap()
    }

    /// The link `account` has in the accounts store.
    async fn link_of(&self, account: &str) -> Option<crystalline_service::rest::GithubLink> {
        self.auth.github_link_of(account).await.unwrap()
    }

    /// `GET /me/github-link` for the session `cookies` holds.
    async fn my_link(&self, cookies: &Cookies) -> serde_json::Value {
        let response = self.get("/me/github-link", cookies).await;
        assert_eq!(response.status(), 200);
        response.json().await.unwrap()
    }
}

/// A lookup that answers like a keychain that cannot be read, with the
/// marker saying an App is saved in it (an empty marker counts for every
/// host).
fn locked_keychain(file_dir: std::path::PathBuf) -> crystalline_service::engine::AppLookupFn {
    Arc::new(move || {
        Ok(AppLookup {
            store: AppStore::file_fallback(&file_dir),
            credentials: None,
            keychain_failed: Some("the keychain is locked".to_string()),
            marker: Some(String::new()),
        })
    })
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
        .expect("a redirect carries a location")
        .to_str()
        .unwrap()
        .to_string()
}

/// Links `account` (whose session `cookies` holds) to GitHub user
/// `id`/`login` through the whole web flow.
async fn link(
    ctx: &Ctx,
    github: &FakeGithub,
    cookies: &Cookies,
    token: &str,
    id: u64,
    login: &str,
) {
    let code = format!("code-{token}");
    github.grant(&code, token, id, login);
    let state = ctx.started_state(cookies).await;
    let response = ctx.callback(cookies, &code, &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github=linked");
}

// --- the tests ------------------------------------------------------------------

/// The whole flow: the start hands back GitHub's authorize url with the App's
/// client id, the callback address under `service.public_url` and a state;
/// the callback exchanges the code with that same redirect uri and links the
/// account of the session that started it.
#[tokio::test]
async fn the_callback_links_the_session_account() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;

    let start = ctx.start_link(&ann).await;
    assert_eq!(start.status(), 200);
    let body: serde_json::Value = start.json().await.unwrap();
    let authorize_url = body["authorize_url"].as_str().unwrap().to_string();
    assert!(
        authorize_url.starts_with(&format!("{}/login/oauth/authorize?", github.url)),
        "{authorize_url}"
    );
    let authorize = reqwest::Url::parse(&authorize_url).unwrap();
    let query: HashMap<String, String> = authorize.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], CLIENT_ID);
    let redirect_uri = format!("http://{}/api/v1/github/link/callback", ctx.addr);
    assert_eq!(query["redirect_uri"], redirect_uri);
    let state = query["state"].clone();
    assert!(state.len() >= 32, "the state is long and random: {state}");

    github.grant("code-1", "ghu_ann", 501, "ann-gh");
    let response = ctx.callback(&ann, "code-1", &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github=linked");
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let exchanges = github.state.exchanges.lock().unwrap().clone();
    assert_eq!(exchanges.len(), 1);
    assert_eq!(
        exchanges[0]["redirect_uri"], redirect_uri,
        "the exchange names the redirect uri the authorize url carried"
    );

    let link = ctx.link_of("ann").await.expect("ann is linked");
    assert_eq!(link.github_id, 501);
    assert_eq!(link.login, "ann-gh");
    let mine = ctx.my_link(&ann).await;
    assert_eq!(mine["linked"], true, "{mine}");
    assert_eq!(mine["login"], "ann-gh");
    assert_eq!(mine["github_id"], 501);
    assert_eq!(mine["token_live"], true);
    assert!(
        !mine.to_string().contains("ghu_") && !mine.to_string().contains("ghr_"),
        "the view carries no token: {mine}"
    );
}

/// A state is bound to the session that started it: a callback from another
/// account's session, or from another session of the same account, is
/// refused before anything reaches GitHub, and no link and no token appear.
#[tokio::test]
async fn a_state_from_another_session_is_refused() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let bob = ctx.login("bob").await;
    github.grant("code-1", "ghu_x", 501, "x-gh");

    let state = ctx.started_state(&ann).await;
    let response = ctx.callback(&bob, "code-1", &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=state");
    assert_eq!(github.token_hits(), 0, "nothing was exchanged");
    assert!(ctx.link_of("ann").await.is_none());
    assert!(ctx.link_of("bob").await.is_none());
    assert_eq!(ctx.my_link(&bob).await["linked"], false);

    // Another session of the same account is another session.
    let ann_again = ctx.login("ann").await;
    let state = ctx.started_state(&ann).await;
    let response = ctx.callback(&ann_again, "code-1", &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=state");
    assert_eq!(github.token_hits(), 0);
    assert!(ctx.link_of("ann").await.is_none());
}

/// The callback needs a session. GitHub's redirect is a browser landing, so
/// without one it sends the browser to the profile with `session` (the app
/// then asks for a sign-in and returns there) instead of a raw 401, and
/// nothing is exchanged or linked. The state is left alone: nothing proved
/// who sent it.
#[tokio::test]
async fn the_callback_needs_a_session() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    github.grant("code-1", "ghu_ann", 501, "ann-gh");
    let state = ctx.started_state(&ann).await;
    let response = ctx.callback(&Vec::new(), "code-1", &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=session");
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(github.token_hits(), 0);
    assert!(ctx.link_of("ann").await.is_none());

    // The usual way to lose it: a cookie the server has since forgotten.
    let state = ctx.started_state(&ann).await;
    let csrf = ctx.csrf(&ann).await;
    let logout = ctx
        .client
        .post(ctx.url("/auth/logout"))
        .header(reqwest::header::COOKIE, cookie_header(&ann))
        .header("x-csrf-token", csrf)
        .send()
        .await
        .unwrap();
    assert!(logout.status().is_success(), "{}", logout.status());
    let response = ctx.callback(&ann, "code-1", &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=session");
    assert_eq!(github.token_hits(), 0);
    assert!(ctx.link_of("ann").await.is_none());
}

/// A callback without a state, or naming one nobody started, lands on the
/// profile with `state` and reaches nothing.
#[tokio::test]
async fn an_unknown_or_missing_state_lands_on_the_profile() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    github.grant("code-1", "ghu_ann", 501, "ann-gh");
    let response = ctx.get("/github/link/callback?code=code-1", &ann).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=state");
    let response = ctx.callback(&ann, "code-1", "deadbeef").await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=state");
    assert_eq!(github.token_hits(), 0);
    assert!(ctx.link_of("ann").await.is_none());
}

/// A state links once; presenting it again is refused.
#[tokio::test]
async fn a_state_is_single_use() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    github.grant("code-1", "ghu_ann", 501, "ann-gh");
    let state = ctx.started_state(&ann).await;
    assert_eq!(ctx.callback(&ann, "code-1", &state).await.status(), 303);
    github.grant("code-2", "ghu_ann2", 501, "ann-gh");
    let replay = ctx.callback(&ann, "code-2", &state).await;
    assert_eq!(replay.status(), 303);
    assert_eq!(location(&replay), "/profile?github_error=state");
    assert_eq!(github.token_hits(), 1, "the replay reached nothing");
}

/// A state lives ten minutes.
#[tokio::test]
async fn a_state_expires_after_ten_minutes() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    github.grant("code-1", "ghu_ann", 501, "ann-gh");

    let state = ctx.started_state(&ann).await;
    ctx.advance(Duration::from_secs(10 * 60 + 1));
    let late = ctx.callback(&ann, "code-1", &state).await;
    assert_eq!(late.status(), 303);
    assert_eq!(location(&late), "/profile?github_error=state");
    assert_eq!(github.token_hits(), 0);
    assert!(ctx.link_of("ann").await.is_none());

    let state = ctx.started_state(&ann).await;
    ctx.advance(Duration::from_secs(9 * 60 + 59));
    let in_time = ctx.callback(&ann, "code-1", &state).await;
    assert_eq!(in_time.status(), 303);
    assert!(ctx.link_of("ann").await.is_some());
}

/// A GitHub account linked to one account cannot be linked to another: the
/// callback sends the browser to the profile with `id_taken`, and nothing is
/// linked or stored for the second account.
#[tokio::test]
async fn a_github_id_linked_elsewhere_is_refused_with_the_sentence() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let bob = ctx.login("bob").await;
    link(&ctx, &github, &ann, "ghu_ann", 501, "ann-gh").await;

    github.grant("code-bob", "ghu_bob", 501, "ann-gh");
    let state = ctx.started_state(&bob).await;
    let response = ctx.callback(&bob, "code-bob", &state).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=id_taken");
    assert!(ctx.link_of("bob").await.is_none());
    assert_eq!(ctx.link_of("ann").await.unwrap().github_id, 501);
    let bobs = ctx.my_link(&bob).await;
    assert_eq!(bobs["linked"], false, "{bobs}");
    // The refusal sentence is the store's own, the one the JSON surface
    // answers 409 with (pinned in rest's error tests) and the profile copy
    // for `id_taken` mirrors.
    assert_eq!(ID_TAKEN, crystalline_service::rest::GITHUB_ID_TAKEN);
}

/// GitHub's own refusal (the person pressed Cancel) lands on the profile
/// with `denied` and spends the state.
#[tokio::test]
async fn a_cancelled_authorization_lands_on_the_profile() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let state = ctx.started_state(&ann).await;
    let response = ctx
        .get(
            &format!("/github/link/callback?error=access_denied&state={state}"),
            &ann,
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), "/profile?github_error=denied");
    assert_eq!(github.token_hits(), 0);
}

/// A code GitHub refuses lands on the profile with `failed`, and neither the
/// code nor the state reaches a log line or the answer.
#[tokio::test]
async fn the_code_never_reaches_a_log_or_an_answer() {
    let (logs, _guard) = crate::support::capture_logs();
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let state = ctx.started_state(&ann).await;
    let code = "the-secret-code-0451";
    let response = ctx.callback(&ann, code, &state).await;
    assert_eq!(response.status(), 303);
    let landing = location(&response);
    assert_eq!(landing, "/profile?github_error=failed");
    assert_eq!(github.token_hits(), 1);
    let lines = logs.lines();
    assert!(
        lines.iter().any(|l| l.starts_with("WARN")),
        "a failed exchange warns: {lines:?}"
    );
    for needle in [code, state.as_str(), CLIENT_SECRET] {
        assert!(
            !lines.iter().any(|line| line.contains(needle)),
            "{needle} reached the log: {lines:?}"
        );
        assert!(!landing.contains(needle));
    }
}

/// Starting a link is a state-changing request: it needs the session's
/// CSRF token.
#[tokio::test]
async fn starting_a_link_needs_the_csrf_token() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let response = ctx
        .client
        .post(ctx.url("/me/github-link/start"))
        .header(reqwest::header::COOKIE, cookie_header(&ann))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    // The same request with the token is served: the token is the only
    // difference.
    let response = ctx.start_link(&ann).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["authorize_url"].is_string(), "{body}");
}

/// A viewer links too: the link is who the person is at GitHub, not a right
/// to share.
#[tokio::test]
async fn a_viewer_links_their_own_account() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let vic = ctx.login("vic").await;
    link(&ctx, &github, &vic, "ghu_vic", 777, "vic-gh").await;
    assert_eq!(ctx.link_of("vic").await.unwrap().github_id, 777);
}

/// The redirect uri is built from `service.public_url`; without it the start
/// is refused with the setting's name.
#[tokio::test]
async fn start_without_public_url_is_409() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(
        &github,
        Setup {
            public_url: false,
            ..Setup::default()
        },
    )
    .await;
    let ann = ctx.login("ann").await;
    let response = ctx.start_link(&ann).await;
    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("Ask an admin"), "{body}");
    assert!(!detail.contains("service.public_url"), "{body}");
    assert_eq!(body["reason"], "no_public_url", "{body}");
}

/// The web flow cannot exchange a code without the App's client secret, so
/// the start says how to add one.
#[tokio::test]
async fn start_without_a_client_secret_is_409() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(
        &github,
        Setup {
            client_secret: false,
            ..Setup::default()
        },
    )
    .await;
    let ann = ctx.login("ann").await;
    let response = ctx.start_link(&ann).await;
    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("Ask an admin"), "{body}");
    assert!(!detail.contains("crystalline "), "{body}");
    assert_eq!(body["reason"], "no_client_secret", "{body}");

    // The command is the admin's.
    let admin = ctx.login("root").await;
    let response = ctx.start_link(&admin).await;
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("crystalline github app set --app-id <id> --client-id <id> --private-key-file <file> --client-secret-file <file>"),
        "{body}"
    );
    assert_eq!(body["reason"], "no_client_secret", "{body}");
}

/// The start of a link refused because of the App's setup, as a viewer
/// sees it: the status and the `reason` the profile turns into plain words.
async fn start_refusal(setup: Setup) -> (u16, serde_json::Value) {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(&github, setup).await;
    let vic = ctx.login("vic").await;
    let response = ctx.start_link(&vic).await;
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

/// Without a GitHub App there is nothing to link through.
#[tokio::test]
async fn start_without_an_app_says_no_app() {
    let (status, body) = start_refusal(Setup {
        app: false,
        ..Setup::default()
    })
    .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["reason"], "no_app", "{body}");
    assert!(
        !body["detail"].as_str().unwrap().contains("crystalline "),
        "a viewer reads plain words: {body}"
    );
}

/// An App that is configured and cannot be used (its key is not a key).
#[tokio::test]
async fn start_with_an_unusable_app_says_app_unusable() {
    let (status, body) = start_refusal(Setup {
        bad_key: true,
        ..Setup::default()
    })
    .await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["reason"], "app_unusable", "{body}");
}

/// An App saved in a keychain that cannot be read.
#[tokio::test]
async fn start_with_an_unreadable_keychain_says_keychain() {
    let (status, body) = start_refusal(Setup {
        keychain_locked: true,
        ..Setup::default()
    })
    .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["reason"], "keychain", "{body}");
    assert!(
        body["detail"].as_str().unwrap().contains("stored keys"),
        "{body}"
    );
}

/// A refusal that is not about the App's setup carries no `reason`: the
/// start without its CSRF token.
#[tokio::test]
async fn a_refusal_that_is_not_the_setup_carries_no_reason() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let response = ctx
        .client
        .post(ctx.url("/me/github-link/start"))
        .header(reqwest::header::COOKIE, cookie_header(&ann))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body.get("reason").is_none(), "{body}");
}

/// A person unlinks their own account: the token is revoked at GitHub and
/// the link is gone.
#[tokio::test]
async fn a_person_unlinks_their_own_account() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    link(&ctx, &github, &ann, "ghu_ann", 501, "ann-gh").await;

    let response = ctx.delete("/me/github-link", &ann).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["linked"], false, "{body}");
    assert_eq!(body["warning"], serde_json::Value::Null, "{body}");
    assert_eq!(github.revoked(), vec!["ghu_ann".to_string()]);
    assert!(ctx.link_of("ann").await.is_none());
}

/// When GitHub cannot revoke the token, the unlink still happens here and the
/// answer tells the person to revoke it themselves.
#[tokio::test]
async fn an_unlink_github_cannot_revoke_says_so() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    link(&ctx, &github, &ann, "ghu_ann", 501, "ann-gh").await;
    github.state.revoke_broken.store(true, Ordering::SeqCst);

    let response = ctx.delete("/me/github-link", &ann).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["warning"], NOT_REVOKED, "{body}");
    assert!(ctx.link_of("ann").await.is_none());
}

/// An admin unlinks another account, and its token is revoked at GitHub.
#[tokio::test]
async fn an_admin_unlinks_another_account() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let root = ctx.login("root").await;
    link(&ctx, &github, &ann, "ghu_ann", 501, "ann-gh").await;

    let response = ctx.delete("/users/ann/github-link", &root).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["linked"], false, "{body}");
    assert!(ctx.link_of("ann").await.is_none());
    assert_eq!(github.revoked(), vec!["ghu_ann".to_string()]);

    // An account that does not exist is 404.
    let response = ctx.delete("/users/nobody/github-link", &root).await;
    assert_eq!(response.status(), 404);
}

/// Unlinking somebody else is admin only.
#[tokio::test]
async fn a_viewer_cannot_unlink_another_account() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let vic = ctx.login("vic").await;
    let bob = ctx.login("bob").await;
    link(&ctx, &github, &ann, "ghu_ann", 501, "ann-gh").await;

    assert_eq!(
        ctx.delete("/users/ann/github-link", &vic).await.status(),
        403
    );
    assert_eq!(
        ctx.delete("/users/ann/github-link", &bob).await.status(),
        403
    );
    assert!(ctx.link_of("ann").await.is_some());
    assert!(github.revoked().is_empty());
}

/// The older profile disconnect goes through the App unlink when the account
/// is linked: the token is revoked at GitHub and the link is gone, instead of
/// the token being deleted here behind the link's back.
#[tokio::test]
async fn the_identity_disconnect_unlinks_through_the_app() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    link(&ctx, &github, &ann, "ghu_ann", 501, "ann-gh").await;

    let response = ctx.delete("/me/github-identity", &ann).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["connected"], false, "{body}");
    assert!(ctx.link_of("ann").await.is_none());
    assert_eq!(github.revoked(), vec!["ghu_ann".to_string()]);
}

/// When GitHub cannot revoke the token, the older profile disconnect still
/// unlinks here and its answer carries the warning, as the link DELETE does.
#[tokio::test]
async fn the_identity_disconnect_says_when_github_could_not_revoke() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    link(&ctx, &github, &ann, "ghu_ann", 501, "ann-gh").await;
    github.state.revoke_broken.store(true, Ordering::SeqCst);

    let response = ctx.delete("/me/github-identity", &ann).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["connected"], false, "{body}");
    assert_eq!(body["warning"], NOT_REVOKED, "{body}");
    assert!(ctx.link_of("ann").await.is_none());

    // A disconnect with nothing to revoke carries no warning.
    let response = ctx.delete("/me/github-identity", &ann).await;
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["warning"], serde_json::Value::Null, "{body}");
}

/// The link read says whether this instance has a GitHub App, so the profile
/// can offer the App link to every account (the settings read is admin only).
#[tokio::test]
async fn the_link_read_says_whether_an_app_is_configured() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let vic = ctx.login("vic").await;
    assert_eq!(ctx.my_link(&vic).await["app_configured"], true);

    let without = Ctx::with(
        &github,
        Setup {
            app: false,
            ..Setup::default()
        },
    )
    .await;
    let vic = without.login("vic").await;
    let mine = without.my_link(&vic).await;
    assert_eq!(mine["app_configured"], false, "{mine}");
    assert_eq!(mine["linked"], false, "{mine}");
}

// --- the single-use link an agent session hands its person ---------------------

/// Where the single-use link lives under `/api/v1`.
const GO_PATH: &str = "/github/link/go";

impl Ctx {
    /// Opens a single-use link as the browser `cookies` holds.
    async fn go(&self, nonce: &str, cookies: &Cookies) -> reqwest::Response {
        self.get(&format!("{GO_PATH}?n={nonce}"), cookies).await
    }
}

/// The nonce a link carries.
fn nonce_of(url: &str) -> String {
    let url = reqwest::Url::parse(url).unwrap();
    url.query_pairs()
        .find(|(k, _)| k == "n")
        .map(|(_, v)| v.into_owned())
        .expect("the link carries a nonce")
}

/// The first single-use link in a raw MCP answer.
fn link_in(answer: &str, addr: &SocketAddr) -> String {
    let prefix = format!("http://{addr}/api/v1{GO_PATH}?n=");
    let at = answer
        .find(&prefix)
        .unwrap_or_else(|| panic!("the answer carries a link:\n{answer}"));
    let rest = &answer[at + prefix.len()..];
    let nonce: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    format!("{prefix}{nonce}")
}

/// Asserts `response` is a page for a person: HTML, never JSON, not cached.
async fn assert_html_page(response: reqwest::Response, status: u16) -> String {
    assert_eq!(response.status(), status);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/html"), "{content_type}");
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    response.text().await.unwrap()
}

/// Asserts `response` sends the browser to GitHub's authorize url and hands
/// back the state it carries.
fn assert_authorize(response: &reqwest::Response, github: &FakeGithub, ctx: &Ctx) -> String {
    assert_eq!(response.status(), 303, "{:?}", response.headers());
    let authorize = reqwest::Url::parse(&location(response)).unwrap();
    assert!(
        authorize
            .as_str()
            .starts_with(&format!("{}/login/oauth/authorize?", github.url)),
        "{authorize}"
    );
    let query: HashMap<String, String> = authorize.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], CLIENT_ID);
    assert_eq!(
        query["redirect_uri"],
        format!("http://{}/api/v1/github/link/callback", ctx.addr)
    );
    query["state"].clone()
}

/// An agent session over HTTP, authenticated as a person, asks `configure`
/// to connect GitHub on an instance with an App: the answer is a single-use
/// link to relay, not a refusal and not the instance's device flow. It needs
/// no admin rights, because linking one's own GitHub account changes nothing
/// about the instance. Opened in that person's browser, the link goes on to
/// GitHub.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_connect_under_an_app_answers_a_single_use_link() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(
        &github,
        Setup {
            mcp_auth: true,
            ..Setup::default()
        },
    )
    .await;
    let token = ctx
        .auth
        .issue_mcp_token("vic", "agent")
        .await
        .unwrap()
        .token;
    let session = crate::support::McpTestSession::open(&ctx.addr, Some(&token)).await;
    let answer = session
        .call_tool("configure", serde_json::json!({ "connect": "github" }))
        .await;
    assert!(answer.contains("expires_in"), "{answer}");
    assert!(!answer.contains("only an instance admin"), "{answer}");
    let url = link_in(&answer, &ctx.addr);
    let nonce = nonce_of(&url);
    assert_eq!(nonce.len(), 64, "{url}");

    let vic = ctx.login("vic").await;
    let response = ctx.go(&nonce, &vic).await;
    let state = assert_authorize(&response, &github, &ctx);
    github.grant("code-v", "ghu_vic", 503, "vic-gh");
    let landed = ctx.callback(&vic, "code-v", &state).await;
    assert_eq!(location(&landed), "/profile?github=linked");
    assert_eq!(ctx.link_of("vic").await.unwrap().github_id, 503);
}

/// A link works once: the second open is refused with a page, and reaches
/// nothing.
#[tokio::test]
async fn the_link_spends_once() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let nonce = ctx.engine.mint_github_link("ann").unwrap();
    assert_authorize(&ctx.go(&nonce, &ann).await, &github, &ctx);
    let page = assert_html_page(ctx.go(&nonce, &ann).await, 404).await;
    assert!(page.contains("Ask your agent for a new link"), "{page}");
    let page = assert_html_page(ctx.go("deadbeef", &ann).await, 404).await;
    assert!(page.contains("Ask your agent for a new link"), "{page}");
}

/// A link minted while the setup was whole meets a setup that changed
/// before it was opened: `service.public_url` is gone. A person who is not
/// an admin reads the cause in plain words, with no setting named; an admin
/// reads the server's sentence, which names it.
#[tokio::test]
async fn a_link_that_cannot_start_says_why_in_plain_words_to_a_non_admin() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let vic = ctx.login("vic").await;
    let root = ctx.login("root").await;
    let for_vic = ctx.engine.mint_github_link("vic").unwrap();
    let for_root = ctx.engine.mint_github_link("root").unwrap();
    ctx.engine
        .configure(&ConfigureAction::Unset {
            key: "service.public_url".to_string(),
        })
        .await
        .unwrap();

    let page = assert_html_page(ctx.go(&for_vic, &vic).await, 409).await;
    assert!(
        page.contains(
            "This instance does not know its public address yet, so GitHub cannot send you \
             back. Ask an admin to set it."
        ),
        "{page}"
    );
    assert!(!page.contains("service.public_url"), "{page}");

    let page = assert_html_page(ctx.go(&for_root, &root).await, 409).await;
    assert!(page.contains("service.public_url"), "{page}");
    assert!(!page.contains("does not know its public address"), "{page}");
}

/// A link lives ten minutes.
#[tokio::test]
async fn the_link_expires_after_ten_minutes() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;

    let nonce = ctx.engine.mint_github_link("ann").unwrap();
    ctx.advance(Duration::from_secs(10 * 60 + 1));
    assert_html_page(ctx.go(&nonce, &ann).await, 404).await;

    let nonce = ctx.engine.mint_github_link("ann").unwrap();
    ctx.advance(Duration::from_secs(9 * 60 + 59));
    assert_authorize(&ctx.go(&nonce, &ann).await, &github, &ctx);
}

/// A forwarded link cannot link somebody else's GitHub account to the
/// sender's: opened by another account it is refused with a page that names
/// nobody, and spent, so the sender's own browser cannot use it either.
#[tokio::test]
async fn a_link_opened_by_another_account_is_refused() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let bob = ctx.login("bob").await;
    let nonce = ctx.engine.mint_github_link("ann").unwrap();

    let page = assert_html_page(ctx.go(&nonce, &bob).await, 403).await;
    assert!(page.contains("another account"), "{page}");
    assert!(
        !page
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "ann"),
        "the page names nobody: {page}"
    );
    assert_html_page(ctx.go(&nonce, &ann).await, 404).await;
    assert_eq!(github.token_hits(), 0);
    assert!(ctx.link_of("ann").await.is_none());
    assert!(ctx.link_of("bob").await.is_none());
}

/// Without a session the link sends the browser to the sign-in page and
/// back, and is not spent by that: the sign-in page carries the nonce and
/// nothing else, with or without single sign-on.
#[tokio::test]
async fn a_link_without_a_session_goes_to_sign_in_and_back() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let nonce = ctx.engine.mint_github_link("ann").unwrap();
    let response = ctx.go(&nonce, &Vec::new()).await;
    assert_eq!(response.status(), 303);
    assert_eq!(location(&response), format!("/login?github_link={nonce}"));
    let ann = ctx.login("ann").await;
    assert_authorize(&ctx.go(&nonce, &ann).await, &github, &ctx);

    // A nonce that is not one never rides into the sign-in page.
    let response = ctx.go("%2F%2Fevil.example", &Vec::new()).await;
    assert_html_page(response, 404).await;

    let sso = Ctx::with(
        &github,
        Setup {
            sso: true,
            ..Setup::default()
        },
    )
    .await;
    let nonce = sso.engine.mint_github_link("ann").unwrap();
    let response = sso.go(&nonce, &Vec::new()).await;
    assert_eq!(response.status(), 303);
    // With single sign-on too it is the sign-in page, so a person on an
    // instance with both passwords and a provider can choose; the page's
    // provider button carries the same return.
    assert_eq!(location(&response), format!("/login?github_link={nonce}"));
    let ann = sso.login("ann").await;
    assert_authorize(&sso.go(&nonce, &ann).await, &github, &sso);
}

/// A good link goes on into the browser link flow: GitHub's authorize url
/// with a state that the callback accepts from this session, so the link
/// lands on the account the link was made for.
#[tokio::test]
async fn a_good_link_lands_on_githubs_authorize_url_with_a_state() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let nonce = ctx.engine.mint_github_link("ann").unwrap();
    let response = ctx.go(&nonce, &ann).await;
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let state = assert_authorize(&response, &github, &ctx);
    github.grant("code-1", "ghu_ann", 501, "ann-gh");
    let landed = ctx.callback(&ann, "code-1", &state).await;
    assert_eq!(location(&landed), "/profile?github=linked");
    assert_eq!(ctx.link_of("ann").await.unwrap().github_id, 501);
}

/// One `configure` call over stdio, the way a local harness makes it.
async fn stdio_configure(server: McpServer, args: serde_json::Value) -> Result<String, String> {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn(async move { rmcp::serve_server(server, server_io).await });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let _server = served.await.unwrap().unwrap();
    let mut params = CallToolRequestParams::new("configure");
    if let serde_json::Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let answer = match client.call_tool(params).await {
        Ok(result) => {
            let value = serde_json::to_value(&result).unwrap();
            let text = value
                .pointer("/content/0/text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if value["isError"] == true {
                Err(text)
            } else {
                Ok(text)
            }
        }
        Err(e) => Err(e.to_string()),
    };
    client.cancel().await.unwrap();
    answer
}

/// The machine owner on stdio has no account to link a GitHub account to:
/// the answer names the CLI command that links the owner's identity, and no
/// link is made.
#[tokio::test]
async fn the_owner_on_stdio_gets_the_cli_command() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let answer = stdio_configure(
        McpServer::new(ctx.engine.clone()),
        serde_json::json!({ "connect": "github" }),
    )
    .await
    .expect("an answer, not a refusal");
    assert!(
        answer.contains("crystalline connect github --personal"),
        "{answer}"
    );
    assert!(!answer.contains(GO_PATH), "{answer}");
    assert!(!answer.contains("user_code"), "no device flow: {answer}");
}

/// An unattended session answers people it has never met, so it never hands
/// out a link that would link somebody's GitHub account.
#[tokio::test]
async fn an_unattended_session_cannot_mint_a_link() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let refused = crystalline_service::read_limit::within(
        Some(crystalline_service::read_limit::Limit::default()),
        async { ctx.engine.mint_github_link("ann") },
    )
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("unattended"), "{refused}");

    let answer = stdio_configure(
        McpServer::new(ctx.engine.clone()).with_unattended(true),
        serde_json::json!({ "connect": "github" }),
    )
    .await
    .expect_err("refused");
    assert!(answer.contains("unattended"), "{answer}");
}

/// One account holds at most one live link: a new one replaces the earlier
/// one, which stops working, and another account's link is untouched.
#[tokio::test]
async fn a_new_link_replaces_the_accounts_earlier_one() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let ann = ctx.login("ann").await;
    let bob = ctx.login("bob").await;
    let first = ctx.engine.mint_github_link("ann").unwrap();
    let bobs = ctx.engine.mint_github_link("bob").unwrap();
    let second = ctx.engine.mint_github_link("ann").unwrap();
    assert_html_page(ctx.go(&first, &ann).await, 404).await;
    assert_authorize(&ctx.go(&second, &ann).await, &github, &ctx);
    assert_authorize(&ctx.go(&bobs, &bob).await, &github, &ctx);
}

/// A removed account's link dies with it, so an account created later under
/// the same name cannot use it.
#[tokio::test]
async fn removing_an_account_drops_its_link() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let root = ctx.login("root").await;
    let nonce = ctx.engine.mint_github_link("bob").unwrap();
    let removed = ctx.delete("/users/bob", &root).await;
    assert_eq!(removed.status(), 204);
    ctx.auth
        .add_user("bob", "bob", None, Role::Editor, PASSWORD)
        .await
        .unwrap();
    let bob = ctx.login("bob").await;
    assert_html_page(ctx.go(&nonce, &bob).await, 404).await;

    // The CLI removes an account in its own process and tells the daemon to
    // forget that person's credential; the link goes with it.
    let nonce = ctx.engine.mint_github_link("ann").unwrap();
    ctx.engine.forget_cached_credential(Some("ann")).unwrap();
    let ann = ctx.login("ann").await;
    assert_html_page(ctx.go(&nonce, &ann).await, 404).await;
}

/// An agent that is not signed in as an account (an instance that does not
/// make agents authenticate) has nothing to link: it is told how the person
/// links from their profile, and no link is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_anonymous_agent_is_told_to_link_from_the_profile() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::start(&github).await;
    let session = crate::support::McpTestSession::open(&ctx.addr, None).await;
    let answer = session
        .call_tool("configure", serde_json::json!({ "connect": "github" }))
        .await;
    assert!(answer.contains("not signed in as an account"), "{answer}");
    assert!(!answer.contains(GO_PATH), "{answer}");
    assert!(!answer.contains("user_code"), "no device flow: {answer}");
}

/// Without `service.public_url` there is no address to build a link with:
/// nothing is minted, and the answer says what an admin sets.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_public_url_the_answer_names_the_setting() {
    let github = FakeGithub::start().await;
    let ctx = Ctx::with(
        &github,
        Setup {
            mcp_auth: true,
            public_url: false,
            ..Setup::default()
        },
    )
    .await;
    let token = ctx
        .auth
        .issue_mcp_token("ann", "agent")
        .await
        .unwrap()
        .token;
    let session = crate::support::McpTestSession::open(&ctx.addr, Some(&token)).await;
    let answer = session
        .call_tool("configure", serde_json::json!({ "connect": "github" }))
        .await;
    assert!(
        answer.contains(
            "Linking GitHub through the App needs service.public_url, which this instance does \
             not have. An admin sets it (crystalline config set service.public_url <url>), then \
             ask again."
        ),
        "{answer}"
    );
    assert!(!answer.contains("profile"), "{answer}");
    assert!(!answer.contains(GO_PATH), "{answer}");
}
