//! Whose identity a share runs on when `github.share_identity` is not set:
//! the person's with a GitHub App configured, usable or not, and the instance
//! credential without one. An explicit key always wins and an unreadable value
//! keeps reading as `instance`.
//!
//! Each matrix walks (no App, App ready, App refused) by (key absent,
//! `instance`, `personal`, garbage).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::Utc;
use crystalline_core::config::save_yaml;
use crystalline_core::config::{AuthConfig, GitHubConfig, GlobalConfig, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_remote::{AppCredentials, AppLookup, AppStore, RemoteError, UserTokenGrant};
use crystalline_service::daemon::http_router;
use crystalline_service::engine::{ConfigureAction, EngineError, Invalidate, ShareActor};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EnvOverlay, Scope};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::github_app::{
    Forge, INSTANCE_TOKEN, PEM, bearer, connect, engram, repo_with_manifest, spawn_forge,
};
use super::github_governed::{PASSWORD, client, login, with_session};

#[derive(Clone, Copy, Debug, PartialEq)]
enum App {
    None,
    Ready,
    Refused,
    /// Saved in a keychain that cannot be read.
    Locked,
}

const APPS: [App; 3] = [App::None, App::Ready, App::Refused];
const KEYS: [Option<&str>; 4] = [None, Some("instance"), Some("personal"), Some("nonsense")];

/// The mode the rule gives one cell.
fn expected_personal(app: App, key: Option<&str>) -> bool {
    match key {
        Some(word) => word == "personal",
        None => app != App::None,
    }
}

struct Case {
    forge: Arc<Forge>,
    eng: Arc<Engine>,
    auth: Arc<AuthStore>,
    addr: std::net::SocketAddr,
    tmp: tempfile::TempDir,
}

fn credentials(host: &str) -> AppCredentials {
    AppCredentials {
        app_id: 7,
        client_id: "Iv23.test".to_string(),
        client_secret: Some("app-client-secret".to_string()),
        private_key: PEM.to_string(),
        webhook_secret: None,
        slug: Some("crystalline-acme".to_string()),
        host: host.to_string(),
    }
}

/// One instance in one cell: the domain `kb` connected while the App still
/// works, one new engram waiting, the accounts `ada` (editor), `root` (admin)
/// and `vic` (viewer), and the JSON API served. A refused App is a saved App
/// for another host from the moment the domain is connected on.
async fn case(app: App, key: Option<&str>) -> Case {
    case_with_clock(
        app,
        key,
        Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
    )
    .await
}

async fn case_with_clock(
    app: App,
    key: Option<&str>,
    now: Arc<std::sync::Mutex<std::time::Instant>>,
) -> Case {
    case_with_auth(app, key, now, None).await
}

async fn case_with_auth(
    app: App,
    key: Option<&str>,
    now: Arc<std::sync::Mutex<std::time::Instant>>,
    connect_auth: Option<Arc<crate::support::StubConnectAuth>>,
) -> Case {
    case_full(app, key, now, connect_auth, None, true).await
}

async fn case_full(
    app: App,
    key: Option<&str>,
    now: Arc<std::sync::Mutex<std::time::Instant>>,
    connect_auth: Option<Arc<crate::support::StubConnectAuth>>,
    mcp_auth: Option<bool>,
    public_url: bool,
) -> Case {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(url.clone()),
            share_identity: key.map(str::to_string),
            ..GitHubConfig::default()
        }),
        service: Some(ServiceConfig {
            public_url: public_url.then(|| "https://kb.example.com".into()),
            ..Default::default()
        }),
        auth: Some(AuthConfig {
            oauth: Some(false),
            mcp: mcp_auth,
            ..AuthConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let config_path = root.join("config.yaml");
    save_yaml(&config_path, &cfg).unwrap();
    let overlay = EnvOverlay::from_vars([(
        "CRYSTALLINE_GITHUB_TOKEN".to_string(),
        INSTANCE_TOKEN.to_string(),
    )])
    .unwrap();
    let mut eng = Engine::new(
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
        cfg,
        None,
        Some(config_path),
    )
    .with_origins_dir(root.join("origins"))
    .with_state_dir(root.to_path_buf())
    .with_token_store_dir(root.join("tokens"))
    .with_env_overlay(overlay)
    .with_app_clock(Arc::new(move || *now.lock().unwrap()));
    let broken = Arc::new(AtomicBool::new(false));
    if app != App::None {
        let host = url.trim_start_matches("http://").to_string();
        let dir = root.join("tokens");
        let broken = broken.clone();
        let locked = app == App::Locked;
        let broke = broken.clone();
        eng = eng.with_app_store_lookup(Arc::new(move || {
            if locked && broke.load(Ordering::SeqCst) {
                return Ok(AppLookup {
                    store: AppStore::file_fallback(&dir),
                    credentials: None,
                    keychain_failed: Some("the keychain is locked".to_string()),
                    marker: Some(String::new()),
                });
            }
            let host = if broken.load(Ordering::SeqCst) {
                "github.com"
            } else {
                host.as_str()
            };
            Ok(AppLookup {
                store: AppStore::file_fallback(&dir),
                credentials: Some(credentials(host)),
                keychain_failed: None,
                marker: None,
            })
        }));
    }
    if let Some(stub) = connect_auth {
        eng = eng.with_connect_auth(stub);
    }
    let eng = Arc::new(eng);
    let domain_root = root.join("kb");
    connect(&eng, "acme/kb", "kb", &domain_root).await;
    std::fs::create_dir_all(domain_root.join("notes")).unwrap();
    std::fs::write(
        domain_root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();
    if app == App::Refused || app == App::Locked {
        broken.store(true, Ordering::SeqCst);
        eng.reset_github_app();
    }
    // The slot settles, as the load at start does on a daemon.
    assert_eq!(eng.github_app().is_some(), app == App::Ready);

    let auth = Arc::new(AuthStore::open(&root.join("auth.db")).await.unwrap());
    for (name, role) in [
        ("root", Role::Admin),
        ("ada", Role::Editor),
        ("vic", Role::Viewer),
    ] {
        auth.add_user(name, name, Some("x@example.com"), role, PASSWORD)
            .await
            .unwrap();
    }
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth.clone()).with_governor(eng.knowledge_governor()),
    ));
    let router = http_router(
        eng.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        None,
    )
    .unwrap();
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
    Case {
        forge,
        eng,
        auth,
        addr,
        tmp,
    }
}

async fn share_as_ada(eng: &Engine) -> Result<Value, EngineError> {
    eng.origin_share(
        "kb",
        None,
        None,
        None,
        None,
        ShareActor::Account("ada".to_string()),
    )
    .await
}

/// The `Authorization` of every write the forge saw.
fn writes(forge: &Forge) -> Vec<Option<String>> {
    forge
        .hits()
        .into_iter()
        .filter(|h| h.is_repo_call() && h.method != "GET")
        .map(|h| h.auth)
        .collect()
}

