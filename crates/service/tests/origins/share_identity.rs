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
use crystalline_service::engine::{ConfigureAction, EngineError, ShareActor};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EnvOverlay, Scope};
use serde_json::Value;
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
    .with_env_overlay(overlay);
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
    Case {
        forge,
        eng,
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
            assert_eq!(
                can_share(&fx, "ada").await,
                personal,
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
