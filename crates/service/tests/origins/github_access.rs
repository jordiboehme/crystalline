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
use crystalline_service::engine::WriteBlock;
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