async fn can_share(case: &Case, name: &str) -> bool {
    let session = login(case.addr, name).await.unwrap();
    let me: Value = with_session(
        client().get(format!("http://{}/api/v1/auth/me", case.addr)),
        Some(&session),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    me["can_share"].as_bool().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_engine_answers_the_mode_in_every_cell() {
    for app in APPS {
        for key in KEYS {
            let fx = case(app, key).await;
            let personal = expected_personal(app, key);
            let word = if personal { "personal" } else { "instance" };
            assert_eq!(
                fx.eng.share_identity_mode().as_str(),
                word,
                "{app:?} by {key:?}"
            );
            let status = fx
                .eng
                .origin_status(None, false, false, &Scope::Unrestricted)
                .await
                .unwrap();
            let connection = &status["connection"];
            assert_eq!(connection["share_identity"], word, "{app:?} by {key:?}");
            assert_eq!(
                connection["share_identity_default"],
                key.is_none(),
                "{app:?} by {key:?}: {connection}"
            );
            let reason = connection["share_identity_reason"].as_str().unwrap_or("");
            let wanted = match (key, app) {
                (Some(_), _) => "set by github.share_identity",
                (None, App::None) => "default, because no GitHub App is configured",
                (None, _) => "default, because a GitHub App is configured",
            };
            assert_eq!(reason, wanted, "{app:?} by {key:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_share_gate_follows_the_mode_in_every_cell() {
    for app in APPS {
        for key in KEYS {
            let fx = case(app, key).await;
            let personal = expected_personal(app, key);
            assert!(can_share(&fx, "root").await, "admin, {app:?} by {key:?}");
            // A refused App refuses every share when personal is only the
            // default, so the role answer would promise what the share refuses.
            let editor = personal && !(app == App::Refused && key.is_none());
            assert_eq!(
                can_share(&fx, "ada").await,
                editor,
                "editor, {app:?} by {key:?}"
            );
            assert!(!can_share(&fx, "vic").await, "viewer, {app:?} by {key:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_share_goes_where_the_mode_says_in_every_cell() {
    for app in APPS {
        for key in KEYS {
            let fx = case(app, key).await;
            let personal = expected_personal(app, key);
            let answer = share_as_ada(&fx.eng).await;
            let label = format!("{app:?} by {key:?}: {answer:?}");
            match (app, personal, key) {
                // A refused App refuses with its own reason unless the admin
                // asked for personal sharing by name.
                (App::Refused, _, key) if key != Some("personal") => {
                    assert!(
                        matches!(&answer, Err(EngineError::Remote(RemoteError::Refused(d))) if d.contains("cannot be used right now")),
                        "{label}"
                    );
                }
                (App::Ready, true, _) => {
                    let text = answer.as_ref().expect_err(&label).to_string();
                    assert!(text.contains("Link yours in your profile"), "{label}");
                    assert!(!text.contains("crystalline "), "no command line: {label}");
                }
                (_, true, _) => {
                    let text = answer.as_ref().expect_err(&label).to_string();
                    assert!(text.contains("Connect yours in your profile"), "{label}");
                    assert!(!text.contains("crystalline "), "no command line: {label}");
                }
                (App::Ready, false, _) => {
                    assert_eq!(answer.as_ref().unwrap()["outcome"], "proposed", "{label}");
                    assert!(
                        writes(&fx.forge).iter().all(|a| *a == bearer("ghs_test")),
                        "{label}"
                    );
                }
                (_, false, _) => {
                    assert_eq!(answer.as_ref().unwrap()["outcome"], "proposed", "{label}");
                    assert!(
                        writes(&fx.forge)
                            .iter()
                            .all(|a| *a == bearer(INSTANCE_TOKEN)),
                        "{label}"
                    );
                }
            }
            if answer.is_err() {
                assert!(writes(&fx.forge).is_empty(), "{label}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_an_app_and_no_key_a_linked_person_opens_the_proposal() {
    let fx = case(App::Ready, None).await;
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("ghu_ada".to_string(), (1001, "ada-gh".to_string()));
    fx.eng.link_github("ada", grant("ghu_ada")).await.unwrap();
    let answer = share_as_ada(&fx.eng).await.unwrap();
    assert_eq!(answer["outcome"], "proposed", "{answer}");
    let all = writes(&fx.forge);
    assert!(!all.is_empty());
    assert!(all.iter().all(|a| *a == bearer("ghu_ada")), "{all:?}");
    let state = crystalline_remote::state::OriginState::load(&fx.tmp.path().join("origins/kb"))
        .unwrap()
        .unwrap();
    assert_eq!(state.proposals[0].author_login.as_deref(), Some("ada-gh"));
}

/// With an App and no key, the machine owner and an HTTP agent with no
/// `github.agent_identity` have nobody to be until someone says so: each share
/// is refused with the words that name the fix, and nothing goes out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_an_app_and_no_key_the_owner_and_an_open_agent_are_refused_with_the_fix() {
    for (actor, words) in [
        (ShareActor::Owner, "through its GitHub App"),
        (ShareActor::HttpAgent, "Ask an admin to name the account"),
    ] {
        let fx = case(App::Ready, None).await;
        let text = fx
            .eng
            .origin_share("kb", None, None, None, None, actor)
            .await
            .unwrap_err()
            .to_string();
        assert!(text.contains(words), "{text}");
        assert!(writes(&fx.forge).is_empty());
    }
}

fn grant(access: &str) -> UserTokenGrant {
    UserTokenGrant {
        access_token: access.to_string(),
        refresh_token: None,
        expires_at: Some(Utc::now() + chrono::Duration::hours(8)),
        refresh_expires_at: None,
    }
}

/// While the slot is not settled a gate does not wait for the store: it reads
/// nothing and stays on the narrower role gate.
#[tokio::test]
async fn an_unsettled_app_keeps_the_gate_narrow_and_reads_no_store() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let host = url.trim_start_matches("http://").to_string();
    AppStore::file_fallback(&tmp.path().join("tokens"))
        .save(&credentials(&host))
        .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = reads.clone();
    let dir = tmp.path().join("tokens");
    let eng = base_engine(tmp.path(), &url)
        .await
        .with_app_store_lookup(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(AppLookup {
                store: AppStore::file_fallback(&dir),
                credentials: Some(credentials(&host)),
                keychain_failed: None,
                marker: None,
            })
        }));
    assert_eq!(eng.share_identity_mode().as_str(), "instance");
    assert_eq!(reads.load(Ordering::SeqCst), 0, "the gate read no store");
    assert!(eng.github_app().is_some());
    assert_eq!(eng.share_identity_mode().as_str(), "personal");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

async fn base_engine(tmp: &Path, url: &str) -> Engine {
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(url.to_string()),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let config_path = tmp.join("config.yaml");
    save_yaml(&config_path, &cfg).unwrap();
    Engine::new(
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
        cfg,
        None,
        Some(config_path),
    )
    .with_origins_dir(tmp.join("origins"))
    .with_state_dir(tmp.to_path_buf())
    .with_token_store_dir(tmp.join("tokens"))
    .with_env_overlay(EnvOverlay::default())
}

/// `configure` shows an admin the value in effect, and that it is a default.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_settings_row_shows_the_effective_value_and_where_it_comes_from() {
    for (app, key, value, source) in [
        (App::None, None, "instance", "default"),
        (App::Ready, None, "personal", "default"),
        (App::Refused, None, "personal", "default"),
        (App::Ready, Some("instance"), "instance", "config"),
        (App::None, Some("personal"), "personal", "config"),
    ] {
        let fx = case(app, key).await;
        let shown = fx.eng.configure(&ConfigureAction::Show).await.unwrap();
        let row = shown["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == "github.share_identity")
            .unwrap()
            .clone();
        assert_eq!(row["value"], value, "{app:?} by {key:?}: {row}");
        assert_eq!(row["source"], source, "{app:?} by {key:?}: {row}");
        // The MCP `configure` tool's snapshot says the same.
        let snapshot = fx.eng.configure_snapshot().await.unwrap();
        let row = snapshot["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == "github.share_identity")
            .unwrap()
            .clone();
        assert_eq!(
            row["value"], value,
            "configure tool, {app:?} by {key:?}: {row}"
        );
    }
    // Clearing the key goes back to what the App says, in the same answer.
    let fx = case(App::Ready, Some("instance")).await;
    let row = fx
        .eng
        .configure(&ConfigureAction::Unset {
            key: "github.share_identity".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(row["value"], "personal", "{row}");
    assert_eq!(row["source"], "default", "{row}");
    assert_eq!(fx.eng.share_identity_mode().as_str(), "personal");
}

// --- the JSON API with the gate in the path ------------------------------------

async fn rest(
    fx: &Case,
    session: Option<&(String, String)>,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = with_session(
        client().request(method, format!("http://{}{path}", fx.addr)),
        session,
    );
    if let Some(body) = body {
        request = request.json(&body);
    }
    let resp = request.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

const SHARE: &str = "/api/v1/domains/kb/sync/share";
const ADA_GH: u64 = 1001;

/// Gives `ada` the token `ghu_ada`: linked through the App with GitHub write,
/// or connected as a personal token with no App.
async fn give_ada_a_token(fx: &Case, app: App) {
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("ghu_ada".to_string(), (ADA_GH, "ada-gh".to_string()));
    fx.forge.collaborators.lock().unwrap().insert(
        "acme/kb".to_string(),
        vec![json!({"id": ADA_GH, "login": "ada-gh", "role_name": "write"})],
    );
    if app == App::None {
        crystalline_remote::TokenStore::file_fallback_for(
            &crystalline_remote::TokenIdentity::Personal("ada".to_string()),
            &fx.tmp.path().join("tokens"),
        )
        .unwrap()
        .save(&stored("ghu_ada"))
        .unwrap();
    } else {
        fx.eng.link_github("ada", grant("ghu_ada")).await.unwrap();
    }
    fx.eng.invalidate_github_access(Invalidate::All);
}

fn stored(token: &str) -> crystalline_remote::StoredToken {
    crystalline_remote::StoredToken {
        access_token: token.to_string(),
        host: "github.com".to_string(),
        user: format!("{token}-gh"),
        created_at: Utc::now(),
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
    }
}

fn change(fx: &Case, name: &str) {
    std::fs::write(
        fx.tmp.path().join("kb/notes").join(format!("{name}.md")),
        engram(name, name, "more content"),
    )
    .unwrap();
}

fn token_files(fx: &Case) -> Vec<String> {
    std::fs::read_dir(fx.tmp.path().join("tokens"))
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|name| name.contains("ada"))
                .collect()
        })
        .unwrap_or_default()
}

/// A new person under a removed account's name never writes on that account's
/// token: the token and its cache entry go with the account. Through the JSON
/// API, the old person's real share first so the cache is full.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_account_under_a_freed_name_never_writes_on_the_old_token() {
    for (app, key) in [(App::Ready, None), (App::None, Some("personal"))] {
        for role in [Role::Admin, Role::Editor] {
            let label = format!("{app:?} by {key:?}, new {role:?}");
            let fx = case(app, key).await;
            give_ada_a_token(&fx, app).await;
            let old = login(fx.addr, "ada").await.unwrap();
            let (status, answer) = rest(
                &fx,
                Some(&old),
                reqwest::Method::POST,
                SHARE,
                Some(json!({})),
            )
            .await;
            assert_eq!(status, 200, "{label}: the old person shares: {answer}");
            let before = writes(&fx.forge);
            assert!(
                !before.is_empty() && before.iter().all(|a| *a == bearer("ghu_ada")),
                "{label}: {before:?}"
            );

            let root = login(fx.addr, "root").await.unwrap();
            let (status, answer) = rest(
                &fx,
                Some(&root),
                reqwest::Method::DELETE,
                "/api/v1/users/ada",
                None,
            )
            .await;
            assert_eq!(status, 204, "{label}: {answer}");
            assert_eq!(token_files(&fx), Vec::<String>::new(), "{label}");
            if app == App::Ready {
                let revokes = fx.forge.revokes.lock().unwrap().clone();
                assert_eq!(revokes.len(), 1, "{label}: revoked at GitHub: {revokes:?}");
                assert_eq!(revokes[0].1, "ghu_ada", "{label}");
            }

            fx.auth
                .add_user("ada", "Another Ada", None, role, PASSWORD)
                .await
                .unwrap();
            change(&fx, "other");
            let new = login(fx.addr, "ada").await.unwrap();
            let (status, answer) = rest(
                &fx,
                Some(&new),
                reqwest::Method::POST,
                SHARE,
                Some(json!({})),
            )
            .await;
            assert_ne!(status, 200, "{label}: {answer}");
            assert_eq!(writes(&fx.forge), before, "{label}: nothing new went out");
        }
    }
}

/// An unlinked editor on a governed domain reads the report with `may_share`
/// false, so a dialog can show the link fix, and the refusal names the link in
/// words with no command line and no config key. Writes stay closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unlinked_person_reads_the_report_and_is_told_to_link() {
    let fx = case(App::Ready, None).await;
    let ada = login(fx.addr, "ada").await.unwrap();
    let (status, report) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::GET,
        "/api/v1/domains/kb/sync",
        None,
    )
    .await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["may_share"], false, "{report}");
    assert_eq!(
        report["connection"]["share_identity"], "personal",
        "{report}"
    );

    let (status, refusal) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::POST,
        SHARE,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 403, "{refusal}");
    let detail = refusal["detail"].as_str().unwrap();
    assert!(detail.contains("link"), "{detail}");
    assert!(detail.contains("profile"), "{detail}");
    assert!(
        !detail.contains("crystalline "),
        "no command line: {detail}"
    );
    assert!(!detail.contains("github."), "no config key: {detail}");
    assert!(!detail.contains("membership"), "{detail}");
    for (method, path, body) in [
        (
            reqwest::Method::GET,
            "/api/v1/domains/kb/sync/changes",
            None,
        ),
        (
            reqwest::Method::POST,
            "/api/v1/domains/kb/sync/proposals/1/withdraw",
            Some(json!({})),
        ),
        (
            reqwest::Method::POST,
            "/api/v1/domains/kb/sync/conflicts/nope/resolve",
            Some(json!({ "resolution": "theirs" })),
        ),
    ] {
        let (status, answer) = rest(&fx, Some(&ada), method, path, body).await;
        assert_eq!(status, 403, "{path}: {answer}");
    }
    assert!(writes(&fx.forge).is_empty());

    // Once linked with GitHub write, the same person may share.
    give_ada_a_token(&fx, App::Ready).await;
    let (status, report) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::GET,
        "/api/v1/domains/kb/sync",
        None,
    )
    .await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["may_share"], true, "{report}");

    // Nobody signed in, and no App: nothing opened.
    let (status, answer) = rest(
        &fx,
        None,
        reqwest::Method::GET,
        "/api/v1/domains/kb/sync",
        None,
    )
    .await;
    assert_eq!(status, 401, "{answer}");
    let plain = case(App::None, None).await;
    let ada = login(plain.addr, "ada").await.unwrap();
    let (status, answer) = rest(
        &plain,
        Some(&ada),
        reqwest::Method::GET,
        "/api/v1/domains/kb/sync",
        None,
    )
    .await;
    assert_eq!(status, 403, "no App changes nothing: {answer}");
}

/// A fresh engine with an App saved and the slot not read: the share loads it
/// before it decides, so a linked person's write never goes out through the App.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_share_loads_the_app_before_it_decides() {
    let fx = case(App::Ready, None).await;
    give_ada_a_token(&fx, App::Ready).await;
    fx.eng.reset_github_app();
    let answer = share_as_ada(&fx.eng).await.unwrap();
    assert_eq!(answer["outcome"], "proposed", "{answer}");
    let all = writes(&fx.forge);
    assert!(!all.is_empty());
    assert!(all.iter().all(|a| *a == bearer("ghu_ada")), "{all:?}");
}

/// What names a config key or the machine owner's connection is an admin's:
/// the two status routes leave it out for anybody else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_an_admin_reads_the_reason_and_the_owner_slot() {
    let fx = case(App::Ready, Some("personal")).await;
    give_ada_a_token(&fx, App::Ready).await;
    for (name, admin) in [("ada", false), ("root", true)] {
        let session = login(fx.addr, name).await.unwrap();
        for path in ["/api/v1/sync", "/api/v1/domains/kb/sync"] {
            let (status, answer) =
                rest(&fx, Some(&session), reqwest::Method::GET, path, None).await;
            assert_eq!(status, 200, "{name} {path}: {answer}");
            let connection = &answer["connection"];
            assert_eq!(connection["share_identity"], "personal", "{connection}");
            assert_eq!(connection["share_identity_default"], false, "{connection}");
            for key in ["share_identity_reason", "owner_identity"] {
                assert_eq!(
                    connection.get(key).is_some(),
                    admin,
                    "{name} {path} {key}: {connection}"
                );
            }
        }
    }
}

/// The row asks what the route asks: no App, explicit personal, an editor off
/// the write list.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listing_row_says_what_the_share_route_serves() {
    let fx = case(App::None, Some("personal")).await;
    let ada = login(fx.addr, "ada").await.unwrap();
    let row = |listing: &Value| {
        listing["domains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "kb")
            .unwrap()["may_share"]
            .clone()
    };
    let (_, listing) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::GET,
        "/api/v1/domains",
        None,
    )
    .await;
    assert_eq!(row(&listing), true, "{listing}");
    fx.auth
        .set_write_list("ada", &["elsewhere".to_string()])
        .await
        .unwrap();
    let (_, listing) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::GET,
        "/api/v1/domains",
        None,
    )
    .await;
    assert_eq!(row(&listing), false, "{listing}");
    let (status, answer) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::POST,
        SHARE,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 403, "{answer}");
}

