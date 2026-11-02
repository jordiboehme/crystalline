//! Access on a domain connected to GitHub through the GitHub App, end to end
//! through the engine: the governor asks the forge for the repository's
//! collaborators, an MCP token agent follows its account's GitHub role, and
//! the MCP open tier acts as `github.agent_identity` on a governed domain
//! (and never writes there without one), while it keeps its carve-out on a
//! domain GitHub does not govern.
//!
//! The forge is the one `origins::github_app` serves, with its collaborator
//! listing and repository visibility routes.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{GitHubConfig, GlobalConfig, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::engine::{WriteBlock, as_mcp_call};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::scope::{DomainRight, ReflectionCaller};
use crystalline_service::{DomainAccess, Engine, EnvOverlay, Scope};
use serde_json::json;
use tokio::sync::Mutex;

use super::github_app::{Forge, app_env, connect, repo_with_manifest, spawn_forge};

const AGENT_ID: u64 = 909;
const ADA_ID: u64 = 1001;

/// An engine with an App pointed at the forge, `acme/kb` connected as the
/// domain `kb`, the domain `plain` without an origin, and
/// the accounts `agent` (instance viewer) and `ada` (instance editor) linked
/// to GitHub. `agent_identity` is `github.agent_identity`; `auth.mcp` is off,
/// so an anonymous MCP caller is the open tier.
async fn setup(
    tmp: &Path,
    agent_identity: Option<&str>,
) -> (Arc<Forge>, Arc<Engine>, Arc<AuthStore>) {
    let (forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let store = TursoStore::open_in_memory().await.unwrap();
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(url.clone()),
            agent_identity: agent_identity.map(str::to_string),
            ..GitHubConfig::default()
        }),
        service: Some(ServiceConfig {
            public_url: Some("https://kb.example.com".into()),
            ..Default::default()
        }),
        ..GlobalConfig::default()
    };
    let overlay = EnvOverlay::from_vars(
        app_env()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let eng = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(store)),
            cfg,
            None,
            Some(tmp.join("config.yaml")),
        )
        .with_origins_dir(tmp.join("origins"))
        .with_state_dir(tmp.to_path_buf())
        .with_token_store_dir(tmp.join("tokens"))
        .with_env_overlay(overlay),
    );
    connect(&eng, "acme/kb", "kb", &tmp.join("kb")).await;
    // A domain with no origin, which GitHub does not govern.
    eng.domain_add_virtual("plain").await.unwrap();
    let host = eng.github_app().unwrap().credentials().host.clone();
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    auth.add_user("agent", "Agent", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    auth.add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.link_github("agent", &host, AGENT_ID, "agent-gh")
        .await
        .unwrap();
    auth.link_github("ada", &host, ADA_ID, "ada-gh")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth.clone()).with_governor(eng.knowledge_governor()),
    ));
    (forge, eng, auth)
}

/// Sets `repo`'s collaborators: (id, login, role).
fn collaborators(forge: &Forge, repo: &str, people: &[(u64, &str, &str)]) {
    forge.collaborators.lock().unwrap().insert(
        repo.to_string(),
        people
            .iter()
            .map(|(id, login, role)| json!({"id": id, "login": login, "role_name": role}))
            .collect(),
    );
}

#[tokio::test]
async fn the_open_tier_follows_the_agent_identity_link() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = setup(tmp.path(), Some("agent")).await;
    collaborators(&forge, "acme/kb", &[(AGENT_ID, "agent-gh", "write")]);
    let open_tier = Scope::Anonymous;
    // Inside an MCP request, as the server runs every tool call.
    as_mcp_call(async {
        assert!(eng.github_governs("kb"));
        assert!(!eng.github_governs("plain"));

        // The agent account is an instance viewer, and GitHub write is what
        // lets it write on the governed domain.
        assert_eq!(
            eng.domain_right(&open_tier, "kb").await.unwrap(),
            DomainRight::Write
        );
        assert_eq!(
            eng.write_access_block(&open_tier, "kb").await.unwrap(),
            None
        );
        assert!(eng.hidden_for(&open_tier).await.unwrap().is_empty());

        // GitHub says read now: the open tier reads and no longer writes.
        collaborators(&forge, "acme/kb", &[(AGENT_ID, "agent-gh", "read")]);
        eng.invalidate_github_access(crystalline_service::engine::Invalidate::Repo(
            "acme/kb".to_string(),
        ));
        assert_eq!(
            eng.domain_right(&open_tier, "kb").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            eng.write_access_block(&open_tier, "kb").await.unwrap(),
            Some(WriteBlock::Role)
        );

        // Not a collaborator on a private domain: the domain is hidden.
        auth.set_domain_visibility("kb", true, "ada").await.unwrap();
        collaborators(&forge, "acme/kb", &[]);
        eng.invalidate_github_access(crystalline_service::engine::Invalidate::All);
        assert!(eng.hidden_for(&open_tier).await.unwrap().contains("kb"));

        // The domain GitHub does not govern keeps the open tier's carve-out.
        assert_eq!(
            eng.write_access_block(&open_tier, "plain").await.unwrap(),
            None
        );
    })
    .await;
}

