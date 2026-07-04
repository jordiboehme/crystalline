//! Engine-level tests for GitHub origin collaboration: `origin_add`,
//! `origin_update` and `origin_status`, plus the gating matrix (the
//! `github.enabled` refusal and the read-only mode's asymmetric refusal).
//!
//! Every test injects `support::MockProvider` via `Engine::with_origin_provider`
//! and points origin state at a tempdir via `Engine::with_origins_dir`, so
//! nothing here reaches a network, a real GitHub repository or the real
//! machine's state directory.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_remote::RemoteError;
use crystalline_service::Engine;
use crystalline_service::engine::EngineError;
use crystalline_service::params::{ReadParams, SearchParams};
use support::MockProvider;
use tokio::sync::Mutex;

fn config(github_enabled: bool) -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    if github_enabled {
        cfg.github = Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        });
    }
    cfg
}

async fn engine_with(
    config_path: &Path,
    origins_dir: &Path,
    provider: Arc<MockProvider>,
    github_enabled: bool,
    read_only: bool,
) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        config(github_enabled),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_read_only(read_only)
    .with_origin_provider(provider)
    .with_origins_dir(origins_dir.to_path_buf())
}

fn manifest() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
    .into_bytes()
}

fn commit_files(pairs: &[(&str, Vec<u8>)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(p, c)| (p.to_string(), c.clone()))
        .collect()
}

// --- gating matrix -----------------------------------------------------------

#[tokio::test]
async fn github_disabled_refuses_all_three_origin_operations() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        false,
        false,
    )
    .await;
    let root = tmp.path().join("root");

    let add_err = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(add_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{add_err}"
    );

    let update_err = eng.origin_update(None).await.unwrap_err();
    assert!(
        matches!(update_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{update_err}"
    );

    let status_err = eng.origin_status(None).await.unwrap_err();
    assert!(
        matches!(status_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{status_err}"
    );
}

#[tokio::test]
async fn read_only_refuses_add_but_allows_update_and_status() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        true,
    )
    .await;
    let root = tmp.path().join("root");

    let add_err = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
        )
        .await
        .unwrap_err();
    assert!(matches!(add_err, EngineError::ReadOnly), "{add_err}");
    assert!(!root.exists(), "a refused add must not touch disk");

    // No origin domains are registered in this test, but the calls
    // themselves must not be refused for being read-only.
    let update = eng.origin_update(None).await.unwrap();
    assert_eq!(update["domains"].as_array().unwrap().len(), 0);
    assert_eq!(update["errors"].as_array().unwrap().len(), 0);

    let status = eng.origin_status(None).await.unwrap();
    assert_eq!(status["domains"].as_array().unwrap().len(), 0);
}

// --- origin_add ----------------------------------------------------------------

#[tokio::test]
async fn origin_add_creates_folder_registers_domain_and_indexes_engrams() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    let result = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            None,
            Some(root.to_str().unwrap()),
        )
        .await
        .unwrap();

    assert_eq!(result["domain"], "brand-knowledge");
    assert_eq!(result["engrams"], 2);
    assert_eq!(result["base_commit"], commit);
    assert_eq!(result["root"], root.display().to_string());

    // Files landed on disk.
    assert!(root.join("MANIFEST.md").exists());
    assert!(root.join("notes/alpha.md").exists());

    // Registered in the in-memory config and persisted to the config file.
    assert!(eng.config().domains.contains_key("brand-knowledge"));
    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let entry = on_disk.domains.get("brand-knowledge").unwrap();
    let origin_cfg = entry.origin.as_ref().expect("origin config");
    assert_eq!(origin_cfg.repo, "acme/brand-knowledge");
    assert_eq!(origin_cfg.branch(), "main");
    assert_eq!(entry.file_path().as_deref(), Some(root.as_path()));

    // Indexed: readable through the engine's own read path.
    let read = eng
        .read_engram(&ReadParams {
            identifier: "alpha".to_string(),
            domain: Some("brand-knowledge".to_string()),
        })
        .await
        .unwrap();
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("shared knowledge about turbines")
    );
}

#[tokio::test]
async fn origin_add_refuses_a_domain_name_already_registered() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;

    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let other_root = tmp.path().join("other");
    let err = eng
        .origin_add(
            "acme/other-repo",
            Some("brand"),
            None,
            None,
            Some(other_root.to_str().unwrap()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(!other_root.exists(), "a refused add must not touch disk");
}

// --- origin_update ---------------------------------------------------------

#[tokio::test]
async fn origin_update_applies_an_upstream_edit_and_the_index_reflects_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "version one")),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c2);

    let result = eng.origin_update(Some("brand")).await.unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
    assert_eq!(domains[0]["up_to_date"], false);
    assert_eq!(domains[0]["applied"][0], "notes/alpha.md");
    assert_eq!(result["errors"].as_array().unwrap().len(), 0);

    // The working tree carries the upstream edit.
    let content = std::fs::read_to_string(root.join("notes/alpha.md")).unwrap();
    assert!(content.contains("version two"));

    // The index reflects it too.
    let hits = eng
        .search_engrams(&SearchParams {
            query: Some("revised upstream".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 1);
}

#[tokio::test]
async fn origin_update_named_domain_with_no_origin_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        false,
    )
    .await;

    let err = eng.origin_update(Some("nope")).await.unwrap_err();
    // Unregistered entirely, since none was ever added.
    assert!(matches!(err, EngineError::UnknownDomain { .. }), "{err}");
}

#[tokio::test]
async fn origin_update_one_domain_failing_does_not_abort_the_others() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let good_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("good-branch", &good_commit);
    let bad_commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("bad-branch", &bad_commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let good_root = tmp.path().join("good");
    let bad_root = tmp.path().join("bad");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;

    eng.origin_add(
        "acme/good",
        Some("good"),
        None,
        Some("good-branch"),
        Some(good_root.to_str().unwrap()),
    )
    .await
    .unwrap();
    eng.origin_add(
        "acme/bad",
        Some("bad"),
        None,
        Some("bad-branch"),
        Some(bad_root.to_str().unwrap()),
    )
    .await
    .unwrap();

    // Corrupt "bad"'s origin state so its pull fails (simulating an
    // unavailable origin) without touching "good".
    std::fs::remove_file(origins_dir.join("bad").join("state.json")).unwrap();

    let good_commit_2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("good-branch", &good_commit_2);

    let result = eng.origin_update(None).await.unwrap();
    let domains = result["domains"].as_array().unwrap();
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{result}");
    assert_eq!(domains[0]["domain"], "good");
    assert_eq!(errors.len(), 1, "{result}");
    assert_eq!(errors[0]["domain"], "bad");
    assert!(
        errors[0]["error"]
            .as_str()
            .unwrap()
            .contains("origin state")
    );

    // The healthy domain still applied its upstream change.
    assert!(good_root.join("notes/new.md").exists());
}

// --- origin_status -----------------------------------------------------------

#[tokio::test]
async fn origin_status_reports_behind_and_connection() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let status = eng.origin_status(Some("brand")).await.unwrap();
    assert_eq!(status["connection"]["connected"], true);
    assert_eq!(status["connection"]["user"], "mock-user");
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
    assert_eq!(domains[0]["repo"], "acme/brand-knowledge");
    assert_eq!(domains[0]["behind"], false);

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &c2);

    let status2 = eng.origin_status(Some("brand")).await.unwrap();
    let domains2 = status2["domains"].as_array().unwrap();
    assert_eq!(domains2[0]["behind"], true);
}

#[tokio::test]
async fn origin_status_with_no_domain_reports_every_origin_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let status = eng.origin_status(None).await.unwrap();
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
}