/// A status read does not ask the store again for a token it just did not find,
/// and a token saved here is seen at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_status_read_remembers_a_missing_token_for_a_short_time() {
    let now = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let fx = case_with_clock(App::Ready, None, now.clone()).await;
    let owner = async |fx: &Case| {
        fx.eng
            .origin_status(None, false, false, &Scope::Unrestricted)
            .await
            .unwrap()["connection"]["owner_identity"]["connected"]
            .clone()
    };
    assert_eq!(owner(&fx).await, false);
    // Another process writes the token and nobody says so: not seen yet.
    let store = crystalline_remote::TokenStore::file_fallback_for(
        &crystalline_remote::TokenIdentity::Personal("owner".to_string()),
        &fx.tmp.path().join("tokens"),
    )
    .unwrap();
    store.save(&stored("ghu_owner")).unwrap();
    assert_eq!(owner(&fx).await, false, "remembered");
    // Being told ends it, as the CLI tells a daemon.
    fx.eng.forget_cached_credential(Some("owner")).unwrap();
    assert_eq!(owner(&fx).await, true, "told");

    // And time ends it.
    fx.eng.forget_cached_credential(Some("owner")).unwrap();
    store.delete().unwrap();
    assert_eq!(owner(&fx).await, false);
    store.save(&stored("ghu_owner")).unwrap();
    *now.lock().unwrap() += std::time::Duration::from_secs(120);
    assert_eq!(owner(&fx).await, true, "the memory ran out");

    // A person's link is seen at once.
    let status_for_ada = fx.eng.github_link_status("ada").await.unwrap();
    assert!(!status_for_ada.linked);
    assert!(share_as_ada(&fx.eng).await.is_err());
    give_ada_a_token(&fx, App::Ready).await;
    assert_eq!(share_as_ada(&fx.eng).await.unwrap()["outcome"], "proposed");
}