#[tokio::test]
async fn the_open_tier_never_writes_a_governed_domain_without_an_agent_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = setup(tmp.path(), None).await;
    collaborators(&forge, "acme/kb", &[(AGENT_ID, "agent-gh", "admin")]);
    forge.public.lock().unwrap().insert("acme/kb".to_string());
    let open_tier = Scope::Anonymous;
    // A shared domain on a public repository: anonymous reads it.
    assert_eq!(
        eng.domain_right(&open_tier, "kb").await.unwrap(),
        DomainRight::Read
    );
    assert_eq!(
        eng.write_access_block(&open_tier, "kb").await.unwrap(),
        Some(WriteBlock::Role)
    );
    assert!(!eng.open_tier_carve_out(&open_tier, "kb"));
    // Inside an MCP request too: there is no agent identity to act as.
    as_mcp_call(async {
        assert_eq!(
            eng.write_access_block(&open_tier, "kb").await.unwrap(),
            Some(WriteBlock::Role)
        );
    })
    .await;
    // Nor does it curate the governed domain's Reflection, which it still
    // does on the domain GitHub does not govern.
    let caller = ReflectionCaller::agent(Scope::Anonymous, "agent", true);
    let right = |domain: &'static str| {
        let (eng, caller) = (&eng, &caller);
        async move { eng.reflection_status(domain, caller).await.unwrap()["right"].clone() }
    };
    assert_ne!(right("kb").await, json!("curate"));
    assert_eq!(right("plain").await, json!("curate"));
    // A private repository: nothing at all.
    forge.public.lock().unwrap().clear();
    auth.set_domain_visibility("kb", true, "ada").await.unwrap();
    eng.invalidate_github_access(crystalline_service::engine::Invalidate::All);
    assert!(eng.hidden_for(&open_tier).await.unwrap().contains("kb"));
}

#[tokio::test]
async fn an_mcp_token_agent_follows_its_account() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _auth) = setup(tmp.path(), Some("agent")).await;
    collaborators(
        &forge,
        "acme/kb",
        &[(ADA_ID, "ada-gh", "write"), (AGENT_ID, "agent-gh", "read")],
    );
    // An agent with an MCP token is resolved to its account, so it is the
    // account's own GitHub role that counts, never `github.agent_identity`.
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    assert_eq!(
        eng.domain_right(&ada, "kb").await.unwrap(),
        DomainRight::Write
    );
    assert_eq!(eng.write_access_block(&ada, "kb").await.unwrap(), None);
    let agent = Scope::User {
        account: "agent".to_string(),
        admin: false,
    };
    assert_eq!(
        eng.write_access_block(&agent, "kb").await.unwrap(),
        Some(WriteBlock::Role)
    );
    // Ada is no collaborator any more: she reads the shared domain as an
    // instance editor and writes nothing there.
    collaborators(&forge, "acme/kb", &[]);
    eng.invalidate_github_access(crystalline_service::engine::Invalidate::User(ADA_ID));
    assert_eq!(
        eng.domain_right(&ada, "kb").await.unwrap(),
        DomainRight::Read
    );
    assert_eq!(
        eng.write_access_block(&ada, "kb").await.unwrap(),
        Some(WriteBlock::Role)
    );
}

