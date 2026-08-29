//! Engine-level tests for GitHub origin collaboration: `origin_add`,
//! `origin_update`, `origin_status`, `origin_share`, `origin_share_preview`,
//! `origin_withdraw`, `origin_conflict_detail` and
//! `origin_resolve`, plus the gating matrix (the `github.enabled` refusal
//! and the read-only mode's asymmetric refusal). The embed-worker
//! scheduling checks also live here, beside the harness they share; they
//! include the non-origin `domain_add_local` one.
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
use crystalline_remote::provider::{Feedback, ProposalState};
use crystalline_remote::state::{
    FeedbackItem, FeedbackKind, OriginState, Proposal, ProposalStatus, ProposedChange, ProposedFile,
};
use crystalline_service::engine::{EngineError, ShareActor};
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
async fn github_disabled_refuses_share_withdraw_preview_and_resolve() {
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

    let share_err = eng
        .origin_share("brand", None, None, None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(share_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{share_err}"
    );

    let withdraw_err = eng
        .origin_withdraw("brand", None, false, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(withdraw_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{withdraw_err}"
    );

    let preview_err = eng
        .origin_share_preview("brand", None, None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(preview_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{preview_err}"
    );

    let resolve_err = eng
        .origin_resolve("brand", "notes/a.md", Some("mine"), None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(resolve_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{resolve_err}"
    );
}

#[tokio::test]
async fn read_only_refuses_share_withdraw_preview_and_resolve() {
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
    let share_err = eng
        .origin_share("brand", None, None, None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(matches!(share_err, EngineError::ReadOnly), "{share_err}");

    let withdraw_err = eng
        .origin_withdraw("brand", None, false, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(withdraw_err, EngineError::ReadOnly),
        "{withdraw_err}"
    );

    let preview_err = eng
        .origin_share_preview("brand", None, None, ShareActor::Owner)
        .await
        .unwrap_err();
    assert!(
        matches!(preview_err, EngineError::ReadOnly),
        "{preview_err}"
    );

    let resolve_err = eng
        .origin_resolve("brand", "notes/a.md", Some("mine"), None, ShareActor::Owner)
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
async fn origin_add_reports_stage_progress() {
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

    let seen: Arc<std::sync::Mutex<Vec<(u64, u64, String)>>> = Arc::default();
    let cb: crystalline_service::engine::OriginProgress = {
        let seen = seen.clone();
        Arc::new(move |step, total, msg: &str| {
            seen.lock().unwrap().push((step, total, msg.to_string()));
        })
    };
    eng.origin_add_with_progress(
        "acme/brand-knowledge",
        None,
        None,
        None,
        Some(root.to_str().unwrap()),
        Some(cb),
    )
    .await
    .unwrap();
    let seen = seen.lock().unwrap();
    let steps: Vec<u64> = seen.iter().map(|(s, _, _)| *s).collect();
    assert_eq!(
        steps,
        vec![1, 2, 3, 4],
        "one strictly increasing step per stage"
    );
    assert!(seen.iter().all(|(_, total, _)| *total == 4));
    assert!(seen[0].2.contains("acme/brand-knowledge"));
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
async fn origin_add_retry_of_the_same_connect_is_idempotent() {
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
    let root_str = root.to_str().unwrap();

    // connect once
    let first = eng
        .origin_add("acme/brand-knowledge", None, None, None, Some(root_str))
        .await
        .unwrap();
    // retry with identical arguments
    let second = eng
        .origin_add("acme/brand-knowledge", None, None, None, Some(root_str))
        .await
        .unwrap();
    assert_eq!(second["already_connected"], serde_json::json!(true));
    assert_eq!(second["base_commit"], first["base_commit"]);
    assert_eq!(second["domain"], first["domain"]);
    assert_eq!(second["engrams"], first["engrams"]);
}

#[tokio::test]
async fn origin_add_retry_treats_absent_branch_as_main() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    let root_str = root.to_str().unwrap();

    // connect with branch None, retry with Some("main"): both mean main
    eng.origin_add("acme/brand-knowledge", None, None, None, Some(root_str))
        .await
        .unwrap();
    let second = eng
        .origin_add(
            "acme/brand-knowledge",
            None,
            None,
            Some("main"),
            Some(root_str),
        )
        .await
        .unwrap();
    assert_eq!(second["already_connected"], serde_json::json!(true));
}

#[tokio::test]
async fn origin_add_retry_with_a_different_origin_still_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = engine_with(&config_path, &origins_dir, mock, true, false).await;
    let root_str = root.to_str().unwrap();

    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root_str),
    )
    .await
    .unwrap();
    // different repo under the same domain name
    let err = eng
        .origin_add(
            "acme/other-knowledge",
            Some("brand"),
            None,
            None,
            Some(root_str),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(err.to_string().contains("already connected"), "{err}");
    // same repo, different branch
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            None,
            Some("dev"),
            Some(root_str),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    // same repo, different subpath
    let err = eng
        .origin_add(
            "acme/brand-knowledge",
            Some("brand"),
            Some("docs"),
            None,
            Some(root_str),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_add_racing_retry_waits_under_the_lock_and_never_redownloads() {
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

    // Park every tarball download on a gate: the first connect stalls
    // mid-download, holding the origin lock, while an identical retry races
    // in behind it.
    let gate = mock.block_tarball();

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = Arc::new(engine_with(&config_path, &origins_dir, mock.clone(), true, false).await);
    let root_str = root.to_str().unwrap().to_string();

    // Connect A: passes the pre-lock guard (no origin on file yet), takes the
    // origin lock and blocks in tarball with the config not yet persisted.
    let a = {
        let eng = eng.clone();
        let root_str = root_str.clone();
        tokio::spawn(async move {
            eng.origin_add("acme/brand-knowledge", None, None, None, Some(&root_str))
                .await
        })
    };
    // Give A a moment to reach the gate.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Connect B: identical arguments. It also passes the pre-lock guard
    // (still no origin persisted) and then queues on the origin lock A holds.
    let b = {
        let eng = eng.clone();
        let root_str = root_str.clone();
        tokio::spawn(async move {
            eng.origin_add("acme/brand-knowledge", None, None, None, Some(&root_str))
                .await
        })
    };
    // Give B a moment to reach and block on the origin lock.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Release the download; A finishes and persists its origin, then B wakes,
    // re-reads the config under the lock and must answer idempotently.
    gate.send(true).unwrap();

    let a = a.await.unwrap().unwrap();
    let b = b.await.unwrap().unwrap();

    // A is the fresh connect.
    assert!(
        a.get("already_connected").is_none(),
        "A is the fresh connect: {a}"
    );
    assert_eq!(a["domain"], "brand-knowledge");
    assert_eq!(a["engrams"], 2);

    // B saw A's just-persisted origin under the lock and answered
    // already-connected instead of re-running the whole connect.
    assert_eq!(b["already_connected"], serde_json::json!(true), "{b}");
    assert_eq!(b["domain"], a["domain"]);
    assert_eq!(b["base_commit"], a["base_commit"]);
    assert_eq!(b["engrams"], a["engrams"]);

    // The whole repo was downloaded exactly once: the racing retry never
    // re-downloaded it.
    assert_eq!(
        mock.tarball_calls(),
        1,
        "the racing retry must not re-download the repo"
    );
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
async fn origin_update_schedules_embedding_on_the_worker_channel() {
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false)
        .await
        .with_embed_channel(tx);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    // Drain the connect's own scheduled embed so the assertion below sees
    // only the update's pass.
    while rx.try_recv().is_ok() {}

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c2);

    eng.origin_update(Some("brand")).await.unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "origin_update must schedule a background embed instead of embedding inline"
    );
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
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
        author_login: None,
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

    let result = eng
        .origin_share("brand", None, None, None, ShareActor::Owner)
        .await
        .unwrap();
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

/// The join the whole feature hangs on: the login `resolve_share_provider`
/// hands back is what the proposal record names, in the default identity mode.
///
/// It is worth a whole share rather than a shaper assertion because the engine
/// holds a second, same-typed `Option<String>` at that call site - the
/// personal-mode-only login write failures are enriched with - and swapping
/// the two would zero every instance-mode share's author while every other test
/// in this tree stayed green.
#[tokio::test]
async fn a_share_records_the_login_it_acted_as_on_the_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    // The default identity mode, and a credential that names somebody: the
    // shape resolution 3 locked, where instance mode records its own login
    // rather than nobody.
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false)
        .await
        .with_origin_provider_login("instance-gh");
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

    let result = eng
        .origin_share("brand", None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");

    let state = OriginState::load(&origins_dir.join("brand"))
        .unwrap()
        .unwrap();
    assert_eq!(
        state.proposals[0].author_login.as_deref(),
        Some("instance-gh"),
        "the acting login reaches the record, not the personal-mode-only one"
    );
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

    let result = eng
        .origin_share("brand", None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "conflicts_pending");
    assert_eq!(result["count"], 1);
    let conflicts = result["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"], "notes/a.md");
}

/// A preview and a share both pull first, and a pull writes files. Those files
/// have to reach the index in the same call, exactly as `origin_update` makes
/// them reach it: leaving them out means an engram is on disk and unsearchable
/// until the poller happens to run, which is the shape of a bug nobody
/// attributes to a share.
#[tokio::test]
async fn a_preview_and_a_share_index_what_their_pull_applied() {
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

    let found = |needle: &'static str| {
        let eng = &eng;
        async move {
            eng.search_engrams(&SearchParams {
                query: Some(needle.to_string()),
                ..SearchParams::default()
            })
            .await
            .unwrap()["total"]
                .as_u64()
                .unwrap()
        }
    };

    // Upstream gains a file. The preview's pull applies it.
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/upstream-one.md",
            engram("Upstream One", "upstream-one", "first upstream arrival"),
        ),
    ]));
    mock.set_branch("main", &c2);

    let plan = eng
        .origin_share_preview("brand", None, None, ShareActor::Owner)
        .await
        .unwrap();
    // Nothing this domain knows is unshared. What the plan does carry is the
    // folder listings the generator wrote when the domain was subscribed: an
    // origin that has never seen them is genuinely behind on them, and they
    // ride along with a share rather than being one - so the action is a
    // create, and every path in it is a listing.
    assert_eq!(plan["action"], "create", "{plan}");
    let changes = plan["changes"].as_array().unwrap();
    assert!(
        !changes.is_empty(),
        "the listings are what makes this a create: {plan}"
    );
    assert!(
        changes
            .iter()
            .all(|c| c["path"].as_str().unwrap_or_default().ends_with("index.md")),
        "{plan}"
    );
    assert!(root.join("notes/upstream-one.md").exists());
    assert_eq!(
        found("first upstream arrival").await,
        1,
        "the preview's pull reached the index"
    );

    // And again for a share, whose own pull applies a second one.
    let c3 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/upstream-one.md",
            engram("Upstream One", "upstream-one", "first upstream arrival"),
        ),
        (
            "notes/upstream-two.md",
            engram("Upstream Two", "upstream-two", "second upstream arrival"),
        ),
    ]));
    mock.set_branch("main", &c3);

    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/local.md"),
        engram("Local", "local", "locally captured"),
    )
    .unwrap();

    let result = eng
        .origin_share("brand", None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(
        found("second upstream arrival").await,
        1,
        "the share's pull reached the index too"
    );
}