/// Amending and withdrawing in the default mode go out on the person's token.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn amend_and_withdraw_by_default_run_on_the_persons_token() {
    let fx = case(App::Ready, None).await;
    give_ada_a_token(&fx, App::Ready).await;
    let ada = login(fx.addr, "ada").await.unwrap();
    let (status, shared) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::POST,
        SHARE,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "{shared}");
    let number = shared["number"].as_u64().unwrap();
    change(&fx, "amended");
    let (status, amended) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::POST,
        SHARE,
        Some(json!({ "proposal": number })),
    )
    .await;
    assert_eq!(status, 200, "{amended}");
    assert_eq!(amended["outcome"], "updated", "{amended}");
    let (status, withdrawn) = rest(
        &fx,
        Some(&ada),
        reqwest::Method::POST,
        &format!("/api/v1/domains/kb/sync/proposals/{number}/withdraw"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "{withdrawn}");
    let all = writes(&fx.forge);
    assert!(all.len() > 3, "{all:?}");
    assert!(all.iter().all(|a| *a == bearer("ghu_ada")), "{all:?}");
}

// --- a token left under a name ---------------------------------------------------

fn token_file(root: &Path, account: &str) -> std::path::PathBuf {
    root.join("tokens")
        .join(format!("github-token-personal-{account}.json"))
}

fn leave_token(root: &Path, account: &str) {
    crystalline_remote::TokenStore::file_fallback_for(
        &crystalline_remote::TokenIdentity::Personal(account.to_string()),
        &root.join("tokens"),
    )
    .unwrap()
    .save(&stored("ghu_left"))
    .unwrap();
}

#[cfg(unix)]
fn set_writable(dir: &Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(
        dir,
        std::fs::Permissions::from_mode(if writable { 0o755 } else { 0o555 }),
    )
    .unwrap();
}

async fn create_account(fx: &Case, session: &(String, String), name: &str) -> (u16, Value) {
    rest(
        fx,
        Some(session),
        reqwest::Method::POST,
        "/api/v1/users",
        Some(json!({ "name": name, "password": PASSWORD, "role": "editor" })),
    )
    .await
}

/// A token an earlier removal left behind, cached or not, is gone before a new
/// account under that name can act.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creating_an_account_sweeps_a_token_left_under_its_name() {
    let fx = case(App::Ready, None).await;
    let root = login(fx.addr, "root").await.unwrap();
    leave_token(fx.tmp.path(), "newbie");
    // Cached, as a daemon that read it has it.
    let before = fx.eng.github_identity_status("newbie").await.unwrap();
    assert!(before.connected);
    let (status, answer) = create_account(&fx, &root, "newbie").await;
    assert_eq!(status, 201, "{answer}");
    assert!(!token_file(fx.tmp.path(), "newbie").exists());
    let after = fx.eng.github_identity_status("newbie").await.unwrap();
    assert!(!after.connected, "the cache went too");
}

/// A delete that failed at the removal is made good when the name is taken
/// again, and a delete that fails then refuses the new account, leaving none.
#[cfg(unix)]
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_delete_never_reaches_a_new_account() {
    let fx = case(App::None, Some("personal")).await;
    give_ada_a_token(&fx, App::None).await;
    let root = login(fx.addr, "root").await.unwrap();
    let tokens = fx.tmp.path().join("tokens");
    set_writable(&tokens, false);
    let (status, answer) = rest(
        &fx,
        Some(&root),
        reqwest::Method::DELETE,
        "/api/v1/users/ada",
        None,
    )
    .await;
    assert_eq!(status, 204, "the removal stands: {answer}");
    assert!(
        token_file(fx.tmp.path(), "ada").exists(),
        "the delete failed"
    );

    let (status, answer) = create_account(&fx, &root, "ada").await;
    set_writable(&tokens, true);
    assert_eq!(status, 500, "{answer}");
    assert!(
        fx.auth.user("ada").await.unwrap().is_none(),
        "no account stands on a token it could inherit"
    );

    let (status, answer) = create_account(&fx, &root, "ada").await;
    assert_eq!(status, 201, "{answer}");
    assert!(!token_file(fx.tmp.path(), "ada").exists());
}

