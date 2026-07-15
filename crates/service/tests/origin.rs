//! Engine-level tests for GitHub origin collaboration: `origin_add`,
//! `origin_update`, `origin_status`, `origin_share`, `origin_discard` and
//! `origin_resolve`, plus the gating matrix (the `github.enabled` refusal
//! and the read-only mode's asymmetric refusal).
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
use crystalline_remote::provider::ProposalState;
use crystalline_remote::state::{
    OriginState, Proposal, ProposalStatus, ProposedChange, ProposedFile,
};
use crystalline_service::engine::EngineError;
use crystalline_service::params::{ReadParams, SearchParams};
use crystalline_service::{Engine, EnvOverlay};
use support::{CountingEmbedder, MockProvider, sha256_hex};
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

/// An engine whose only domains come from an environment overlay, wired to the
/// mock provider and a tempdir origins directory. GitHub is enabled so the
/// origin operations are not gated off.
async fn engine_with_env(
    config_path: &Path,
    origins_dir: &Path,
    provider: Arc<MockProvider>,
    env_vars: &[(&str, &str)],
) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    let overlay = EnvOverlay::from_vars(
        env_vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        config(true),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_origin_provider(provider)
    .with_origins_dir(origins_dir.to_path_buf())
    .with_env_overlay(overlay)
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

#[tokio::test]
async fn github_disabled_refuses_share_discard_and_resolve() {
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

    let share_err = eng.origin_share("brand", None, None).await.unwrap_err();
    assert!(
        matches!(share_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{share_err}"
    );

    let discard_err = eng.origin_discard("brand", 1).await.unwrap_err();
    assert!(
        matches!(discard_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{discard_err}"
    );

    let resolve_err = eng
        .origin_resolve("brand", "notes/a.md", Some("mine"), None)
        .await
        .unwrap_err();
    assert!(
        matches!(resolve_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{resolve_err}"
    );
}

#[tokio::test]
async fn read_only_refuses_share_discard_and_resolve() {
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

    // None of these need a registered domain: read-only refuses before the
    // domain is even resolved, exactly like `origin_add` above.
    let share_err = eng.origin_share("brand", None, None).await.unwrap_err();
    assert!(matches!(share_err, EngineError::ReadOnly), "{share_err}");

    let discard_err = eng.origin_discard("brand", 1).await.unwrap_err();
    assert!(
        matches!(discard_err, EngineError::ReadOnly),
        "{discard_err}"
    );

    let resolve_err = eng
        .origin_resolve("brand", "notes/a.md", Some("mine"), None)
        .await
        .unwrap_err();
    assert!(
        matches!(resolve_err, EngineError::ReadOnly),
        "{resolve_err}"
    );
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
async fn origin_add_connects_a_registered_domain_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "the team version"),
        ),
        (
            "notes/beta.md",
            engram("Beta", "beta", "only upstream has this"),
        ),
    ]));
    mock.set_branch("main", &commit);

    // A plain file domain, already registered and on disk, whose alpha
    // differs from upstream and which has no beta at all.
    let root = tmp.path().join("brand-knowledge");
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        root.join("notes/alpha.md"),
        engram("Alpha", "alpha", "my local take"),
    )
    .unwrap();

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut cfg = config(true);
    cfg.domains.insert(
        "brand".to_string(),
        crystalline_core::config::DomainEntry {
            kind: crystalline_core::config::DomainKind::File,
            path: Some(root.clone()),
            origin: None,
            provision: None,
        },
    );
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    )
    .with_origin_provider(mock)
    .with_origins_dir(origins_dir);

    let result = eng
        .origin_add("acme/brand-knowledge", Some("brand"), None, None, None)
        .await
        .expect("a registered origin-less domain connects in place");

    assert_eq!(result["domain"], "brand");
    assert_eq!(result["root"], root.display().to_string());
    assert_eq!(result["adopted"], true);
    assert_eq!(result["local_changes"], 1, "the differing alpha");

    // Local knowledge kept, missing upstream knowledge arrived.
    let alpha = std::fs::read_to_string(root.join("notes/alpha.md")).unwrap();
    assert!(alpha.contains("my local take"), "{alpha}");
    assert!(root.join("notes/beta.md").exists());

    // The entry kept its root and gained the origin, persisted to disk.
    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let entry = on_disk.domains.get("brand").unwrap();
    assert_eq!(entry.origin.as_ref().unwrap().repo, "acme/brand-knowledge");
    assert_eq!(entry.file_path().as_deref(), Some(root.as_path()));
}