// --- origin_withdraw, origin_share_preview, origin_conflict_detail -------------

/// A team engine over the mock: domain "kb" subscribed at a two-file commit,
/// one engram edited locally and already shared. Returns the engine, the
/// mock, the working-tree root and the open proposal's number.
async fn shared_team_engine(
    tmp: &tempfile::TempDir,
) -> (Engine, Arc<MockProvider>, std::path::PathBuf, u64) {
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "alpha")),
    ]));
    mock.set_branch("main", &commit);
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock.clone(),
        true,
        false,
    )
    .await;
    let root = tmp.path().join("kb");
    eng.origin_add(
        "acme/kb",
        Some("kb"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    std::fs::write(
        root.join("notes/a.md"),
        engram("Alpha", "notes/a", "alpha v2"),
    )
    .unwrap();
    let shared = eng
        .origin_share("kb", None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "proposed", "{shared}");
    let number = shared["number"].as_u64().unwrap();
    (eng, mock, root, number)
}

/// The injected test provider short-circuits BOTH share-identity modes, which
/// is what keeps every origin test in this file (and every other one that
/// injects a mock) free of a credential: an engine sharing personally, with no
/// token of any kind on disk, still shares through the mock and never reaches
/// the token store. Personal mode's refusals are engine unit tests, where a
/// real credential resolution actually runs.
#[tokio::test]
async fn an_injected_provider_short_circuits_personal_mode_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, _number) = shared_team_engine(&tmp).await;
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "personal".to_string(),
    })
    .await
    .unwrap();

    std::fs::write(root.join("notes/c.md"), engram("Gamma", "notes/c", "gamma")).unwrap();
    let shared = eng
        .origin_share(
            "kb",
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "updated", "{shared}");
}

