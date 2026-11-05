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
            public_url: Some("https://kb.example.com".into()),
            ..Default::default()
        }),
        auth: Some(AuthConfig {
            oauth: Some(false),
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
        eng = eng.with_app_store_lookup(Arc::new(move || {
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
    let eng = Arc::new(eng);
    let domain_root = root.join("kb");
    connect(&eng, "acme/kb", "kb", &domain_root).await;
    std::fs::create_dir_all(domain_root.join("notes")).unwrap();
    std::fs::write(
        domain_root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();
    if app == App::Refused {
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
                        matches!(&answer, Err(EngineError::Remote(RemoteError::AppCredential(d))) if d.contains("github.com")),
                        "{label}"
                    );
                }
                (App::Ready, true, _) => {
                    let text = answer.as_ref().expect_err(&label).to_string();
                    assert!(text.contains("through its GitHub App"), "{label}");
                }
                (_, true, _) => {
                    let text = answer.as_ref().expect_err(&label).to_string();
                    assert!(text.contains("Connect yours in Fluid"), "{label}");
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
        (ShareActor::HttpAgent, "set github.agent_identity"),
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