#[tokio::test]
async fn origin_add_on_a_registered_domain_refuses_a_different_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let root = tmp.path().join("brand-knowledge");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut cfg = config(true);
    cfg.domains.insert(
        "brand".to_string(),
        crystalline_core::config::DomainEntry {
            kind: crystalline_core::config::DomainKind::File,
            path: Some(root.clone()),
            origin: None,
            provision: None,
        },
    );
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    )
    .with_origin_provider(mock)
    .with_origins_dir(origins_dir);

    let elsewhere = tmp.path().join("elsewhere");
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            None,
            None,
            Some(elsewhere.to_str().unwrap()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(!elsewhere.exists(), "a refused add must not touch disk");
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

#[tokio::test]
async fn origin_add_schedules_embedding_on_the_worker_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "alpha body")),
    ]));
    mock.set_branch("main", &commit);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        true,
        false,
    )
    .await
    .with_embed_channel(tx);
    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "origin_add must schedule a background embed instead of embedding inline"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embed_worker_runs_the_scheduled_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "alpha body")),
    ]));
    mock.set_branch("main", &commit);
    let root = tmp.path().join("brand-knowledge");

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = Arc::new(
        engine_with(
            &tmp.path().join("config.yaml"),
            &tmp.path().join("origins"),
            mock,
            true,
            false,
        )
        .await
        .with_embed_channel(tx),
    );
    let embedder = Arc::new(CountingEmbedder::new());
    eng.set_provider(embedder.clone());
    tokio::spawn(crystalline_service::engine::run_embed_worker(
        eng.clone(),
        rx,
    ));
    eng.origin_add(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    // Poll up to 2 s for the worker to run the pass.
    for _ in 0..200 {
        if embedder.calls.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the embed worker never ran the scheduled pass");
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

// --- env-defined domains -----------------------------------------------------

#[tokio::test]
async fn origin_update_bootstraps_an_env_domain_then_plain_pulls() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "shared knowledge about turbines"),
        ),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("team");
    let eng = engine_with_env(
        &config_path,
        &origins_dir,
        mock.clone(),
        &[
            ("CRYSTALLINE_DOMAIN_TEAM", root.to_str().unwrap()),
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/brand-knowledge"),
        ],
    )
    .await;

    // First update bootstraps: the missing-state env domain subscribes.
    let result = eng.origin_update(Some("team")).await.unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "team");
    assert_eq!(domains[0]["bootstrapped"], true);
    assert_eq!(domains[0]["engrams"], 2);
    assert_eq!(domains[0]["base_commit"], c1);
    assert_eq!(result["errors"].as_array().unwrap().len(), 0);

    // Files landed on disk and origin state now exists.
    assert!(root.join("MANIFEST.md").exists());
    assert!(root.join("notes/alpha.md").exists());
    assert!(
        OriginState::load(&origins_dir.join("team"))
            .unwrap()
            .is_some(),
        "origin state written on bootstrap"
    );

    // Indexed and searchable through the engine's own read path.
    let hits = eng
        .search_engrams(&SearchParams {
            query: Some("turbines".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 1);

    // Second update is a plain pull now that state is present: nothing new
    // upstream, so it is up to date and no longer marked bootstrapped.
    let result = eng.origin_update(Some("team")).await.unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert!(
        domains[0]["bootstrapped"].is_null(),
        "the second pull does not bootstrap"
    );
    assert_eq!(domains[0]["up_to_date"], true);
}

#[tokio::test]
async fn origin_add_on_an_env_defined_name_names_the_variable() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let team_root = tmp.path().join("team");
    let eng = engine_with_env(
        &config_path,
        &origins_dir,
        mock,
        &[("CRYSTALLINE_DOMAIN_TEAM", team_root.to_str().unwrap())],
    )
    .await;

    let other_root = tmp.path().join("other");
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("team"),
            None,
            None,
            Some(other_root.to_str().unwrap()),
        )
        .await
        .unwrap_err();
    match err {
        EngineError::Conflict(msg) => {
            assert!(msg.contains("CRYSTALLINE_DOMAIN_TEAM"), "{msg}")
        }
        other => panic!("expected Conflict naming the variable, got {other}"),
    }
    assert!(!other_root.exists(), "a refused add must not touch disk");
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