#[tokio::test]
async fn origin_withdraw_closes_the_pr_and_records_withdrawn() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root, _number) = shared_team_engine(&tmp).await;
    let v = eng
        .origin_withdraw("kb", None, false, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(v["status"], "withdrawn");
    assert_eq!(v["closed"], true);
    assert!(v["restored"].as_array().unwrap().is_empty());
    assert!(
        mock.calls()
            .iter()
            .any(|c| c.starts_with("close_proposal:")),
        "{:?}",
        mock.calls()
    );
    // The local edit stays: no revert was asked for.
    let text = std::fs::read_to_string(root.join("notes/a.md")).unwrap();
    assert!(text.contains("alpha v2"), "{text}");
}

#[tokio::test]
async fn origin_withdraw_with_revert_restores_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    let v = eng
        .origin_withdraw("kb", Some(number), true, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(v["restored"][0], "notes/a.md");
    let text = std::fs::read_to_string(root.join("notes/a.md")).unwrap();
    assert!(!text.contains("alpha v2"), "restored to base: {text}");
}

#[tokio::test]
async fn origin_share_maps_updated_and_diverged() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root, number) = shared_team_engine(&tmp).await;

    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();
    let second = eng
        .origin_share("kb", None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "updated");
    assert_eq!(second["proposal"]["number"], number);

    // A reviewer amends the proposal branch.
    let branch = second["proposal"]["branch"].as_str().unwrap().to_string();
    let amended = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch(&branch, &amended);
    std::fs::write(root.join("notes/c.md"), engram("Gamma", "notes/c", "gamma")).unwrap();
    let third = eng
        .origin_share("kb", None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(third["outcome"], "proposal_diverged");
    assert_eq!(third["proposal"]["number"], number);
    assert!(
        third["guidance"].as_str().unwrap().contains("withdraw"),
        "{third}"
    );
}