/// A device flow still running when its account goes never saves a token: the
/// removal stops it, and a flow nobody told refuses when it lands.
#[tokio::test]
async fn a_late_device_flow_saves_nothing_for_a_removed_account() {
    use crate::support::{device_flow_start, fake_auth};
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;

    // Nobody told the engine: the account went in another process.
    let stub = fake_auth(
        Ok(device_flow_start()),
        Ok("gho_late".to_string()),
        Ok("late-gh".to_string()),
    );
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    auth.add_user("ada", "Ada", None, Role::Editor, PASSWORD)
        .await
        .unwrap();
    let eng = base_engine(tmp.path(), &url)
        .await
        .with_connect_auth(stub.clone());
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    eng.start_github_identity_device_flow("ada", false)
        .await
        .unwrap();
    auth.remove_user("ada").await.unwrap();
    stub.run_gate.notify_one();
    let mut error = None;
    for _ in 0..400 {
        error = eng.github_identity_status("ada").await.unwrap().error;
        if error.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        error
            .as_deref()
            .unwrap_or_default()
            .contains("no longer exists"),
        "{error:?}"
    );
    assert!(!token_file(tmp.path(), "ada").exists());

    // Told: the flow stops, and an account made meanwhile gets nothing.
    let tmp = tempfile::tempdir().unwrap();
    let stub = fake_auth(
        Ok(device_flow_start()),
        Ok("gho_late".to_string()),
        Ok("late-gh".to_string()),
    );
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    auth.add_user("ada", "Ada", None, Role::Editor, PASSWORD)
        .await
        .unwrap();
    let eng = base_engine(tmp.path(), &url)
        .await
        .with_connect_auth(stub.clone());
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    eng.start_github_identity_device_flow("ada", false)
        .await
        .unwrap();
    for _ in 0..400 {
        if stub.run_was_entered() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    auth.remove_user("ada").await.unwrap();
    eng.forget_removed_account("ada").await;
    auth.add_user("ada", "Another", None, Role::Editor, PASSWORD)
        .await
        .unwrap();
    stub.run_gate.notify_one();
    for _ in 0..100 {
        if stub.run_was_abandoned() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(stub.run_was_abandoned());
    assert!(!token_file(tmp.path(), "ada").exists());
}

/// First-run setup and a trusted proxy header make accounts too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_other_ways_to_make_an_account_sweep_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(url.clone()),
            ..GitHubConfig::default()
        }),
        auth: Some(AuthConfig {
            oauth: Some(false),
            trusted_header: Some("x-remote-user".to_string()),
            ..AuthConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let config_path = tmp.path().join("config.yaml");
    save_yaml(&config_path, &cfg).unwrap();
    let eng = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg,
            None,
            Some(config_path),
        )
        .with_origins_dir(tmp.path().join("origins"))
        .with_state_dir(tmp.path().to_path_buf())
        .with_token_store_dir(tmp.path().join("tokens"))
        .with_env_overlay(EnvOverlay::default()),
    );
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    let router = http_router(
        eng.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        None,
    )
    .unwrap();
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

    leave_token(tmp.path(), "first");
    let setup = client()
        .post(format!("http://{addr}/api/v1/auth/setup"))
        .json(&json!({ "name": "first", "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert!(setup.status().is_success(), "{setup:?}");
    assert!(!token_file(tmp.path(), "first").exists(), "setup swept");

    leave_token(tmp.path(), "proxied");
    let me = client()
        .get(format!("http://{addr}/api/v1/auth/me"))
        .header("x-remote-user", "proxied")
        .send()
        .await
        .unwrap();
    assert!(me.status().is_success(), "{me:?}");
    assert!(auth.user("proxied").await.unwrap().is_some());
    assert!(!token_file(tmp.path(), "proxied").exists(), "header swept");
    // Only the first request makes the account: a token connected afterwards
    // is that person's own and stays.
    leave_token(tmp.path(), "proxied");
    client()
        .get(format!("http://{addr}/api/v1/auth/me"))
        .header("x-remote-user", "proxied")
        .send()
        .await
        .unwrap();
    assert!(token_file(tmp.path(), "proxied").exists());
}

// --- a scheduled run ------------------------------------------------------------

/// One run on `kb` with no account, as a scheduled run on a server is: lease,
/// a change made during the run, the share.
async fn run_shares(fx: &Case) -> Result<Value, EngineError> {
    use crystalline_core::automation::Loop;
    let take = fx
        .eng
        .automation_lease_take(Loop::Evolve, "kb", "cron", &Scope::Unrestricted)
        .await
        .unwrap();
    let token = take["token"].as_str().unwrap().to_string();
    change(fx, "run");
    fx.eng
        .automation_share(Loop::Evolve, "kb", &token, "The run", &Scope::Unrestricted)
        .await
}

fn only_the_runs_file_went_out(answer: &Value, label: &str) {
    assert_eq!(answer["outcome"], "shared", "{label}: {answer}");
    assert_eq!(answer["attributed"], false, "{label}: {answer}");
}

/// Who a scheduled run shares as, row by row (ruling S7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_run_shares_as_the_machine() {
    // Key absent, App ready: through the App, with the bot as the author.
    let fx = case(App::Ready, None).await;
    let answer = run_shares(&fx).await.unwrap();
    only_the_runs_file_went_out(&answer, "absent, ready");
    let all = writes(&fx.forge);
    assert!(!all.is_empty());
    assert!(all.iter().all(|a| *a == bearer("ghs_test")), "{all:?}");
    let commits = fx.forge.commits();
    assert_eq!(commits.len(), 1, "{commits:?}");
    let keys: Vec<&String> = commits[0].as_object().unwrap().keys().collect();
    assert!(
        !keys.iter().any(|k| *k == "author" || *k == "committer"),
        "the commit names nobody but the bot: {keys:?}"
    );
    let state = crystalline_remote::state::OriginState::load(&fx.tmp.path().join("origins/kb"))
        .unwrap()
        .unwrap();
    assert_eq!(
        state.proposals[0].author_login.as_deref(),
        Some("crystalline-acme[bot]")
    );

    // Explicit `instance`: the App, as ever.
    let fx = case(App::Ready, Some("instance")).await;
    only_the_runs_file_went_out(&run_shares(&fx).await.unwrap(), "instance, ready");
    assert!(writes(&fx.forge).iter().all(|a| *a == bearer("ghs_test")));

    // Explicit `personal`, App ready: the linked owner identity or a refusal.
    let fx = case(App::Ready, Some("personal")).await;
    let refused = run_shares(&fx).await.unwrap_err().to_string();
    assert!(refused.contains("through its GitHub App"), "{refused}");
    assert!(writes(&fx.forge).is_empty());
    let fx = case(App::Ready, Some("personal")).await;
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("ghu_owner".to_string(), (2002, "owner-gh".to_string()));
    leave_owner_token(fx.tmp.path());
    only_the_runs_file_went_out(&run_shares(&fx).await.unwrap(), "personal, owner linked");
    let all = writes(&fx.forge);
    assert!(all.iter().all(|a| *a == bearer("ghu_owner")), "{all:?}");

    // A refused App with no key: the App's own reason.
    let fx = case(App::Refused, None).await;
    let refused = run_shares(&fx).await.unwrap_err();
    assert!(
        matches!(&refused, EngineError::Remote(RemoteError::AppCredential(_))),
        "{refused:?}"
    );
    assert!(writes(&fx.forge).is_empty());

    // No App: the instance credential, and explicit `personal` the owner's.
    let fx = case(App::None, None).await;
    only_the_runs_file_went_out(&run_shares(&fx).await.unwrap(), "absent, none");
    assert!(
        writes(&fx.forge)
            .iter()
            .all(|a| *a == bearer(INSTANCE_TOKEN))
    );
    let fx = case(App::None, Some("personal")).await;
    let refused = run_shares(&fx).await.unwrap_err().to_string();
    assert!(refused.contains("connect github --personal"), "{refused}");
    assert!(writes(&fx.forge).is_empty());
}

fn leave_owner_token(root: &Path) {
    crystalline_remote::TokenStore::file_fallback_for(
        &crystalline_remote::TokenIdentity::Personal("owner".to_string()),
        &root.join("tokens"),
    )
    .unwrap()
    .save(&stored("ghu_owner"))
    .unwrap();
}

/// A person at the machine stays a person: every other way to share as the
/// owner is refused with no owner identity, however a run would go.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_at_the_machine_never_takes_the_runs_way_out() {
    let fx = case(App::Ready, None).await;
    let refused = fx
        .eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("through its GitHub App"), "{refused}");
    assert!(writes(&fx.forge).is_empty());
    for source in [
        include_str!("../../src/control.rs"),
        include_str!("../../src/client.rs"),
        include_str!("../../src/remote_ctl.rs"),
        include_str!("../../src/mcp.rs"),
    ] {
        assert!(!source.contains("ShareActor::Machine"));
    }
}