#[tokio::test]
async fn the_daemon_status_says_since_when_github_has_not_answered() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _auth) = setup(tmp.path(), None).await;
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    let status = || {
        let eng = eng.clone();
        async move {
            crystalline_service::control::answer_for_test(eng, &json!({ "v": 1, "cmd": "status" }))
                .await
        }
    };
    eng.domain_right(&ada, "kb").await.unwrap();
    let answer = status().await;
    assert_eq!(answer["ok"], json!(true), "{answer}");
    assert_eq!(answer["data"]["github_access_degraded_since"], json!(null));
    assert_eq!(answer["data"]["github_access_closed"], json!([]));
    assert_eq!(answer["data"]["github_agent_identity_admin"], json!(false));
    assert_eq!(eng.github_access_degraded(), None);

    forge
        .collaborators_down
        .store(true, std::sync::atomic::Ordering::SeqCst);
    eng.invalidate_github_access(crystalline_service::engine::Invalidate::All);
    // GitHub down and nothing known: ada, an instance editor, reads the
    // shared domain and writes nothing.
    assert_eq!(
        eng.domain_right(&ada, "kb").await.unwrap(),
        DomainRight::Read
    );
    let since = eng.github_access_degraded().expect("degraded");
    let answer = status().await;
    assert_eq!(
        answer["data"]["github_access_degraded_since"],
        json!(since.to_rfc3339())
    );
    assert_eq!(
        answer["data"]["github_access_stale_since"],
        json!(since.to_rfc3339())
    );
}

/// `Scope::Anonymous` outside an MCP request is the web's anonymous
/// visitor: it never gets the agent identity's GitHub access, even with
/// `auth.mcp` off and `github.agent_identity` set.
#[tokio::test]
async fn a_web_visitor_never_acts_as_the_agent_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = setup(tmp.path(), Some("agent")).await;
    collaborators(&forge, "acme/kb", &[(AGENT_ID, "agent-gh", "admin")]);
    auth.set_domain_visibility("kb", true, "ada").await.unwrap();
    let anonymous = Scope::Anonymous;
    assert!(eng.hidden_for(&anonymous).await.unwrap().contains("kb"));
    assert_eq!(
        eng.domain_right(&anonymous, "kb").await.unwrap(),
        DomainRight::None
    );
    assert!(
        eng.write_access_block(&anonymous, "kb")
            .await
            .unwrap()
            .is_some()
    );
    // The same call inside an MCP request is the open tier as the agent.
    as_mcp_call(async {
        assert!(!eng.hidden_for(&anonymous).await.unwrap().contains("kb"));
        assert_eq!(
            eng.write_access_block(&anonymous, "kb").await.unwrap(),
            None
        );
    })
    .await;
}

/// An engine whose App store read blocks until the test sends on the
/// returned channel, then answers a saved App; `kb` is registered with an
/// origin straight in the configuration, and `ada` (instance editor) is in
/// the accounts store. No forge: nothing here asks GitHub anything that has
/// to answer.
async fn blocked_app_engine(tmp: &Path) -> (Arc<Engine>, std::sync::mpsc::Sender<()>) {
    use crystalline_core::config::{DomainEntry, OriginConfig};
    let api_url = "http://127.0.0.1:9";
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let wait = std::sync::Mutex::new(wait);
    let lookup: crystalline_service::engine::AppLookupFn = Arc::new(move || {
        let _ = wait.lock().unwrap().recv();
        Ok(crystalline_remote::AppLookup {
            store: crystalline_remote::AppStore::file_fallback(Path::new("/nonexistent")),
            credentials: Some(crystalline_remote::AppCredentials {
                app_id: 7,
                client_id: "Iv23.test".to_string(),
                client_secret: None,
                private_key: super::github_app::PEM.to_string(),
                webhook_secret: None,
                slug: Some("crystalline-acme".to_string()),
                host: "127.0.0.1:9".to_string(),
            }),
            keychain_failed: None,
            marker: None,
        })
    });
    let mut domains = std::collections::BTreeMap::new();
    domains.insert(
        "kb".to_string(),
        DomainEntry {
            path: Some(tmp.join("kb")),
            origin: Some(OriginConfig {
                repo: "acme/kb".to_string(),
                path: None,
                branch: None,
                poll_secs: None,
            }),
            ..DomainEntry::default()
        },
    );
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(api_url.to_string()),
            ..GitHubConfig::default()
        }),
        domains: domains.into_iter().collect(),
        ..GlobalConfig::default()
    };
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(store)),
            cfg,
            None,
            Some(tmp.join("config.yaml")),
        )
        .with_origins_dir(tmp.join("origins"))
        .with_state_dir(tmp.to_path_buf())
        .with_token_store_dir(tmp.join("tokens"))
        .with_app_store_lookup(lookup),
    );
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    auth.add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth).with_governor(eng.knowledge_governor()),
    ));
    (eng, release)
}