/// The `proposal` argument reaches `ops::propose` rather than being dropped
/// on the way: naming the one open layer amends exactly it, and the response
/// carries the stack fields (null here, since the mock forge serves no stacks
/// and the share takes the single-proposal fallback).
#[tokio::test]
async fn origin_share_amend_param_reaches_ops() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();

    let v = eng
        .origin_share("kb", None, None, Some(number), ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(v["outcome"], "updated", "{v}");
    assert_eq!(v["proposal"]["number"].as_u64(), Some(number), "{v}");
    assert!(v["proposal"]["stack_number"].is_null(), "{v}");
    assert!(v["proposal"]["stack_position"].is_null(), "{v}");
}

/// A share naming a proposal that is not an open layer earns `ops`'s teaching
/// refusal, and that text has to reach a control or MCP client word for word:
/// it names what was asked for and lists the layers that are actually open, so
/// the caller retries against a real number without a second round trip. The
/// engine boundary must not summarize it away - and nothing may be prepended
/// to it either: a framing clause in front of the guidance blames the machine
/// for what the request asked for, so the rendered error starts with the
/// teaching text itself.
#[tokio::test]
async fn origin_share_teaching_refusal_survives_the_engine_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();

    let err = eng
        .origin_share("kb", None, None, Some(9999), ShareActor::Owner)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        format!(
            "proposal #9999 is not an open layer of this domain; open layers: #{number} (layer 1)"
        ),
        "the refusal reaches a control or MCP client whole and unprefixed"
    );
}