/// An admin removes `name` and makes it again, over the JSON API.
async fn admin_replaces(fx: &Case, name: &str) {
    let root = login(fx.addr, "root").await.unwrap();
    let gone = with_session(
        client().delete(format!("http://{}/api/v1/users/{name}", fx.addr)),
        Some(&root),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(gone.status(), 204);
    let made = with_session(
        client().post(format!("http://{}/api/v1/users", fx.addr)),
        Some(&root),
    )
    .json(&json!({ "name": name, "password": PASSWORD, "role": "editor" }))
    .send()
    .await
    .unwrap();
    let status = made.status();
    assert_eq!(status, 201, "{}", made.text().await.unwrap());
}

/// Waits until the flow's task is stopped or its token is on file.
async fn until_stopped_or_saved(stub: &crate::support::StubConnectAuth, fx: &Case) {
    for _ in 0..200 {
        if stub.run_was_abandoned() || token_file(fx.tmp.path(), "ada").exists() {
            // A moment more, so a row written just before the token shows too.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// The old person starts a link and disconnects, which drops the record but
/// used to leave the task running. The admin replaces the name, and the old
/// person then confirms the code. Nothing of that may reach the new account.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_flow_cleared_by_a_disconnect_never_reaches_a_new_account() {
    use crate::support::{device_flow_start, fake_auth};
    let stub = fake_auth(
        Ok(device_flow_start()),
        Ok("gho_old_person".to_string()),
        Err(RemoteError::NotConnected),
    );
    let fx = case_with_auth(
        App::Ready,
        None,
        Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        Some(stub.clone()),
    )
    .await;
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("gho_old_person".to_string(), (4242, "old-gh".to_string()));
    fx.forge.collaborators.lock().unwrap().insert(
        "acme/kb".to_string(),
        vec![json!({"id": 4242, "login": "old-gh", "role_name": "write"})],
    );

    fx.eng.start_github_link_device_flow("ada").await.unwrap();
    for _ in 0..400 {
        if stub.run_was_entered() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    fx.eng.disconnect_github_identity("ada").await.unwrap();
    admin_replaces(&fx, "ada").await;
    stub.run_gate.notify_one();
    until_stopped_or_saved(&stub, &fx).await;
    fx.eng.invalidate_github_access(Invalidate::All);

    // The new person shares, and nothing goes out on the old token.
    let session = login(fx.addr, "ada").await.unwrap();
    let answer = with_session(
        client().post(format!("http://{}/api/v1/domains/kb/sync/share", fx.addr)),
        Some(&session),
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap();
    let status = answer.status();
    let body = answer.text().await.unwrap();
    assert!(
        !writes(&fx.forge)
            .iter()
            .any(|auth| auth.as_deref() == Some("Bearer gho_old_person")),
        "{:?}",
        writes(&fx.forge)
    );
    assert_ne!(status, 200, "{body}");
    let status = fx.eng.github_link_status("ada").await.unwrap();
    assert!(!status.linked, "{:?}", status.login);
    assert!(!token_file(fx.tmp.path(), "ada").exists());
}

/// The same with no App, on the plain device flow.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_flow_cleared_by_a_disconnect_never_reaches_a_new_account() {
    use crate::support::{device_flow_start, fake_auth};
    let stub = fake_auth(
        Ok(device_flow_start()),
        Ok("gho_old_person".to_string()),
        Ok("old-person-gh".to_string()),
    );
    let fx = case_with_auth(
        App::None,
        Some("personal"),
        Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        Some(stub.clone()),
    )
    .await;
    fx.eng
        .start_github_identity_device_flow("ada", false)
        .await
        .unwrap();
    for _ in 0..400 {
        if stub.run_was_entered() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    fx.eng.disconnect_github_identity("ada").await.unwrap();
    admin_replaces(&fx, "ada").await;
    stub.run_gate.notify_one();
    until_stopped_or_saved(&stub, &fx).await;

    let status = fx.eng.github_identity_status("ada").await.unwrap();
    assert!(!status.connected, "{:?}", status.login);
    assert!(!token_file(fx.tmp.path(), "ada").exists());
}

/// A browser link that began for the old account finishes after the name was
/// made again: no row and no token, though the session check is not what stops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_that_began_for_the_old_account_is_refused() {
    let fx = case(App::Ready, None).await;
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("gho_old_person".to_string(), (4242, "old-gh".to_string()));
    let born = fx.eng.account_born("ada").await.unwrap();
    admin_replaces(&fx, "ada").await;
    let refused = fx
        .eng
        .link_github_begun("ada", born, grant("gho_old_person"))
        .await;
    assert!(refused.is_err(), "{refused:?}");
    let status = fx.eng.github_link_status("ada").await.unwrap();
    assert!(!status.linked);
    assert!(!token_file(fx.tmp.path(), "ada").exists());
}

/// A renewal holds no check while GitHub answers. The name is made again in that
/// time: the renewed pair is not saved for the new account, and the share that
/// asked for it does not go out on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_renewal_in_flight_saves_nothing_for_a_new_account() {
    let fx = case(App::Ready, None).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *fx.forge.refresh_gate.lock().unwrap() = Some(gate.clone());
    fx.forge.refreshable.lock().unwrap().insert(
        "ghr_old".to_string(),
        (
            "ghu_renewed".to_string(),
            "ghr_renewed".to_string(),
            (ADA_GH, "ada-gh".to_string()),
        ),
    );
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("ghu_old".to_string(), (ADA_GH, "ada-gh".to_string()));
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("ghu_renewed".to_string(), (ADA_GH, "ada-gh".to_string()));
    fx.forge.collaborators.lock().unwrap().insert(
        "acme/kb".to_string(),
        vec![json!({"id": ADA_GH, "login": "ada-gh", "role_name": "write"})],
    );
    let mut expiring = grant("ghu_old");
    expiring.refresh_token = Some("ghr_old".to_string());
    expiring.expires_at = Some(Utc::now() + chrono::Duration::minutes(1));
    fx.eng.link_github("ada", expiring).await.unwrap();

    let eng = fx.eng.clone();
    let share = tokio::spawn(async move { share_as_ada(&eng).await });
    for _ in 0..1000 {
        if !fx.forge.refreshes.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    admin_replaces(&fx, "ada").await;
    gate.add_permits(4);
    let shared = share.await.unwrap();
    assert!(shared.is_err(), "{shared:?}");
    assert!(!token_file(fx.tmp.path(), "ada").exists());
    assert!(
        !writes(&fx.forge)
            .iter()
            .any(|auth| auth.as_deref() == Some("Bearer ghu_renewed")),
        "{:?}",
        writes(&fx.forge)
    );
}

/// The name is cleared before the account exists, so while a token left under it
/// is still being dealt with there is no account for a second request to act as.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_account_stands_while_its_name_is_being_cleared() {
    let fx = case(App::Ready, None).await;
    leave_token(fx.tmp.path(), "newbie");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *fx.forge.revoke_gate.lock().unwrap() = Some(gate.clone());

    let root = login(fx.addr, "root").await.unwrap();
    let url = format!("http://{}/api/v1/users", fx.addr);
    let making = tokio::spawn(async move {
        with_session(client().post(url), Some(&root))
            .json(&json!({ "name": "newbie", "password": PASSWORD, "role": "editor" }))
            .send()
            .await
            .unwrap()
            .status()
    });
    for _ in 0..1000 {
        if !fx.forge.revokes.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(!fx.forge.revokes.lock().unwrap().is_empty());
    assert!(
        fx.auth.user("newbie").await.unwrap().is_none(),
        "the account stands while the old token is still there"
    );
    gate.add_permits(4);
    assert_eq!(making.await.unwrap(), 201);
    assert!(fx.auth.user("newbie").await.unwrap().is_some());
    assert!(!token_file(fx.tmp.path(), "newbie").exists());
}

/// The step before an account is made deletes what is saved under the name, so it
/// never runs for a name that holds an account, or for the machine owner's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clearing_a_name_never_touches_a_live_account_or_the_machine_owner() {
    let fx = case(App::None, Some("personal")).await;
    leave_token(fx.tmp.path(), "ada");
    leave_token(fx.tmp.path(), "owner");
    let root = login(fx.addr, "root").await.unwrap();
    for name in ["ada", "owner"] {
        let answer = with_session(
            client().post(format!("http://{}/api/v1/users", fx.addr)),
            Some(&root),
        )
        .json(&json!({ "name": name, "password": PASSWORD, "role": "editor" }))
        .send()
        .await
        .unwrap();
        assert!(!answer.status().is_success(), "{name}");
        assert!(token_file(fx.tmp.path(), name).exists(), "{name}'s token");
    }
}

/// The proposal records on `kb`.
async fn proposals(fx: &Case) -> Vec<Value> {
    let state = crystalline_remote::state::OriginState::load(&fx.tmp.path().join("origins/kb"))
        .unwrap()
        .unwrap();
    state
        .proposals
        .iter()
        .map(|p| json!({ "number": p.number, "author_login": p.author_login }))
        .collect()
}

/// Links `ada` so her shares go out on `ghu_ada`, and a run's on the App.
async fn run_and_person(fx: &Case) {
    give_ada_a_token(fx, App::Ready).await;
}

/// Where a forge serves no stacks, a run and a person reach one proposal. Neither
/// order changes whose it is, and each commit goes out on who made it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_stacks_the_author_never_flips_and_each_commit_is_its_makers() {
    // The person first, then a run.
    let fx = case(App::Ready, None).await;
    run_and_person(&fx).await;
    let first = share_as_ada(&fx.eng).await.unwrap();
    assert_eq!(first["outcome"], "proposed", "{first}");
    run_shares(&fx).await.unwrap();
    let records = proposals(&fx).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["author_login"], "ada-gh", "{records:?}");
    let commits: Vec<Option<String>> = fx
        .forge
        .hits()
        .into_iter()
        .filter(|h| h.method == "POST" && h.path.ends_with("/git/commits"))
        .map(|h| h.auth)
        .collect();
    assert_eq!(commits, vec![bearer("ghu_ada"), bearer("ghs_test")]);

    // A run first, then the person.
    let fx = case(App::Ready, None).await;
    run_and_person(&fx).await;
    run_shares(&fx).await.unwrap();
    change(&fx, "person");
    share_as_ada(&fx.eng).await.unwrap();
    let records = proposals(&fx).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0]["author_login"], "crystalline-acme[bot]",
        "{records:?}"
    );
    let commits: Vec<Option<String>> = fx
        .forge
        .hits()
        .into_iter()
        .filter(|h| h.method == "POST" && h.path.ends_with("/git/commits"))
        .map(|h| h.auth)
        .collect();
    assert_eq!(commits, vec![bearer("ghs_test"), bearer("ghu_ada")]);
}