#[tokio::test]
async fn origin_update_reports_a_proposal_transition_with_its_url_and_title() {
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

    // Record an open share proposal directly in the domain's origin state, as
    // if it had been opened by a previous share (sharing itself is a later
    // task); `origin_update`'s pull refreshes it against the provider below.
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.proposals.push(Proposal {
        number: 7,
        url: "https://github.com/acme/brand-knowledge/pull/7".to_string(),
        branch: "share/glossary".to_string(),
        title: "Share glossary edits".to_string(),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Open,
        files: vec![],
    });
    state.save(&state_dir).unwrap();
    mock.set_proposal_state(7, ProposalState::Merged);

    // Move the branch so `pull` takes the "changed" path (which refreshes
    // proposals) rather than short-circuiting as up to date.
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &c2);

    let result = eng.origin_update(Some("brand")).await.unwrap();
    let domains = result["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{result}");
    let proposals = domains[0]["proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 1, "{result}");
    assert_eq!(proposals[0]["number"], 7);
    assert_eq!(proposals[0]["status"], "Merged");
    assert_eq!(
        proposals[0]["url"],
        "https://github.com/acme/brand-knowledge/pull/7"
    );
    assert_eq!(proposals[0]["title"], "Share glossary edits");

    // The merged proposal moved from `proposals` to `history` on disk.
    let reloaded = OriginState::load(&state_dir).unwrap().unwrap();
    assert!(reloaded.proposals.iter().all(|p| p.number != 7));
    assert!(reloaded.history.iter().any(|p| p.number == 7));
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
    assert_eq!(domains[0]["local_changes"], 0);

    // A local edit shows up as "ahead" (a local change against the base).
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/local.md"),
        engram("Local", "local", "not shared yet"),
    )
    .unwrap();
    let status_local = eng.origin_status(Some("brand")).await.unwrap();
    assert_eq!(status_local["domains"][0]["local_changes"], 1);

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

#[tokio::test]
async fn origin_status_survives_a_live_offline_probe_for_a_connected_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

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

    // A local edit so `local_changes` reports something real, not just a
    // default zero.
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/local.md"),
        engram("Local", "local", "not shared yet"),
    )
    .unwrap();

    // The GitHub connection (the mock provider override) is still present -
    // this is a live network outage, not a missing token - but the probe
    // itself cannot reach GitHub.
    mock.fail_branch_head_offline("main");

    let status = eng.origin_status(Some("brand")).await.unwrap();
    assert_eq!(
        status["errors"].as_array().unwrap().len(),
        0,
        "an offline probe must never hard-fail origin_status: {status}"
    );
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{status}");
    assert_eq!(domains[0]["domain"], "brand");
    assert!(
        domains[0]["behind"].is_null(),
        "behind must degrade to unknown, not error: {status}"
    );
    assert_eq!(domains[0]["local_changes"], 1);
    let probe_error = domains[0]["probe_error"]
        .as_str()
        .expect("probe_error must carry the offline message");
    assert!(probe_error.contains("offline"), "{probe_error}");
}

#[tokio::test]
async fn origin_status_offline_probe_on_one_domain_still_reports_both_domains() {
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

    mock.fail_branch_head_offline("bad-branch");

    let status = eng.origin_status(None).await.unwrap();
    assert_eq!(status["errors"].as_array().unwrap().len(), 0, "{status}");
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(
        domains.len(),
        2,
        "both domains must still be reported: {status}"
    );

    let good = domains
        .iter()
        .find(|d| d["domain"] == "good")
        .expect("good domain present");
    assert!(good["probe_error"].is_null());
    assert_eq!(good["behind"], false);

    let bad = domains
        .iter()
        .find(|d| d["domain"] == "bad")
        .expect("bad domain still present despite its offline probe");
    assert!(bad["probe_error"].as_str().is_some());
    assert!(bad["behind"].is_null());
}

#[tokio::test]
async fn origin_status_one_domain_genuinely_failing_does_not_abort_the_others() {
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

    // Corrupt "bad"'s origin state so its status genuinely fails, without
    // touching "good".
    std::fs::remove_file(origins_dir.join("bad").join("state.json")).unwrap();

    let status = eng.origin_status(None).await.unwrap();
    let domains = status["domains"].as_array().unwrap();
    let errors = status["errors"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{status}");
    assert_eq!(domains[0]["domain"], "good");
    assert_eq!(errors.len(), 1, "{status}");
    assert_eq!(errors[0]["domain"], "bad");
    assert!(
        errors[0]["error"]
            .as_str()
            .unwrap()
            .contains("origin state")
    );
}

// --- origin_share --------------------------------------------------------------

#[tokio::test]
async fn origin_share_happy_path_opens_a_proposal_and_records_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

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

    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();

    let result = eng.origin_share("brand", None, None).await.unwrap();
    assert_eq!(result["outcome"], "proposed");
    assert_eq!(result["added"][0], "notes/new.md");
    assert!(
        result["url"].as_str().unwrap().starts_with("https://"),
        "{result}"
    );

    // The branch name is slugged from the registered domain name "brand",
    // never the working tree's own folder name "brand-knowledge".
    let branch = result["branch"].as_str().unwrap();
    assert!(branch.contains("share-brand-"), "{branch}");
    assert!(!branch.contains("brand-knowledge"), "{branch}");

    // Recorded in the domain's origin state, open.
    let state_dir = origins_dir.join("brand");
    let state = OriginState::load(&state_dir).unwrap().unwrap();
    assert_eq!(state.proposals.len(), 1);
    assert_eq!(state.proposals[0].status, ProposalStatus::Open);
    assert_eq!(state.proposals[0].branch, branch);

    // The generated PR title names the domain "brand", not the folder
    // "brand-knowledge" it happens to live in.
    let title = &state.proposals[0].title;
    assert!(title.contains("brand"), "{title}");
    assert!(!title.contains("brand-knowledge"), "{title}");

    // Nothing local changed: a share never touches the working tree.
    assert!(root.join("notes/new.md").exists());
}