/// Waits (for real, up to five seconds) until `kb` is governed.
async fn until_governed(eng: &Engine) {
    for _ in 0..500 {
        if eng.github_governs("kb") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the App never loaded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_never_waits_for_the_app_to_load() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, release) = blocked_app_engine(tmp.path()).await;
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    // The App store read hangs; the request is answered anyway, as on an
    // instance without an App, since nothing cheap says one is saved.
    let right = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        eng.domain_right(&ada, "kb"),
    )
    .await
    .expect("answered while the App loads")
    .unwrap();
    assert_eq!(right, DomainRight::Write);
    assert!(!eng.github_governs("kb"));
    release.send(()).unwrap();
    until_governed(&eng).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn while_a_saved_app_loads_its_domains_are_closed() {
    let tmp = tempfile::tempdir().unwrap();
    // The App file says an App is saved here.
    std::fs::create_dir_all(tmp.path().join("tokens")).unwrap();
    let crystalline_remote::AppStore::File { path } =
        crystalline_remote::AppStore::file_fallback(&tmp.path().join("tokens"))
    else {
        unreachable!()
    };
    std::fs::write(path, "{}").unwrap();
    let (eng, release) = blocked_app_engine(tmp.path()).await;
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    // Governed with no access: an instance editor reads the shared domain
    // and writes nothing until GitHub says otherwise.
    let right = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        eng.domain_right(&ada, "kb"),
    )
    .await
    .expect("answered while the App loads")
    .unwrap();
    assert!(eng.github_governs("kb"));
    assert_eq!(right, DomainRight::Read);
    release.send(()).unwrap();
}

/// The settings screen never waits for the App to load, and never shows
/// "no App" (which offers Create) while a saved one is loading.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_settings_view_of_a_loading_app_is_loading_not_absent() {
    use crystalline_service::engine::SettledApp;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("tokens")).unwrap();
    let crystalline_remote::AppStore::File { path } =
        crystalline_remote::AppStore::file_fallback(&tmp.path().join("tokens"))
    else {
        unreachable!()
    };
    std::fs::write(path, "{}").unwrap();
    let (eng, release) = blocked_app_engine(tmp.path()).await;
    eng.preload_github_app();
    assert!(matches!(
        eng.github_app_settled_status(),
        SettledApp::Loading
    ));
    release.send(()).unwrap();
    for _ in 0..500 {
        if let SettledApp::Known(status) = eng.github_app_settled_status() {
            assert!(status.configured);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the App never loaded");
}

/// Without anything saved, a loading App reads as none: Create is offered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_settings_view_of_nothing_saved_is_no_app_while_loading() {
    use crystalline_service::engine::SettledApp;
    let tmp = tempfile::tempdir().unwrap();
    let (eng, release) = blocked_app_engine(tmp.path()).await;
    eng.preload_github_app();
    assert!(matches!(eng.github_app_settled_status(), SettledApp::None));
    release.send(()).unwrap();
}

/// An instance admin named as `github.agent_identity` gives the open tier
/// its GitHub role on a governed domain and nothing more.
#[tokio::test]
async fn an_admin_agent_identity_gets_only_its_github_role() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = setup(tmp.path(), Some("root")).await;
    let host = eng.github_app().unwrap().credentials().host.clone();
    auth.add_user("root", "Root", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    auth.link_github("root", &host, 777, "root-gh")
        .await
        .unwrap();
    collaborators(&forge, "acme/kb", &[(777, "root-gh", "read")]);
    let open_tier = Scope::Anonymous;
    as_mcp_call(async {
        assert_eq!(
            eng.domain_right(&open_tier, "kb").await.unwrap(),
            DomainRight::Read
        );
        assert_eq!(
            eng.write_access_block(&open_tier, "kb").await.unwrap(),
            Some(WriteBlock::Role)
        );
        // Private, and no GitHub read: hidden, admin or not.
        auth.set_domain_visibility("kb", true, "ada").await.unwrap();
        collaborators(&forge, "acme/kb", &[]);
        eng.invalidate_github_access(crystalline_service::engine::Invalidate::All);
        assert!(eng.hidden_for(&open_tier).await.unwrap().contains("kb"));
        assert_eq!(
            eng.domain_right(&open_tier, "kb").await.unwrap(),
            DomainRight::None
        );
    })
    .await;
    assert!(eng.github_agent_identity_is_admin().await);
}