/// The default on github.com: the forge serves stacks, so a run and a person each
/// have a proposal of their own, as layers of one chain, in either order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_stacks_a_run_and_a_person_each_keep_a_proposal_of_their_own() {
    for person_first in [true, false] {
        let fx = case(App::Ready, None).await;
        fx.forge.repos["acme/kb"].enable_stacks();
        run_and_person(&fx).await;
        if person_first {
            share_as_ada(&fx.eng).await.unwrap();
            run_shares(&fx).await.unwrap();
        } else {
            run_shares(&fx).await.unwrap();
            change(&fx, "person");
            share_as_ada(&fx.eng).await.unwrap();
        }
        let records = proposals(&fx).await;
        assert_eq!(records.len(), 2, "person first {person_first}: {records:?}");
        let authors: Vec<&str> = records
            .iter()
            .map(|r| r["author_login"].as_str().unwrap_or_default())
            .collect();
        let wanted = if person_first {
            ["ada-gh", "crystalline-acme[bot]"]
        } else {
            ["crystalline-acme[bot]", "ada-gh"]
        };
        assert_eq!(authors, wanted, "person first {person_first}: {records:?}");
    }
}

/// Says that `text`, read by somebody who is not an admin, holds no command line,
/// no setting and no internal host.
fn plain(text: &str, addr_hint: &str) {
    assert!(!text.contains("crystalline "), "{text}");
    assert!(!text.contains("github."), "{text}");
    assert!(!text.contains("CRYSTALLINE_"), "{text}");
    assert!(!text.contains("service.public_url"), "{text}");
    assert!(!text.contains(addr_hint), "{text}");
}

/// An editor on an instance whose App cannot be used: the status, the link and the
/// share all answer in plain words. An admin still reads the App's own sentence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_admin_reads_no_command_line_or_setting_on_any_path() {
    let fx = case(App::Refused, None).await;
    let host = "127.0.0.1";
    let ada = login(fx.addr, "ada").await.unwrap();
    for path in ["/api/v1/sync", "/api/v1/domains/kb/sync"] {
        let answer: Value = with_session(
            client().get(format!("http://{}{path}", fx.addr)),
            Some(&ada),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert!(answer["connection"]["app"].is_null(), "{path}: {answer}");
    }
    // The same status reached another way, by MCP or a remote runner.
    let status = fx
        .eng
        .origin_status(
            None,
            false,
            false,
            &Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert!(status["connection"]["app"].is_null(), "{status}");

    let start = with_session(
        client().post(format!("http://{}/api/v1/me/github-link/start", fx.addr)),
        Some(&ada),
    )
    .send()
    .await
    .unwrap();
    let start: Value = start.json().await.unwrap();
    assert_eq!(start["reason"], "app_unusable", "{start}");
    plain(start["detail"].as_str().unwrap(), host);

    // An unlinked editor is told about the App, not told to link.
    let share = with_session(
        client().post(format!("http://{}{SHARE}", fx.addr)),
        Some(&ada),
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap();
    let share: Value = share.json().await.unwrap();
    let detail = share["detail"].as_str().unwrap();
    assert!(!detail.contains("not linked"), "{detail}");
    assert!(detail.contains("Ask an admin"), "{detail}");
    plain(detail, host);

    // The share path itself, for an account past the gate.
    let engine_refusal = share_as_ada(&fx.eng).await.unwrap_err().to_string();
    plain(&engine_refusal, host);

    // An admin reads what to do.
    let root = login(fx.addr, "root").await.unwrap();
    let answer: Value = with_session(
        client().get(format!("http://{}/api/v1/domains/kb/sync", fx.addr)),
        Some(&root),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(answer["connection"]["app"]["error"].is_string(), "{answer}");
}

/// A run by an agent account stays that account (S7 speaks of runs with no
/// account). One with no GitHub identity hears it when it takes the lease, before
/// the run does its work. One with an identity hears nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unlinked_agent_account_hears_it_cannot_share_at_the_take() {
    use crystalline_core::automation::Loop;
    let fx = case(App::Ready, None).await;
    let unlinked = Scope::User {
        account: "root".to_string(),
        admin: true,
    };
    let take = fx
        .eng
        .automation_lease_take(Loop::Evolve, "kb", "cron", &unlinked)
        .await
        .unwrap();
    let sentence = take["share_refused"].as_str().unwrap();
    plain(sentence, "127.0.0.1");
    fx.eng
        .automation_lease_release(
            Loop::Evolve,
            "kb",
            take["token"].as_str().unwrap(),
            &unlinked,
        )
        .await
        .ok();

    fx.eng.link_github("root", grant("ghu_root")).await.ok();
    leave_token(fx.tmp.path(), "root");
    fx.eng.forget_cached_credential(Some("root")).unwrap();
    let take = fx
        .eng
        .automation_lease_take(Loop::Reflect, "kb", "cron", &unlinked)
        .await;
    if let Ok(take) = take {
        assert!(take["share_refused"].is_null(), "{take}");
    }
}

/// Bare `configure` over MCP is the last door to the App's own sentence: a person
/// who is not an admin reads the plain one, with no command line, setting or host.
/// An admin keeps the full text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bare_configure_gives_a_non_admin_the_plain_app_sentence() {
    use super::github_governed::Mcp;
    let fx = case_full(
        App::Refused,
        None,
        Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        None,
        Some(true),
        true,
    )
    .await;
    let editor = fx.auth.issue_mcp_token("ada", "t").await.unwrap().token;
    let mcp = Mcp::open(fx.addr, Some(editor)).await.unwrap();
    let answer = mcp.call("configure", json!({})).await.unwrap();
    assert!(answer.contains("cannot be used right now"), "{answer}");
    assert!(!answer.contains("save the App again"), "{answer}");
    assert!(!answer.contains("saved App belongs"), "{answer}");

    let admin = fx.auth.issue_mcp_token("root", "t").await.unwrap().token;
    let mcp = Mcp::open(fx.addr, Some(admin)).await.unwrap();
    let answer = mcp.call("configure", json!({})).await.unwrap();
    assert!(answer.contains("save the App again"), "{answer}");
}

/// A trusted-header instance whose `github.agent_identity` is `bot`.
async fn agent_identity_instance() -> (tempfile::TempDir, std::net::SocketAddr, Arc<AuthStore>) {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            agent_identity: Some("bot".to_string()),
            ..GitHubConfig::default()
        }),
        auth: Some(AuthConfig {
            oauth: Some(false),
            trusted_header: Some("x-remote-user".to_string()),
            ..AuthConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let config_path = tmp.path().join("config.yaml");
    save_yaml(&config_path, &cfg).unwrap();
    let eng = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg,
            None,
            Some(config_path),
        )
        .with_origins_dir(tmp.path().join("origins"))
        .with_state_dir(tmp.path().to_path_buf())
        .with_token_store_dir(tmp.path().join("tokens"))
        .with_env_overlay(EnvOverlay::default()),
    );
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    let router = http_router(
        eng.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        None,
    )
    .unwrap();
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
    (tmp, addr, auth)
}