/// `origin_status`'s per-domain entry names the stack and every debt around
/// it, even when there is nothing stacked: a caller reads the same four keys
/// whichever path the domain is on.
#[tokio::test]
async fn origin_status_json_names_wedge_and_pending_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, _root, _number) = shared_team_engine(&tmp).await;
    let v = eng.origin_status(Some("kb")).await.unwrap();
    let domain = &v["domains"][0];
    assert!(domain["stack_number"].is_null(), "{v}");
    assert_eq!(
        domain["stack_wedged"].as_array().map(Vec::len),
        Some(0),
        "{v}"
    );
    assert_eq!(domain["repair_pending"], false, "{v}");
    assert_eq!(domain["stack_link_pending"], false, "{v}");
}

#[tokio::test]
async fn origin_share_preview_names_the_action_and_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _mock, root, number) = shared_team_engine(&tmp).await;
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();
    let v = eng
        .origin_share_preview("kb", None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(v["action"], "update");
    assert_eq!(v["number"].as_u64(), Some(number));
    let changes = v["changes"].as_array().unwrap();
    assert!(!changes.is_empty());
    assert!(
        changes
            .iter()
            .all(|c| c["path"].is_string() && c["kind"].is_string()),
        "{changes:?}"
    );
    assert!(v["effective_title"].as_str().is_some());
}

#[tokio::test]
async fn origin_update_response_carries_open_proposal_feedback() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, _root, number) = shared_team_engine(&tmp).await;
    mock.set_feedback(
        number,
        Feedback {
            review_state: Some("changes_requested".to_string()),
            items: vec![FeedbackItem {
                author: "ana".to_string(),
                body: "tighten the wording".to_string(),
                path: None,
                line: None,
                submitted_at: "2026-08-21T10:00:00Z".to_string(),
                kind: FeedbackKind::Comment,
            }],
        },
    );
    let v = eng.origin_update(Some("kb")).await.unwrap();
    let prop = &v["domains"][0]["open_proposals"][0];
    assert_eq!(prop["feedback"][0]["body"], "tighten the wording", "{v}");
    assert_eq!(prop["review_state"], "changes_requested");
}

#[tokio::test]
async fn origin_status_flags_an_amended_open_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, _root, number) = shared_team_engine(&tmp).await;
    let branch = {
        let state_dir = tmp.path().join("origins").join("kb");
        let state = OriginState::load(&state_dir).unwrap().unwrap();
        state.proposals[0].branch.clone()
    };
    let amended = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch(&branch, &amended);

    let v = eng.origin_status(Some("kb")).await.unwrap();
    let open = &v["domains"][0]["open_proposals"][0];
    assert_eq!(open["number"].as_u64(), Some(number), "{v}");
    assert_eq!(open["amended_upstream"], true, "{v}");
}