/// The open tier comments on a governed domain's threads only as the
/// agent identity, and only inside an MCP request; elsewhere it keeps
/// commenting wherever it reads.
#[tokio::test]
async fn the_open_tier_comments_on_a_governed_domain_only_as_its_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let caller = ReflectionCaller::agent(Scope::Anonymous, "agent", true);
    let (_forge, eng, _auth) = setup(tmp.path(), None).await;
    as_mcp_call(async {
        assert!(!eng.open_tier_may_comment(&caller, "kb"));
        assert!(eng.open_tier_may_comment(&caller, "plain"));
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, eng, _auth) = setup(tmp.path(), Some("agent")).await;
    assert!(!eng.open_tier_may_comment(&caller, "kb"), "outside MCP");
    as_mcp_call(async {
        assert!(eng.open_tier_may_comment(&caller, "kb"));
    })
    .await;
}

/// Through the MCP server itself: a tool call over an HTTP server with no
/// signed-in identity is the open tier, and the server runs it as an MCP
/// request, so it sees the private governed domain its agent identity
/// reads on GitHub.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_call_runs_as_an_mcp_request() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = setup(tmp.path(), Some("agent")).await;
    collaborators(&forge, "acme/kb", &[(AGENT_ID, "agent-gh", "read")]);
    auth.set_domain_visibility("kb", true, "ada").await.unwrap();
    // Outside an MCP request the anonymous caller does not see it.
    assert!(
        eng.hidden_for(&Scope::Anonymous)
            .await
            .unwrap()
            .contains("kb")
    );

    let server = crystalline_service::mcp::McpServer::new_http(eng.clone());
    let (client_io, server_io) = tokio::io::duplex(1 << 18);
    let server_task = tokio::spawn(async move { rmcp::serve_server(server, server_io).await });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let _server = server_task.await.unwrap().unwrap();
    let mut params = rmcp::model::CallToolRequestParams::new("list_domains".to_string());
    params = params.with_arguments(serde_json::Map::new());
    let answer = serde_json::to_string(&client.peer().call_tool(params).await.unwrap()).unwrap();
    assert!(answer.contains("kb"), "{answer}");
}

/// An engine with an App pointed at the forge and the domain `kb` on
/// `acme/kb` registered straight in the configuration (connecting it would
/// need the App installed), with `vic` (instance viewer, GitHub write),
/// `boss` (instance admin) and the open tier acting as `vic`.
async fn closed_setup(tmp: &Path, installed: bool) -> (Arc<Forge>, Arc<Engine>) {
    use crystalline_core::config::{DomainEntry, OriginConfig};
    let installed: &[(&str, u64)] = if installed { &[("acme/kb", 1)] } else { &[] };
    let (forge, url) = spawn_forge(
        &[("acme/kb", repo_with_manifest(""))],
        installed,
        &[(1, "ghs_test")],
    )
    .await;
    collaborators(&forge, "acme/kb", &[(5, "vic-gh", "write")]);
    let mut domains = std::collections::BTreeMap::new();
    domains.insert(
        "kb".to_string(),
        DomainEntry {
            path: Some(tmp.join("kb")),
            origin: Some(OriginConfig {
                repo: "acme/kb".to_string(),
                path: None,
                branch: None,
                poll_secs: None,
            }),
            ..DomainEntry::default()
        },
    );
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(url),
            agent_identity: Some("vic".to_string()),
            ..GitHubConfig::default()
        }),
        domains: domains.into_iter().collect(),
        ..GlobalConfig::default()
    };
    let overlay = EnvOverlay::from_vars(
        app_env()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let eng = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg,
            None,
            Some(tmp.join("config.yaml")),
        )
        .with_origins_dir(tmp.join("origins"))
        .with_state_dir(tmp.to_path_buf())
        .with_token_store_dir(tmp.join("tokens"))
        .with_env_overlay(overlay),
    );
    let host = eng.github_app().unwrap().credentials().host.clone();
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    auth.add_user("vic", "Vic", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    auth.add_user("boss", "Boss", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    auth.link_github("vic", &host, 5, "vic-gh").await.unwrap();
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth).with_governor(eng.knowledge_governor()),
    ));
    (forge, eng)
}

fn vic() -> Scope {
    Scope::User {
        account: "vic".to_string(),
        admin: false,
    }
}

