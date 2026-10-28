//! The GitHub App web authorization, driven end to end over HTTP: a link
//! minted by the JSON API, spent by a browser, answered by a GitHub this
//! process runs itself.
//!
//! Nothing here reaches the internet. `MockGitHub` is a small axum server on an
//! ephemeral loopback port serving the one endpoint the exchange talks to, and
//! `service.public_url` names the instance's own address, so a minted link
//! points back at the test server and can actually be followed. The browser is
//! a `reqwest` client with redirects off and its cookies carried by hand, the
//! way `oidc.rs` drives the sign-on flow, so every hop is one assertion.
//!
//! Two properties of the flow are pinned one layer down rather than here,
//! because neither is reachable through HTTP: the five minute expiry of a
//! nonce (the engine's own tests reach into the pending store to age one) and
//! the single refresh N concurrent callers of an expiring credential make
//! between them. Both live in `crates/engine/src/engine/github_web.rs`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex as StdMutex};

use axum::Json;
use axum::extract::State;
use axum::routing::post;
use crystalline_core::config::{
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, OidcConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use serde_json::{Value, json};

/// The registered app this instance authenticates as, and the password every
/// account in this file signs in with.
const CLIENT_ID: &str = "Iv1.web-flow-test";
const CLIENT_SECRET: &str = "shhh-this-is-the-github-app-secret";
const PASSWORD: &str = "correct horse battery staple";

// --- the fake GitHub --------------------------------------------------------

/// GitHub's token endpoint, and a record of what was posted to it.
#[derive(Default)]
struct MockGitHub {
    exchanges: StdMutex<Vec<Value>>,
}

/// A GitHub on a loopback port that answers every exchange with one expiring,
/// refreshable pair.
async fn spawn_github() -> (String, Arc<MockGitHub>) {
    let mock = Arc::new(MockGitHub::default());
    let app = axum::Router::new()
        .route(
            "/login/oauth/access_token",
            post(
                |State(mock): State<Arc<MockGitHub>>, body: String| async move {
                    mock.exchanges
                        .lock()
                        .unwrap()
                        .push(serde_json::from_str(&body).unwrap());
                    Json(json!({
                        "access_token": "ghu_from_the_browser",
                        "expires_in": 28800,
                        "refresh_token": "ghr_rotating",
                        "refresh_token_expires_in": 15_811_200,
                        "token_type": "bearer",
                    }))
                },
            ),
        )
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), mock)
}

// --- the instance under test ------------------------------------------------

/// What an instance is built with: whether a GitHub App of its own is
/// registered, and whether there is a sign-on provider to send a session-less
/// browser to.
struct Setup {
    app: bool,
    sso: bool,
}

/// A served Crystalline instance, the browser that drives it and the GitHub it
/// talks to.
struct Ctx {
    addr: SocketAddr,
    client: reqwest::Client,
    github: Arc<MockGitHub>,
    tokens: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Ctx {
    /// An instance with a GitHub App registered and no sign-on provider.
    async fn with_app() -> Ctx {
        Ctx::build(Setup {
            app: true,
            sso: false,
        })
        .await
    }

    async fn build(setup: Setup) -> Ctx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir = root.join("eng");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: Eng\npermalink: MANIFEST\ntags: []\nstatus: stable\n\
             recorded_at: 2026-01-01T00:00:00Z\n---\n\n# Eng\n",
        )
        .unwrap();
        let (api_url, github) = spawn_github().await;

        // Bound first, so `service.public_url` can name the address this
        // instance is actually about to serve on: a minted link points at it,
        // and the browser in these tests follows the link for real.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();