/// [`shared_team_engine`] carried one step further: the open proposal is
/// withdrawn, then a local and an upstream edit of the same engram are pulled
/// into a genuine EditEdit conflict. Returns the engine, the working-tree root
/// and the recorded conflict's id.
async fn conflicted_team_engine(tmp: &tempfile::TempDir) -> (Engine, std::path::PathBuf, String) {
    let (eng, mock, root, _number) = shared_team_engine(tmp).await;
    // Clear the open proposal so the conflict setup is the only moving part.
    eng.origin_withdraw("kb", None, false, ShareActor::Owner)
        .await
        .unwrap();
    // Local and upstream edit the same engram differently, then pull.
    std::fs::write(
        root.join("notes/a.md"),
        engram("Alpha", "notes/a", "mine mine"),
    )
    .unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "theirs theirs")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("kb")).await.unwrap();

    let status = eng.origin_status(Some("kb")).await.unwrap();
    let id = status["domains"][0]["conflicts"][0]["id"]
        .as_str()
        .expect("the pull recorded a conflict")
        .to_string();
    (eng, root, id)
}

#[tokio::test]
async fn origin_conflict_detail_reads_both_sides_by_id_or_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, _root, id) = conflicted_team_engine(&tmp).await;
    let by_id = eng
        .origin_conflict_detail("kb", Some(&id), None)
        .await
        .unwrap();
    assert_eq!(by_id["id"], id.as_str());
    assert!(by_id["local"].as_str().unwrap().contains("mine mine"));
    assert!(
        by_id["upstream"]
            .as_str()
            .unwrap()
            .contains("theirs theirs")
    );
    let path = by_id["path"].as_str().unwrap().to_string();
    let by_path = eng
        .origin_conflict_detail("kb", None, Some(&path))
        .await
        .unwrap();
    assert_eq!(by_path["id"], id.as_str());
    assert!(by_id["note"].is_null(), "every side is UTF-8 here");
}

#[tokio::test]
async fn origin_conflict_detail_addressing_rules_and_a_missing_local_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, root, id) = conflicted_team_engine(&tmp).await;

    // Neither an id nor a path is a malformed request, not a missing one.
    let err = eng
        .origin_conflict_detail("kb", None, None)
        .await
        .unwrap_err();
    match err {
        EngineError::Invalid(msg) => assert!(msg.contains("an id or a path"), "{msg}"),
        other => panic!("expected Invalid, got {other}"),
    }

    // An id that matches nothing is not found, even though the path of the
    // one real conflict is passed alongside it: the id wins outright.
    let err = eng
        .origin_conflict_detail("kb", Some("deadbeef"), Some("notes/a.md"))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err}");

    // A conflict whose local file is gone reports a null local side rather
    // than failing: the recorded base and upstream still answer.
    std::fs::remove_file(root.join("notes/a.md")).unwrap();
    let v = eng
        .origin_conflict_detail("kb", Some(&id), None)
        .await
        .unwrap();
    assert!(v["local"].is_null(), "{v}");
    assert!(v["upstream"].as_str().unwrap().contains("theirs theirs"));
    assert!(v["note"].is_null(), "a missing side is not a binary side");
}