#[tokio::test]
async fn origin_share_with_pending_conflicts_reports_them_without_erroring() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one")),
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

    // A genuine same-line conflict, from a real pull.
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "line one LOCAL")).unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one UPSTREAM")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("brand")).await.unwrap();

    let result = eng.origin_share("brand", None, None).await.unwrap();
    assert_eq!(result["outcome"], "conflicts_pending");
    assert_eq!(result["count"], 1);
    let conflicts = result["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"], "notes/a.md");
}

// --- origin_discard --------------------------------------------------------------

#[tokio::test]
async fn origin_discard_restores_files_and_syncs_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/keep.md", engram("Keep", "keep", "base content")),
    ]));
    mock.set_branch("main", &commit);

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

    // A previously opened, now declined proposal touching keep.md, without
    // going through a real `origin_share` call.
    let proposed = engram("Keep", "keep", "shared v2 content");
    std::fs::write(root.join("notes/keep.md"), &proposed).unwrap();
    let state_dir = origins_dir.join("brand");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.proposals.push(Proposal {
        number: 5,
        url: "https://github.test/pulls/5".to_string(),
        branch: "crystalline/share-brand-000101000000".to_string(),
        title: "Refine 1 engram in brand".to_string(),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Declined,
        files: vec![ProposedFile {
            path: "notes/keep.md".to_string(),
            change: ProposedChange::Modified,
            sha256: Some(sha256_hex(&proposed)),
        }],
    });
    state.save(&state_dir).unwrap();

    let result = eng.origin_discard("brand", 5).await.unwrap();
    assert_eq!(result["restored"][0], "notes/keep.md");

    // The working tree is back to the base content.
    let content = std::fs::read_to_string(root.join("notes/keep.md")).unwrap();
    assert!(content.contains("base content"), "{content}");

    // The record moved to history, declined status preserved.
    let reloaded = OriginState::load(&state_dir).unwrap().unwrap();
    assert!(reloaded.proposals.is_empty());
    assert_eq!(reloaded.history[0].status, ProposalStatus::Declined);

    // The index reflects the restored content: sync ran after discard.
    let hits = eng
        .search_engrams(&SearchParams {
            query: Some("base content".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 1, "{hits}");
}

// --- origin_resolve --------------------------------------------------------------

#[tokio::test]
async fn origin_resolve_writes_the_resolution_and_syncs_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one")),
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

    // A local edit, then an upstream edit to the same line: a genuine
    // EditEdit conflict once pulled.
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "line one LOCAL")).unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one UPSTREAM")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("brand")).await.unwrap();

    let state_dir = origins_dir.join("brand");
    assert_eq!(
        OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .conflicts
            .len(),
        1
    );

    let result = eng
        .origin_resolve("brand", "notes/a.md", Some("theirs"), None)
        .await
        .unwrap();
    assert_eq!(result["remaining"], 0);

    let content = std::fs::read_to_string(root.join("notes/a.md")).unwrap();
    assert!(content.contains("line one UPSTREAM"), "{content}");
    assert!(
        OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .conflicts
            .is_empty()
    );

    // The index reflects the resolved content: sync ran after resolve.
    let hits = eng
        .search_engrams(&SearchParams {
            query: Some("UPSTREAM".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 1, "{hits}");
}

#[tokio::test]
async fn origin_resolve_unknown_path_errors_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

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

    let err = eng
        .origin_resolve("brand", "notes/missing.md", Some("mine"), None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Remote(RemoteError::ConflictNotFound { .. })
        ),
        "{err}"
    );
}