        let mut config = GlobalConfig {
            github: Some(GitHubConfig {
                enabled: Some(true),
                share_identity: Some("personal".to_string()),
                api_url: Some(api_url),
                oauth_client_id: setup.app.then(|| CLIENT_ID.to_string()),
                oauth_client_secret: setup.app.then(|| CLIENT_SECRET.to_string()),
                ..GitHubConfig::default()
            }),
            auth: Some(AuthConfig {
                oidc: setup.sso.then(|| OidcConfig {
                    issuer: Some("https://idp.example.test".to_string()),
                    client_id: Some("crystalline".to_string()),
                    client_secret: Some("idp-secret".to_string()),
                    name: Some("Contoso".to_string()),
                    scopes: None,
                    default_role: None,
                    redirect_uri: None,
                }),
                ..AuthConfig::default()
            }),
            service: Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                public_url: Some(format!("http://{addr}")),
                ..ServiceConfig::default()
            }),
            ..GlobalConfig::default()
        };
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &config).unwrap();

        let store = TursoStore::open_in_memory().await.unwrap();
        let tokens = root.join("tokens");
        // Never the developer's keychain, and never the real device flow: the
        // web half is what is under test, and the stub validates whatever
        // token the exchange brings back as one login.
        // Never released: a device flow started here stays in flight, which is
        // the state the fallback test reads, and no outcome of it can land on
        // top of what the web half stores.
        let (denying, _gate) = crate::support::StubConnectAuth::denying("the flow was declined");
        let engine = Arc::new(
            Engine::new(
                Arc::new(tokio::sync::Mutex::new(store)),
                config,
                None,
                Some(config_path),
            )
            .with_token_store_dir(tokens.clone())
            .with_connect_auth(Arc::new(if setup.app {
                crate::support::StubConnectAuth::accepting("octo")
            } else {
                denying
            })),
        );
        engine.sync(None).await.unwrap();

        let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
        auth.add_user("alice", "Alice", None, Role::Editor, PASSWORD)
            .await
            .unwrap();
        auth.add_user("bob", "Bob", None, Role::Editor, PASSWORD)
            .await
            .unwrap();

        let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth, None).unwrap();
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
            github,
            tokens,
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}/api/v1{path}", self.addr)
    }

    /// Sign an account in and hand back the cookies it holds.
    async fn login(&self, name: &str) -> Vec<(String, String)> {
        let response = self
            .client
            .post(self.url("/auth/login"))
            .json(&json!({ "name": name, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "the local login should succeed");
        cookies_from(&response)
    }

    async fn get(&self, url: &str, cookies: &[(String, String)]) -> reqwest::Response {
        let mut request = self.client.get(url);
        if !cookies.is_empty() {
            request = request.header(reqwest::header::COOKIE, cookie_header(cookies));
        }
        request.send().await.unwrap()
    }

    /// `POST /me/github-identity/connect`, carrying the session's CSRF token.
    async fn start_connect(&self, cookies: &[(String, String)]) -> Value {
        let probe: Value = self
            .get(&self.url("/auth/me"), cookies)
            .await
            .json()
            .await
            .unwrap();
        let csrf = probe["csrf"].as_str().expect("a session carries a csrf");
        let response = self
            .client
            .post(self.url("/me/github-identity/connect"))
            .header(reqwest::header::COOKIE, cookie_header(cookies))
            .header("x-csrf-token", csrf)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 202, "a started connection answers 202");
        response.json().await.unwrap()
    }

    /// The caller's own identity as the profile card reads it.
    async fn identity(&self, cookies: &[(String, String)]) -> Value {
        self.get(&self.url("/me/github-identity"), cookies)
            .await
            .json()
            .await
            .unwrap()
    }

    /// Walk a minted link to the authorization url GitHub would have shown,
    /// and hand back the state it carries.
    async fn state_of(&self, link: &str, cookies: &[(String, String)]) -> String {
        let redirect = self.get(link, cookies).await;
        assert_eq!(redirect.status(), 302, "a good link redirects to GitHub");
        let authorize = location(&redirect);
        assert!(
            authorize.contains("code_challenge_method=S256"),
            "{authorize}"
        );
        query_value(&authorize, "state")
    }

    /// Every credential on file, as the store wrote them.
    fn stored(&self) -> Vec<Value> {
        let mut found = Vec::new();
        collect_json(&self.tokens, &mut found);
        found
    }
}

/// Every JSON file under `dir`, parsed. The token store names its files
/// itself, so a test asks what is there rather than guessing a path.
fn collect_json(dir: &Path, into: &mut Vec<Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json(&path, into);
        } else if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            into.push(value);
        }
    }
}

fn cookie_header(cookies: &[(String, String)]) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn cookies_from(response: &reqwest::Response) -> Vec<(String, String)> {
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

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("a redirect carries a location")
        .to_str()
        .unwrap()
        .to_string()
}

fn query_value(url: &str, key: &str) -> String {
    let (_, query) = url.split_once('?').expect("no query");
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no {key} in {url}"))
        .to_string()
}