#[tokio::test]
async fn origin_withdraw_restores_files_and_syncs_the_index() {
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
            blob_sha: None,
            size: Some(proposed.len() as u64),
        }],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
        author_login: None,
    });
    state.save(&state_dir).unwrap();

    let result = eng
        .origin_withdraw("brand", Some(5), true, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["restored"][0], "notes/keep.md");
    assert_eq!(
        result["closed"], false,
        "a declined proposal is already closed"
    );

    // The working tree is back to the base content.
    let content = std::fs::read_to_string(root.join("notes/keep.md")).unwrap();
    assert!(content.contains("base content"), "{content}");

    // The record moved to history, recorded as withdrawn.
    let reloaded = OriginState::load(&state_dir).unwrap().unwrap();
    assert!(reloaded.proposals.is_empty());
    assert_eq!(reloaded.history[0].status, ProposalStatus::Withdrawn);

    // The index reflects the restored content: sync ran after the withdraw.
    let hits = eng
        .search_engrams(&SearchParams {
            query: Some("base content".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 1, "{hits}");
}

#[tokio::test]
async fn origin_withdraw_schedules_embedding_on_the_worker_channel() {
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(&config_path, &origins_dir, mock.clone(), true, false)
        .await
        .with_embed_channel(tx);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    // Drain the connect's own scheduled embed so the assertion below sees
    // only the withdraw's pass.
    while rx.try_recv().is_ok() {}

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
            blob_sha: None,
            size: Some(proposed.len() as u64),
        }],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
        author_login: None,
    });
    state.save(&state_dir).unwrap();

    eng.origin_withdraw("brand", Some(5), true, ShareActor::Owner)
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "origin_withdraw must schedule a background embed instead of embedding inline"
    );
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
        .origin_resolve(
            "brand",
            "notes/a.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
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
        .origin_resolve(
            "brand",
            "notes/missing.md",
            Some("mine"),
            None,
            ShareActor::Owner,
        )
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

// --- domain_add_local --------------------------------------------------------

#[tokio::test]
async fn domain_add_local_schedules_embedding_on_the_worker_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock,
        false,
        false,
    )
    .await
    .with_embed_channel(tx);

    let folder = tmp.path().join("local-notes");
    std::fs::create_dir_all(folder.join("notes")).unwrap();
    std::fs::write(folder.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        folder.join("notes/alpha.md"),
        engram("Alpha", "alpha", "alpha body"),
    )
    .unwrap();

    eng.domain_add_local(Some("local-notes"), Some(folder.to_str().unwrap()))
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_ok(),
        "domain_add_local must schedule a background embed instead of embedding inline"
    );
}

// --- hybrid search lock discipline (M2.1) ------------------------------------

/// Pins the three-phase `search_engrams` contract that the A2+A12 lock
/// restructure must preserve: with active embeddings and a stub provider a
/// hybrid search reports `mode: hybrid`, still returns hits and embeds the
/// query exactly once. It passes before and after the change (a regression pin,
/// not a red-first test); the win is that no store guard is held across the
/// provider embed call, which structural review guards, so this asserts
/// behavior only and never times a lock.
#[tokio::test]
async fn hybrid_search_returns_hits_and_embeds_the_query_once() {
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
    let embedder = Arc::new(CountingEmbedder::new());
    eng.set_provider(embedder.clone());

    // A local domain with one engram, synced and embedded inline: no embed
    // channel is wired, so `domain_add_local` runs the embed pass itself and the
    // store ends up with active embeddings for the engine's model.
    let folder = tmp.path().join("local-notes");
    std::fs::create_dir_all(folder.join("notes")).unwrap();
    std::fs::write(folder.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        folder.join("notes/alpha.md"),
        engram("Alpha", "alpha", "alpha body"),
    )
    .unwrap();
    eng.domain_add_local(Some("local-notes"), Some(folder.to_str().unwrap()))
        .await
        .unwrap();

    let before = embedder.calls.load(std::sync::atomic::Ordering::SeqCst);
    assert!(before >= 1, "the inline embed pass ran during domain add");

    let hits = eng
        .search_engrams(&SearchParams {
            query: Some("alpha".to_string()),
            search_type: Some("hybrid".to_string()),
            domains: vec!["local-notes".to_string()],
            ..SearchParams::default()
        })
        .await
        .unwrap();

    assert_eq!(
        hits["mode"], "hybrid",
        "active embeddings keep the mode hybrid"
    );
    assert!(
        hits["total"].as_u64().unwrap() >= 1,
        "the hybrid search still returns hits"
    );
    let after = embedder.calls.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(after, before + 1, "the query was embedded exactly once");
}