/// Asserts `kb` is closed: nothing for the viewer, the open tier as its
/// agent, the open tier's carve-out or the anonymous caller; Own for the
/// instance admin and the machine owner.
async fn assert_closed(eng: &Engine) {
    assert_eq!(
        eng.domain_right(&vic(), "kb").await.unwrap(),
        DomainRight::None
    );
    assert!(eng.hidden_for(&vic()).await.unwrap().contains("kb"));
    as_mcp_call(async {
        assert_eq!(
            eng.domain_right(&Scope::Anonymous, "kb").await.unwrap(),
            DomainRight::None
        );
        assert!(
            eng.hidden_for(&Scope::Anonymous)
                .await
                .unwrap()
                .contains("kb")
        );
        assert!(
            eng.write_access_block(&Scope::Anonymous, "kb")
                .await
                .unwrap()
                .is_some()
        );
    })
    .await;
    assert_eq!(
        eng.domain_right(&Scope::Anonymous, "kb").await.unwrap(),
        DomainRight::None
    );
    let boss = Scope::User {
        account: "boss".to_string(),
        admin: false,
    };
    assert_eq!(
        eng.domain_right(&boss, "kb").await.unwrap(),
        DomainRight::Own
    );
    assert_eq!(
        eng.domain_right(&Scope::Unrestricted, "kb").await.unwrap(),
        DomainRight::Own
    );
    let closed = eng.github_access_closed();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].domain, "kb");
    assert_eq!(
        closed[0].reason,
        "the GitHub App is not installed on acme/kb: install it there or disconnect the domain"
    );
}

/// Waits (for real, up to five seconds) until `kb` is no longer closed.
async fn until_open(eng: &Engine) {
    for _ in 0..500 {
        if eng.github_access_closed().is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the domain never opened");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_whose_repository_never_had_the_app_is_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, eng) = closed_setup(tmp.path(), false).await;
    // The first question looks the repository up and learns it.
    eng.domain_right(&vic(), "kb").await.unwrap();
    assert!(eng.github_governs("kb"));
    assert_closed(&eng).await;
    let answer = crystalline_service::control::answer_for_test(
        eng.clone(),
        &json!({ "v": 1, "cmd": "status" }),
    )
    .await;
    assert_eq!(
        answer["data"]["github_access_closed"][0]["domain"],
        json!("kb"),
        "{answer}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_whose_app_was_removed_is_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng) = closed_setup(tmp.path(), true).await;
    assert_eq!(
        eng.domain_right(&vic(), "kb").await.unwrap(),
        DomainRight::Write
    );
    forge.uninstall("acme/kb");
    eng.invalidate_github_access(crystalline_service::engine::Invalidate::Repo(
        "acme/kb".to_string(),
    ));
    eng.domain_right(&vic(), "kb").await.unwrap();
    assert_closed(&eng).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_opens_when_the_app_is_installed_later() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng) = closed_setup(tmp.path(), false).await;
    eng.domain_right(&vic(), "kb").await.unwrap();
    assert_closed(&eng).await;
    forge.install("acme/kb", 1);
    // The installation webhook: asked again at once, in the background.
    eng.invalidate_github_access(crystalline_service::engine::Invalidate::Installation(1));
    until_open(&eng).await;
    assert_eq!(
        eng.domain_right(&vic(), "kb").await.unwrap(),
        DomainRight::Write
    );
}

/// Waits (for real, up to five seconds) until `kb` is closed.
async fn until_closed(eng: &Engine) {
    for _ in 0..500 {
        if !eng.github_access_closed().is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the domain never closed");
}

/// At stack start the App is loaded in the background and then every
/// governed repository is looked up once: a repository without the App is
/// closed without any caller asking about it first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repository_without_the_app_is_closed_before_anyone_asks() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, eng) = closed_setup(tmp.path(), false).await;
    // What `install_domain_access` does at start.
    eng.preload_github_app();
    until_closed(&eng).await;
    assert_closed(&eng).await;
}

/// The App loaded before the governor existed (a fast load, or a slot an
/// earlier origin operation settled): building the governor warms it up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governor_built_after_the_app_loaded_still_warms_up() {
    let tmp = tempfile::tempdir().unwrap();
    // `closed_setup` settles the App (`github_app()`) before it builds the
    // governor, and never preloads.
    let (_forge, eng) = closed_setup(tmp.path(), false).await;
    until_closed(&eng).await;
    assert_closed(&eng).await;
}