/// The link the connect route answered with.
fn link_of(started: &Value) -> String {
    started["authorize_url"]
        .as_str()
        .unwrap_or_else(|| panic!("no authorize_url in {started}"))
        .to_string()
}

// --- the tests --------------------------------------------------------------

/// The whole walk: a link minted for one account, spent by that account's
/// browser, answered by GitHub, and a refreshable credential on file.
#[tokio::test]
async fn a_full_walk_connects_the_account_and_stores_a_refreshable_credential() {
    let ctx = Ctx::with_app().await;
    let alice = ctx.login("alice").await;

    let started = ctx.start_connect(&alice).await;
    let link = link_of(&started);
    assert!(
        link.starts_with(&format!(
            "http://{}/api/v1/auth/github/connect?n=",
            ctx.addr
        )),
        "{link}"
    );
    assert_eq!(started["authorize_url_expires_in_secs"], 300);
    assert!(
        started["pending"].is_null(),
        "a browser link is not a device code: {started}"
    );

    let state = ctx.state_of(&link, &alice).await;
    let done = ctx
        .get(
            &format!(
                "{}?state={state}&code=the-code",
                ctx.url("/auth/github/callback")
            ),
            &[],
        )
        .await;

    assert_eq!(done.status(), 200);
    assert_eq!(done.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(done.headers()["cache-control"], "no-store");
    let page = done.text().await.unwrap();
    assert!(page.contains("octo"), "{page}");
    assert!(page.contains("alice"), "{page}");

    let sent = ctx.github.exchanges.lock().unwrap()[0].clone();
    assert_eq!(sent["client_id"], CLIENT_ID);
    assert_eq!(sent["client_secret"], CLIENT_SECRET);
    assert_eq!(sent["code"], "the-code");
    assert_eq!(
        sent["redirect_uri"],
        format!("http://{}/api/v1/auth/github/callback", ctx.addr)
    );
    assert!(sent["code_verifier"].as_str().unwrap().len() >= 43);

    let identity = ctx.identity(&alice).await;
    assert_eq!(identity["connected"], true);
    assert_eq!(identity["login"], "octo");

    let stored = ctx.stored();
    assert_eq!(stored.len(), 1, "one credential, for one account");
    assert_eq!(stored[0]["access_token"], "ghu_from_the_browser");
    assert_eq!(stored[0]["refresh_token"], "ghr_rotating");
    assert!(stored[0]["expires_at"].is_string(), "{}", stored[0]);
    assert!(
        stored[0]["refresh_token_expires_at"].is_string(),
        "{}",
        stored[0]
    );
}

/// A link is spent on the way in, so the second browser to present one gets
/// the "start again" refusal rather than a second authorization.
#[tokio::test]
async fn a_link_is_spendable_exactly_once() {
    let ctx = Ctx::with_app().await;
    let alice = ctx.login("alice").await;
    let link = link_of(&ctx.start_connect(&alice).await);

    ctx.state_of(&link, &alice).await;

    let again = ctx.get(&link, &alice).await;
    assert_eq!(again.status(), 422);
    let page = again.text().await.unwrap();
    assert!(page.contains("already been used"), "{page}");
}

/// The attack the account binding exists for: a link forwarded to somebody
/// else connects nothing, and is burnt in the attempt.
#[tokio::test]
async fn a_link_presented_by_another_account_is_forbidden_and_spent() {
    let ctx = Ctx::with_app().await;
    let alice = ctx.login("alice").await;
    let bob = ctx.login("bob").await;
    let link = link_of(&ctx.start_connect(&alice).await);

    let stolen = ctx.get(&link, &bob).await;
    assert_eq!(stolen.status(), 403);
    let page = stolen.text().await.unwrap();
    assert!(page.contains("another account"), "{page}");

    let after = ctx.get(&link, &alice).await;
    assert_eq!(
        after.status(),
        422,
        "single use means spent on the refusal too"
    );
    assert!(ctx.stored().is_empty(), "nothing was connected");
}

/// A callback nobody started: the state names no handshake, so there is
/// nothing to complete and nothing is stored.
#[tokio::test]
async fn a_state_that_matches_no_handshake_is_refused() {
    let ctx = Ctx::with_app().await;
    let alice = ctx.login("alice").await;
    // One handshake really is in flight, so the refusal is about the state
    // presented rather than about there being none at all.
    let link = link_of(&ctx.start_connect(&alice).await);
    ctx.state_of(&link, &alice).await;

    let forged = ctx
        .get(
            &format!(
                "{}?state=not-a-state&code=the-code",
                ctx.url("/auth/github/callback")
            ),
            &[],
        )
        .await;

    assert_eq!(forged.status(), 422);
    assert!(ctx.github.exchanges.lock().unwrap().is_empty());
    assert!(ctx.stored().is_empty());
}

/// What a person sees when they press Cancel at GitHub, with GitHub's own
/// words rendered as text rather than as markup.
#[tokio::test]
async fn a_declined_authorization_says_so_and_escapes_what_github_sent() {
    let ctx = Ctx::with_app().await;

    let declined = ctx
        .get(
            &format!(
                "{}?error=access_denied&error_description=The%20user%20%3Cb%3Edenied%3C%2Fb%3E%20it",
                ctx.url("/auth/github/callback")
            ),
            &[],
        )
        .await;

    assert_eq!(declined.status(), 400);
    let page = declined.text().await.unwrap();
    assert!(page.contains("&lt;b&gt;denied&lt;/b&gt;"), "{page}");
    assert!(!page.contains("<b>denied</b>"), "{page}");
    assert!(ctx.stored().is_empty());
}

/// A browser that is nobody yet is sent to the provider and back to the same
/// link, which is still unspent because nothing has judged it.
#[tokio::test]
async fn a_session_less_browser_is_sent_through_sign_on_and_back_to_the_link() {
    let ctx = Ctx::build(Setup {
        app: true,
        sso: true,
    })
    .await;
    let alice = ctx.login("alice").await;
    let link = link_of(&ctx.start_connect(&alice).await);

    let bounced = ctx.get(&link, &[]).await;

    assert_eq!(bounced.status(), 302);
    let to = location(&bounced);
    assert!(to.starts_with("/api/v1/auth/oidc/login?return_to="), "{to}");
    assert!(to.contains("auth%2Fgithub%2Fconnect"), "{to}");

    // Unspent: the same link still works for the account it was minted for.
    ctx.state_of(&link, &alice).await;
}

/// With no provider there is nowhere to send a session-less browser, so it is
/// told what to do in one sentence instead.
#[tokio::test]
async fn a_session_less_browser_with_no_provider_is_told_to_sign_in() {
    let ctx = Ctx::with_app().await;
    let alice = ctx.login("alice").await;
    let link = link_of(&ctx.start_connect(&alice).await);

    let refused = ctx.get(&link, &[]).await;

    assert_eq!(refused.status(), 401);
    assert_eq!(
        refused.headers()["content-type"],
        "text/html; charset=utf-8"
    );
    let page = refused.text().await.unwrap();
    assert!(page.contains("Sign in"), "{page}");
}

/// An instance that registered no GitHub App of its own is untouched by any of
/// this: the connect answers the device code it always did, and the browser
/// routes say which setting is missing.
#[tokio::test]
async fn an_instance_with_no_app_still_answers_a_device_code() {
    let ctx = Ctx::build(Setup {
        app: false,
        sso: false,
    })
    .await;
    let alice = ctx.login("alice").await;

    let started = ctx.start_connect(&alice).await;
    assert!(
        started["authorize_url"].is_null(),
        "no app, no link: {started}"
    );
    assert_eq!(started["pending"]["user_code"], "ABCD-1234");

    let refused = ctx
        .get(&ctx.url("/auth/github/connect?n=anything"), &alice)
        .await;
    assert_eq!(refused.status(), 422);
    let page = refused.text().await.unwrap();
    assert!(page.contains("github.oauth_client_secret"), "{page}");
}

/// A link with nothing in it is a page a person can act on rather than an
/// extractor's refusal.
#[tokio::test]
async fn a_link_with_no_nonce_says_the_link_is_incomplete() {
    let ctx = Ctx::with_app().await;
    let alice = ctx.login("alice").await;

    let empty = ctx.get(&ctx.url("/auth/github/connect"), &alice).await;

    assert_eq!(empty.status(), 400);
    let page = empty.text().await.unwrap();
    assert!(page.contains("incomplete"), "{page}");
}