/// The step before an account deletes what is saved under the name. A stranger's
/// first sign-in under the name `github.agent_identity` holds must not delete the
/// bot's token: only an admin who adds that account may. It is refused instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_sign_in_under_the_agent_identity_leaves_its_token_alone() {
    let (tmp, addr, auth) = agent_identity_instance().await;
    leave_token(tmp.path(), "bot");
    let me = client()
        .get(format!("http://{addr}/api/v1/auth/me"))
        .header("x-remote-user", "bot")
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 409);
    assert!(token_file(tmp.path(), "bot").exists(), "the bot's token");
    assert!(auth.user("bot").await.unwrap().is_none());
}

/// An instance with `auth.mcp` on, and an MCP session for a real editor and for a
/// real admin. Without `auth.mcp` the token is not read and the caller is the open
/// tier.
async fn editor_and_admin(
    app: App,
    public_url: bool,
) -> (
    Case,
    super::github_governed::Mcp,
    super::github_governed::Mcp,
) {
    use super::github_governed::Mcp;
    let fx = case_full(
        app,
        None,
        Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        None,
        Some(true),
        public_url,
    )
    .await;
    let editor = fx.auth.issue_mcp_token("ada", "t").await.unwrap().token;
    let admin = fx.auth.issue_mcp_token("root", "t").await.unwrap().token;
    let editor = Mcp::open(fx.addr, Some(editor)).await.unwrap();
    let admin = Mcp::open(fx.addr, Some(admin)).await.unwrap();
    (fx, editor, admin)
}

/// What a tool call answered, a refusal or not.
fn said(answer: Result<String, String>) -> String {
    answer.unwrap_or_else(|refusal| refusal)
}

fn no_admin_detail(text: &str) {
    assert!(!text.contains("crystalline "), "{text}");
    assert!(!text.contains("github.api_url"), "{text}");
    assert!(!text.contains("service.public_url"), "{text}");
    assert!(!text.contains("127.0.0.1"), "{text}");
    assert!(text.contains("Ask an admin"), "{text}");
}

/// `configure` with `connect: "github"` is open to every account, so a real editor
/// reads plain words for each way the link cannot start. An admin keeps the fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_github_link_asked_by_an_editor_reads_plain_words() {
    let connect = json!({ "connect": "github" });

    let (_fx, editor, admin) = editor_and_admin(App::Locked, true).await;
    no_admin_detail(&said(editor.call("configure", connect.clone()).await));
    let full = said(admin.call("configure", connect.clone()).await);
    assert!(full.contains("crystalline github app remove"), "{full}");

    let (_fx, editor, admin) = editor_and_admin(App::Refused, true).await;
    no_admin_detail(&said(editor.call("configure", connect.clone()).await));
    let full = said(admin.call("configure", connect.clone()).await);
    assert!(full.contains("github.api_url"), "{full}");

    let (_fx, editor, admin) = editor_and_admin(App::Ready, false).await;
    no_admin_detail(&said(editor.call("configure", connect.clone()).await));
    let full = said(admin.call("configure", connect.clone()).await);
    assert!(full.contains("service.public_url"), "{full}");
}

/// The same sentences on the verbs an editor runs every day: a share refuses
/// with them, and `update_domain`, which needs the admin role, refuses an
/// editor in plain words before it reaches the App. An admin's pull keeps the
/// fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_verbs_an_editor_runs_read_plain_words_about_the_app() {
    for app in [App::Refused, App::Locked] {
        let (_fx, editor, admin) = editor_and_admin(app, true).await;
        no_admin_detail(&said(
            editor
                .call("update_domain", json!({ "domain": "kb" }))
                .await,
        ));
        // The gate answers first here, in its own plain words.
        let share = said(
            editor
                .call("share_changes", json!({ "domain": "kb" }))
                .await,
        );
        assert!(!share.contains("crystalline "), "{share}");
        assert!(!share.contains("github.api_url"), "{share}");
        let full = said(admin.call("update_domain", json!({ "domain": "kb" })).await);
        assert!(
            full.contains("github.api_url") || full.contains("crystalline github app remove"),
            "{full}"
        );
    }
}

/// With GitHub off, an editor cannot turn it on, so the refusal does not teach the
/// key. An admin can, and reads it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn github_off_tells_an_editor_to_ask_an_admin() {
    let (fx, editor, admin) = editor_and_admin(App::None, true).await;
    fx.eng
        .configure(&ConfigureAction::Set {
            key: "github.enabled".to_string(),
            value: "false".to_string(),
        })
        .await
        .unwrap();
    no_admin_detail(&said(
        editor
            .call("update_domain", json!({ "domain": "kb" }))
            .await,
    ));
    let bare = said(editor.call("configure", json!({})).await);
    assert!(!bare.contains("set github.enabled true"), "{bare}");
    let full = said(admin.call("update_domain", json!({ "domain": "kb" })).await);
    assert!(full.contains("github.enabled"), "{full}");
}

/// The refusal under the agent identity's name protects a token. With no token
/// saved there is nothing to protect, so the person signs in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_sign_in_under_the_agent_identity_with_no_token_is_let_in() {
    let (tmp, addr, auth) = agent_identity_instance().await;
    let me = client()
        .get(format!("http://{addr}/api/v1/auth/me"))
        .header("x-remote-user", "bot")
        .send()
        .await
        .unwrap();
    assert!(me.status().is_success(), "{me:?}");
    assert!(auth.user("bot").await.unwrap().is_some());
    drop(tmp);
}
