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
use crystalline_service::Scope;
use crystalline_service::engine::{EngineError, PreviewCredential, ShareActor};
use crystalline_service::params::{ReadParams, SearchParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::scope::DomainAccess;
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
    // The overlay journal's own root, and it is named rather than defaulted:
    // an engine built without one refuses every draft write under the test
    // seam, on purpose, so a fixture that touches drafts has to say where the
    // mirror lives. It sits beside the origins directory, under the same
    // tempdir, so nothing here reaches the real machine's state directory.
    .with_state_dir(
        origins_dir
            .parent()
            .expect("the origins directory is inside the tempdir")
            .to_path_buf(),
    )
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

/// The same MANIFEST, declaring that this domain's generated folder listings
/// travel with it. The default is `local`, so a scenario whose subject needs a
/// listing in a share has to say so, the way a real team says it once in the
/// file all of its members hold.
fn manifest_sharing_indexes() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\ngenerated_indexes: shared\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
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

    let update_err = eng
        .origin_update(None, &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(
        matches!(update_err, EngineError::Remote(RemoteError::NotEnabled)),
        "{update_err}"
    );

    let status_err = eng
        .origin_status(None, false, &Scope::Unrestricted)
        .await
        .unwrap_err();
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
    let update = eng.origin_update(None, &Scope::Unrestricted).await.unwrap();
    assert_eq!(update["domains"].as_array().unwrap().len(), 0);
    assert_eq!(update["errors"].as_array().unwrap().len(), 0);

    let status = eng
        .origin_status(None, false, &Scope::Unrestricted)
        .await
        .unwrap();
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
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
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
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
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
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
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
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
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
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("brand-knowledge".to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
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
            review: None,
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
            review: None,
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

    let result = eng
        .origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
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
        .search_engrams(
            &SearchParams {
                query: Some("revised upstream".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
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

    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
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

    let err = eng
        .origin_update(Some("nope"), &Scope::Unrestricted)
        .await
        .unwrap_err();
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
    let result = eng
        .origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
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
        .search_engrams(
            &SearchParams {
                query: Some("turbines".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(hits["total"], 1);

    // Second update is a plain pull now that state is present: nothing new
    // upstream, so it is up to date and no longer marked bootstrapped.
    let result = eng
        .origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
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

    let result = eng.origin_update(None, &Scope::Unrestricted).await.unwrap();
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

    let result = eng
        .origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();
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

    let status = eng
        .origin_status(Some("brand"), false, &Scope::Unrestricted)
        .await
        .unwrap();
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
    let status_local = eng
        .origin_status(Some("brand"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status_local["domains"][0]["local_changes"], 1);

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &c2);

    let status2 = eng
        .origin_status(Some("brand"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domains2 = status2["domains"].as_array().unwrap();
    assert_eq!(domains2[0]["behind"], true);
}

/// The keys one domain entry carries when nobody asked for detail. Pinned as a
/// list rather than spot-checked so an accidental `detail: null` - a key that
/// costs every reader something and says nothing - fails here.
const STATUS_KEYS_WITHOUT_DETAIL: [&str; 17] = [
    "base_commit",
    "behind",
    "branch",
    "conflicts",
    "declined_proposals",
    "domain",
    "last_checked",
    "local_changes",
    "merged_unconsumed",
    "open_proposals",
    "probe_error",
    "repair_pending",
    "repo",
    "skipped_large",
    "stack_link_pending",
    "stack_number",
    "stack_wedged",
];

/// A status asked for detail names the unshared work; a status not asked for it
/// is the payload it always was.
///
/// The delta is the one that misled a reader: something added, something
/// modified, something DELETED, and folder listings riding along. The deletion
/// is the assertion that earns the test, because it is the change kind no scan
/// of the filesystem can find - the file is not there to be found.
#[tokio::test]
async fn origin_status_detail_names_the_changes_and_the_default_still_only_counts_them() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest_sharing_indexes()),
        ("notes/edit.md", engram("Edit", "edit", "the team's copy")),
        ("notes/gone.md", engram("Gone", "gone", "the team's copy")),
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

    // One of each kind, plus two refreshed folder listings that ride along.
    std::fs::write(
        root.join("notes/added.md"),
        engram("Added", "added", "written here, never shared"),
    )
    .unwrap();
    std::fs::write(
        root.join("notes/edit.md"),
        engram("Edit", "edit", "edited here since the pull"),
    )
    .unwrap();
    std::fs::remove_file(root.join("notes/gone.md")).unwrap();
    std::fs::write(root.join("index.md"), b"# listing\n").unwrap();
    std::fs::write(root.join("notes/index.md"), b"# listing\n").unwrap();

    let counted = eng
        .origin_status(Some("brand"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &counted["domains"][0];
    assert_eq!(entry["local_changes"], 3, "{counted}");
    let mut keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys, STATUS_KEYS_WITHOUT_DETAIL,
        "a status nobody asked detail of is the payload it always was: {entry}"
    );

    let named = eng
        .origin_status(Some("brand"), true, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &named["domains"][0];
    assert_eq!(
        entry["local_changes"], 3,
        "the bare count is unchanged by asking: {entry}"
    );
    let detail = &entry["detail"];
    assert_eq!(detail["added"], serde_json::json!(["notes/added.md"]));
    assert_eq!(detail["modified"], serde_json::json!(["notes/edit.md"]));
    assert_eq!(
        detail["deleted"],
        serde_json::json!(["notes/gone.md"]),
        "the deleted file is on no filesystem: this is the only surface that can name it"
    );
    assert_eq!(
        detail["generated_indexes"],
        serde_json::json!(2),
        "the listings ride along as one number and are named nowhere: {detail}"
    );
    let named_count: usize = ["added", "modified", "deleted"]
        .iter()
        .map(|key| detail[*key].as_array().unwrap().len())
        .sum();
    assert_eq!(
        named_count,
        entry["local_changes"].as_u64().unwrap() as usize,
        "the three arrays sum to the count: {entry}"
    );
}

/// Offline is exactly when a caller cannot look the change list up anywhere
/// else, so the probe-free retry carries the detail too.
#[tokio::test]
async fn origin_status_detail_survives_an_offline_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/gone.md", engram("Gone", "gone", "the team's copy")),
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
    std::fs::remove_file(root.join("notes/gone.md")).unwrap();

    mock.fail_branch_head_offline("main");

    let status = eng
        .origin_status(Some("brand"), true, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &status["domains"][0];
    assert!(
        entry["probe_error"].is_string(),
        "this is the probe-free retry arm: {entry}"
    );
    assert_eq!(
        entry["detail"]["deleted"],
        serde_json::json!(["notes/gone.md"]),
        "a status that degraded to local state still names what the tree owes: {entry}"
    );
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

    let status = eng
        .origin_status(None, false, &Scope::Unrestricted)
        .await
        .unwrap();
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

    let status = eng
        .origin_status(Some("brand"), false, &Scope::Unrestricted)
        .await
        .unwrap();
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

    let status = eng
        .origin_status(None, false, &Scope::Unrestricted)
        .await
        .unwrap();
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

    let status = eng
        .origin_status(None, false, &Scope::Unrestricted)
        .await
        .unwrap();
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
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed");
    // The engram, and only the engram. The domain declares nothing, so its
    // generated `index.md` - which the engine regenerated when the engram
    // landed - stays on this machine.
    assert_eq!(result["added"], serde_json::json!(["notes/new.md"]));
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

/// The same share for a domain that declares `generated_indexes: shared`: the
/// listing the engine regenerated travels with the engram, which is what keeps
/// a repository browsable on the forge for a team that wants that.
#[tokio::test]
async fn a_domain_that_declares_shared_listings_carries_its_index_too() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
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
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed");
    assert_eq!(
        result["added"],
        serde_json::json!(["index.md", "notes/new.md"]),
        "the declared policy is what puts the listing in the share: {result}"
    );
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
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
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
    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();

    let result = eng
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
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
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest_sharing_indexes())]));
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
            eng.search_engrams(
                &SearchParams {
                    query: Some(needle.to_string()),
                    ..SearchParams::default()
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap()["total"]
                .as_u64()
                .unwrap()
        }
    };

    // Upstream gains a file. The preview's pull applies it.
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest_sharing_indexes()),
        (
            "notes/upstream-one.md",
            engram("Upstream One", "upstream-one", "first upstream arrival"),
        ),
    ]));
    mock.set_branch("main", &c2);

    let plan = eng
        .origin_share_preview(
            "brand",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
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
        ("MANIFEST.md", manifest_sharing_indexes()),
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
        .origin_share("brand", None, None, None, None, ShareActor::Owner)
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
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
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
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "updated", "{shared}");
}

/// A team engine over a stack-serving mock with a two-layer chain this machine
/// still owes the forge a link for. Both layers are real shares, and the saved
/// link is then broken by hand exactly as a failed `create_stack` leaves it -
/// the stack number cleared, the debt recorded - which is the state a status
/// probe would otherwise pay off. Returns the engine, the mock, the working
/// tree root and the origin state directory.
async fn engine_owing_a_stack_link(
    tmp: &tempfile::TempDir,
) -> (
    Engine,
    Arc<MockProvider>,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let mock = Arc::new(MockProvider::new());
    mock.enable_stacks();
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "alpha")),
    ]));
    mock.set_branch("main", &commit);
    let origins_dir = tmp.path().join("origins");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &origins_dir,
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
    let first = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(first["outcome"], "proposed", "{first}");
    std::fs::write(root.join("notes/b.md"), engram("Beta", "notes/b", "beta")).unwrap();
    let second = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "proposed", "{second}");
    assert!(
        second["stack_number"].is_number(),
        "the second layer is stacked on the first: {second}"
    );

    let state_dir = origins_dir.join("kb");
    let mut state = OriginState::load(&state_dir).unwrap().unwrap();
    state.stack_number = None;
    state.stack_link_pending = true;
    state.save(&state_dir).unwrap();
    (eng, mock, root, state_dir)
}

/// The stack calls a delta of the mock's call log carries, which is what
/// separates "the status wrote to the forge" from "the status read from it".
fn stack_calls(delta: &[String]) -> Vec<String> {
    delta
        .iter()
        .filter(|c| {
            c.starts_with("create_stack")
                || c.starts_with("extend_stack")
                || c.starts_with("dissolve_stack")
        })
        .cloned()
        .collect()
}

/// **A probed status in personal mode never spends the instance credential on
/// a forge write.** Settling an owed stack link is a `create_stack` /
/// `extend_stack`, and the provider a status probes with is the instance one,
/// since a read verb carries no actor. Instance mode is where that is the
/// credential every write goes out on anyway, so the settlement stays; personal
/// mode leaves the debt standing, and the next write on the acting identity's
/// own credential is what pays it off.
#[tokio::test]
async fn a_personal_mode_status_leaves_an_owed_stack_link_for_the_next_write() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, root, state_dir) = engine_owing_a_stack_link(&tmp).await;
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "personal".to_string(),
    })
    .await
    .unwrap();

    let before = mock.calls().len();
    let status = eng
        .origin_status(Some("kb"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let delta = mock.calls().split_off(before);
    assert!(
        stack_calls(&delta).is_empty(),
        "a read verb made a forge write on the instance credential: {delta:?}"
    );
    assert_eq!(
        status["domains"][0]["stack_link_pending"], true,
        "the debt is reported rather than paid: {status}"
    );
    assert!(
        OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .stack_link_pending,
        "and it is still owed on disk"
    );

    // The next write-class call settles it, on the credential the write itself
    // goes out on - here the acting account's, through the injected provider.
    let before = mock.calls().len();
    std::fs::write(root.join("notes/c.md"), engram("Gamma", "notes/c", "gamma")).unwrap();
    let shared = eng
        .origin_share(
            "kb",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "proposed", "{shared}");
    let delta = mock.calls().split_off(before);
    assert!(
        delta.iter().any(|c| c.starts_with("create_stack:")),
        "the share paid the debt off: {delta:?}"
    );
    assert!(
        !OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .stack_link_pending,
        "the debt is settled"
    );
}

/// The instance-mode half of the same rule, pinned so the fix above cannot
/// quietly become a behavior change for the default install: with one
/// credential doing everything, a probed status settles the owed link exactly
/// as it always has.
#[tokio::test]
async fn an_instance_mode_status_still_settles_an_owed_stack_link() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, mock, _root, state_dir) = engine_owing_a_stack_link(&tmp).await;

    let before = mock.calls().len();
    let status = eng
        .origin_status(Some("kb"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let delta = mock.calls().split_off(before);
    assert!(
        delta.iter().any(|c| c.starts_with("create_stack:")),
        "the probe settled the owed link: {delta:?}"
    );
    assert_eq!(
        status["domains"][0]["stack_link_pending"], false,
        "{status}"
    );
    assert!(
        !OriginState::load(&state_dir)
            .unwrap()
            .unwrap()
            .stack_link_pending,
        "the debt is cleared on disk"
    );
}

/// A 403 from the forge on a personal write is unreadable in its raw form,
/// and the fix is not the caller's to guess: personal mode's stacks are
/// same-repo and forks are unsupported, so collaborator access is a hard
/// requirement rather than a suggestion.
///
/// This drives the failure through the SHARE VERB rather than through the
/// enrichment function alone, which is the half a unit test cannot pin: the
/// teaching text has to be wired into the verb, with the login the write
/// actually went out as and the repository it was refused by interpolated. The
/// instance half of the same rule rides along - in instance mode the raw text
/// is what a failure keeps (spec section 8).
#[tokio::test]
async fn a_403_on_a_personal_share_teaches_the_collaborator_requirement() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("Alpha", "notes/a", "alpha")),
    ]));
    mock.set_branch("main", &commit);
    // The injected provider has no credential behind it to read a login off,
    // so the login it acts as is supplied beside it.
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock.clone(),
        true,
        false,
    )
    .await
    .with_origin_provider_login("alice-gh");
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "personal".to_string(),
    })
    .await
    .unwrap();
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

    mock.forbid_writes();
    let err = eng
        .origin_share(
            "kb",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .expect_err("the forge refused the write");
    assert_eq!(
        err.to_string(),
        "your GitHub account @alice-gh needs write access to acme/kb - ask a maintainer to add you as a collaborator."
    );

    // The same failure on the instance credential keeps today's text: nobody
    // is being told to fix a personal connection that was never used.
    eng.configure(&crystalline_service::engine::ConfigureAction::Set {
        key: "github.share_identity".to_string(),
        value: "instance".to_string(),
    })
    .await
    .unwrap();
    let err = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("the forge refuses this one too");
    assert!(
        !err.to_string().contains("collaborator"),
        "an instance-token failure keeps its own words: {err}"
    );
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
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
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
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
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
        .origin_share("kb", None, None, Some(number), None, ShareActor::Owner)
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
        .origin_share("kb", None, None, Some(9999), None, ShareActor::Owner)
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
    let v = eng
        .origin_status(Some("kb"), false, &Scope::Unrestricted)
        .await
        .unwrap();
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
        .origin_share_preview(
            "kb",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
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
    let v = eng
        .origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();
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

    let v = eng
        .origin_status(Some("kb"), false, &Scope::Unrestricted)
        .await
        .unwrap();
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
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();

    let status = eng
        .origin_status(Some("kb"), false, &Scope::Unrestricted)
        .await
        .unwrap();
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
        .search_engrams(
            &SearchParams {
                query: Some("base content".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
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
    eng.origin_update(Some("brand"), &Scope::Unrestricted)
        .await
        .unwrap();

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
        .search_engrams(
            &SearchParams {
                query: Some("UPSTREAM".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
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
        .search_engrams(
            &SearchParams {
                query: Some("alpha".to_string()),
                search_type: Some("hybrid".to_string()),
                domains: vec!["local-notes".to_string()],
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
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

// --- a share of a domain that reviews changes --------------------------------
//
// In review mode nothing a member writes is on disk: every write joins that
// member's own draft overlay and the folder on disk goes on saying what the
// team reviewed. So a share of such a domain cannot be a walk of the folder -
// it would find nothing, or worse, somebody's stray direct edit. It is built
// from the acting actor's own overlay rows, drafts as content and tombstones
// as deletions, against the base snapshot.
//
// Every fixture here writes the overlay rows by hand, the way the write verbs
// do, and deliberately writes no journal mirror: the rows are the live truth
// and the plan is built from them, so a test that journals nothing and still
// sees its drafts shared is the test that says so.

/// The base engram the team already has, at `notes/plan.md`.
fn team_plan() -> Vec<u8> {
    engram("Plan", "plan", "the plan as the team has it")
}

/// One actor's draft of the same path: their private rewrite of it.
const DRAFT_PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\nthe plan as this actor would have it\n";

/// A draft of a path no file holds: the sharp case, since nothing on disk
/// could have produced it.
const DRAFT_FRESH: &str = "---\ntype: engram\ntitle: Fresh\npermalink: fresh\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\na page only this actor has\n";

/// Another actor's draft, so a scenario can prove whose work travels.
const DRAFT_ALICE: &str = "---\ntype: engram\ntitle: Alice\npermalink: alice\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\na page only alice has\n";

/// A draft standing at a generated folder listing's own path.
const DRAFT_INDEX: &str = "---\ntype: index\ntitle: notes\npermalink: notes-index\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\n---\n\na listing this actor redrew\n";

/// An index row the way a write verb builds one for a draft that is on nobody's
/// disk: the markdown lives in the row's own `content` column, because the row
/// is the only place the draft exists.
fn overlay_record(text: &str, path: &str) -> crystalline_index::EngramRecord {
    let mut record = crystalline_index::EngramRecord::from_engram(
        &crystalline_core::parse_engram(text).unwrap(),
        path,
        crystalline_index::FileStamp {
            mtime: 0,
            size: text.len() as u64,
            sha256: "0".repeat(64),
        },
    );
    record.content = text.to_string();
    record
}

/// A reviewing domain `team` connected to `acme/team`, its first pull already
/// recorded, with `files` as the repository's own content.
///
/// Returns the engine, the working tree root and the origins directory. The
/// temp directory is the caller's to hold: everything here lives under it.
async fn reviewing_domain(
    tmp: &Path,
    mock: Arc<MockProvider>,
    files: &[(&str, Vec<u8>)],
) -> Engine {
    let commit = mock.add_commit(commit_files(files));
    mock.set_branch("main", &commit);
    let root = tmp.join("team-knowledge");
    let eng = engine_with(
        &tmp.join("config.yaml"),
        &tmp.join("origins"),
        mock,
        true,
        false,
    )
    .await;
    eng.origin_add(
        "acme/team",
        Some("team"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    eng.set_review_mode(
        "team",
        Some(crystalline_core::config::ReviewMode::Overlay),
        crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    eng
}

/// Write one actor's draft of `path` straight into the index, the way a write
/// verb in review mode does: a row in that actor's dimension, and nothing on
/// disk.
async fn draft(eng: &Engine, actor: &str, path: &str, text: &str) {
    let store = eng.store();
    let store = store.lock().await;
    let id = store
        .domain_id("team")
        .await
        .unwrap()
        .expect("team is indexed");
    store
        .upsert_overlay(id, actor, &overlay_record(text, path))
        .await
        .unwrap();
}

/// Write one actor's deletion of a base path: a tombstone row standing at that
/// path, whose permalink is the path, exactly as `write_overlay_tombstone`
/// records one.
async fn tombstone(eng: &Engine, actor: &str, path: &str) {
    let store = eng.store();
    let store = store.lock().await;
    let id = store
        .domain_id("team")
        .await
        .unwrap()
        .expect("team is indexed");
    let mut record = overlay_record(DRAFT_PLAN, path);
    record.tombstone = true;
    record.permalink = path.to_string();
    store.upsert_overlay(id, actor, &record).await.unwrap();
}

/// The scope one actor name means, as an authenticated surface resolves it.
/// The machine owner drafts as `owner`, which is [`Scope::Unrestricted`]'s own
/// overlay key, and anybody else drafts under their account.
fn scope_of(actor: &str) -> Scope {
    if actor == "owner" {
        Scope::Unrestricted
    } else {
        Scope::User {
            account: actor.to_string(),
            admin: false,
        }
    }
}

/// Write one actor's draft file: an attachment written in review mode, which
/// lands in that actor's files overlay and never in the folder.
///
/// It goes through `attachment_write_as` rather than through a `DomainView`,
/// because `domain_view` is `pub(crate)` and an integration test cannot build
/// one. The receipt's `draft` flag is asserted here so a fixture can never
/// quietly write the team's folder instead.
async fn file(eng: &Engine, actor: &str, path: &str, bytes: &[u8]) {
    let written = eng
        .attachment_write_as("team", path, bytes.to_vec(), &scope_of(actor))
        .await
        .unwrap();
    assert!(written.draft, "a write in review mode is a draft: {path}");
}

/// One actor's deletion of a reviewed file: a marker in their files overlay,
/// with the folder untouched.
async fn delete_file(eng: &Engine, actor: &str, path: &str) {
    let draft = eng
        .attachment_delete_as("team", path, &scope_of(actor))
        .await
        .unwrap();
    assert!(draft, "a deletion in review mode is a draft: {path}");
}

/// A file the team already has, and one only an actor's draft has.
const OLD_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00the team's old deck";
const DECK_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00a deck only alice has";

/// A share carries the acting actor's overlay FILES beside their rows: the
/// bytes of a file they drafted travel as an addition, their deletion of a
/// reviewed file travels as a deletion, and the folder on disk is untouched by
/// any of it.
///
/// A share that dropped a draft file would be worse than one that carried
/// nothing: `ops::propose` detects against the staged tree, so a file left out
/// of the staging reads as a file the actor deleted and the proposal would ask
/// the team to delete their own copy.
#[tokio::test]
async fn an_overlay_share_stages_the_actors_files_and_the_proposal_carries_the_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            ("assets/old.png", OLD_PNG.to_vec()),
        ],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    file(&eng, "owner", "assets/deck.png", DECK_PNG).await;
    delete_file(&eng, "owner", "assets/old.png").await;

    // Scoped to the deck alone: a staged overlay file is a detected change
    // like any other, so `files` selects among them the way it always has.
    let scoped = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            Some(&["assets/deck.png".to_string()]),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(scoped["outcome"], "proposed", "{scoped}");
    assert_eq!(scoped["added"], serde_json::json!(["assets/deck.png"]));
    assert_eq!(scoped["updated"], serde_json::json!([]), "{scoped}");
    assert_eq!(scoped["deleted"], serde_json::json!([]), "{scoped}");
    let branch = scoped["branch"].as_str().unwrap().to_string();
    let commit = mock
        .branch_commit(&branch)
        .expect("the share made a branch");
    assert_eq!(
        mock.commit_file(&commit, "assets/deck.png").as_deref(),
        Some(DECK_PNG),
        "the proposal carries the draft file's own bytes"
    );

    // And the whole delta: the draft, the new file and the deletion together.
    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    // The second share amends the proposal the first one opened, so the delta
    // rides under `proposal` rather than at the top level.
    let amended = &result["proposal"];
    assert_eq!(
        amended["added"],
        serde_json::json!(["assets/deck.png"]),
        "{result}"
    );
    assert_eq!(amended["updated"], serde_json::json!(["notes/plan.md"]));
    assert_eq!(amended["deleted"], serde_json::json!(["assets/old.png"]));
    let branch = amended["branch"].as_str().unwrap().to_string();
    let commit = mock
        .branch_commit(&branch)
        .expect("the share made a branch");
    assert_eq!(
        mock.commit_file(&commit, "assets/deck.png").as_deref(),
        Some(DECK_PNG),
        "the deck is in the tree the proposal points at"
    );
    assert!(
        mock.commit_file(&commit, "assets/old.png").is_none(),
        "and the file the actor deleted is not"
    );

    // A share never touches the working tree, in review mode least of all.
    assert!(
        !root.join("assets/deck.png").exists(),
        "the draft file is nobody's folder file yet"
    );
    assert!(
        root.join("assets/old.png").exists(),
        "and the deletion is a draft too: the team's copy is where it was"
    );
}

/// The delta a share carries is the actor's overlay and nothing else: a draft
/// over a base file is an update, a draft of a path no file holds is an
/// addition, a tombstone is a deletion - and a file somebody dropped into the
/// reviewed folder by hand is not part of it at all, which is the whole point
/// of review mode.
#[tokio::test]
async fn an_overlay_share_proposes_exactly_the_actors_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            ("notes/old.md", engram("Old", "old", "on its way out")),
        ],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    tombstone(&eng, "owner", "notes/old.md").await;
    // A stray direct edit of the reviewed folder. Nobody reviewed it, so no
    // share carries it: a walk of the tree would, and this is how the test
    // says the plan is not a walk.
    std::fs::write(
        root.join("notes/stray.md"),
        engram("Stray", "stray", "written straight to disk"),
    )
    .unwrap();

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(result["updated"], serde_json::json!(["notes/plan.md"]));
    assert_eq!(result["deleted"], serde_json::json!(["notes/old.md"]));
    assert!(
        !result.to_string().contains("stray"),
        "the stray direct edit is not part of anybody's draft: {result}"
    );
    // A share never touches the working tree, and in review mode that includes
    // the draft: the folder still says what the team reviewed.
    assert!(!root.join("notes/fresh.md").exists());
    assert!(root.join("notes/old.md").exists());
}

/// Scoping a share selects within the acting actor's own overlay. A path
/// somebody else is drafting is not among this actor's changes, so naming it
/// refuses the share and says which path it was.
#[tokio::test]
async fn files_naming_another_actors_path_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    draft(&eng, "alice", "notes/alice.md", DRAFT_ALICE).await;

    let err = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            Some(&["notes/alice.md".to_string()]),
            ShareActor::Owner,
        )
        .await
        .expect_err("a path this actor is not drafting cannot be shared");
    let text = err.to_string();
    assert!(text.contains("notes/alice.md"), "{text}");

    // The actor's own path still shares, so the refusal is about whose draft
    // it is and not about scoping at all.
    let result = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            Some(&["notes/fresh.md".to_string()]),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
}

/// The CLI is the machine owner, and the machine owner drafts under one name.
/// A share it makes carries the owner's overlay and nobody else's.
#[tokio::test]
async fn the_owner_cli_shares_the_owner_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    draft(&eng, "alice", "notes/alice.md", DRAFT_ALICE).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert!(
        !result.to_string().contains("alice"),
        "alice's draft is hers to share: {result}"
    );

    // And the same engine, asked as alice, carries hers and not the owner's.
    let hers = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("alice".to_string()),
        )
        .await
        .unwrap();
    assert!(
        hers.to_string().contains("notes/alice.md"),
        "alice shares her own draft: {hers}"
    );
}

/// An agent over HTTP on an instance that makes no agent authenticate has no
/// identity, so there is no draft for a share to be of. It is refused with the
/// one message the write verbs refuse with, which teaches the way in.
#[tokio::test]
async fn an_http_agent_without_identity_cannot_share_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;

    let err = eng
        .origin_share("team", None, None, None, None, ShareActor::HttpAgent)
        .await
        .expect_err("nobody's draft is every draft");
    assert_eq!(
        err.to_string(),
        crystalline_service::OVERLAY_NEEDS_IDENTITY,
        "the refusal is the constant itself"
    );

    // The preview refuses the same way, so no client is ever asked to confirm
    // a share this instance would then refuse.
    let err = eng
        .origin_share_preview(
            "team",
            None,
            None,
            None,
            ShareActor::HttpAgent,
            PreviewCredential::ActingIdentity,
        )
        .await
        .expect_err("the preview carries the share's own gates");
    assert_eq!(err.to_string(), crystalline_service::OVERLAY_NEEDS_IDENTITY);
}

/// A generated directory index is not a draft, and the domain's own
/// `generated_indexes` policy decides whether it takes part in a share at all.
/// The rule binds BOTH sides: the actor's entries and the base snapshot. Filter
/// the entries alone and every index the repository already recorded turns into
/// a proposed deletion of a file that is sitting right there.
#[tokio::test]
async fn an_overlay_share_respects_the_domains_generated_index_policy_on_both_sides() {
    // Local listings: neither the draft of one nor the base's own key travels.
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest()),
            ("notes/index.md", engram("notes", "notes-index", "listing")),
        ],
    )
    .await;
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    draft(&eng, "owner", "notes/index.md", DRAFT_INDEX).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(result["updated"], serde_json::json!([]));
    assert_eq!(
        result["deleted"],
        serde_json::json!([]),
        "the base's own listing key is not a deletion: {result}"
    );

    // The same domain declaring `generated_indexes: shared`: both halves stay.
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest_sharing_indexes()),
            ("notes/index.md", engram("notes", "notes-index", "listing")),
        ],
    )
    .await;
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    draft(&eng, "owner", "notes/index.md", DRAFT_INDEX).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(
        result["updated"],
        serde_json::json!(["notes/index.md"]),
        "a domain that shares its listings carries the drafted one: {result}"
    );
}

/// A share of a reviewing domain still pulls the team's FOLDER.
///
/// Every share pulls first, because a proposal has to be mergeable when it is
/// opened. In review mode the tree the share is detected against is staged, and
/// a pull that landed there instead would advance the base snapshot while the
/// folder stayed where it was - leaving the team's own files behind their base,
/// where every later share reads them as deletions. So the pull runs against the
/// folder, and the share is staged afterwards.
#[tokio::test]
async fn a_review_mode_share_pulls_the_teams_folder_not_the_staged_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;

    // The team merged somebody else's work while this draft was being written.
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("notes/merged.md", engram("Merged", "merged", "already in")),
    ]));
    mock.set_branch("main", &moved);

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));

    // The merged file is in the team's folder, where the whole instance reads
    // it, and not only in a staged tree that is gone by now.
    assert!(
        root.join("notes/merged.md").exists(),
        "the pull landed in the folder the team shares"
    );

    // And the next share still proposes the draft alone: nothing the pull
    // brought in reads as a deletion.
    let again = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(
        again["deleted"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "the folder is level with its own base: {again}"
    );
}

/// A draft carrying a `generated` block, so a preview of a domain that takes
/// changes directly has provenance to report and the contrast with a reviewing
/// domain's preview is a real difference rather than two empty answers.
const AUTHORED: &str = "---\ntype: engram\ntitle: Authored\npermalink: authored\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-02\ngenerated: { by: human:ada, at: 2026-08-29T09:00:00+00:00 }\n---\n\nsomebody is named for this one\n";

/// The team's copy moving while a share is being prepared is answered, not
/// merged.
///
/// A share pulls the folder first and then proposes against the staged tree. The
/// proposal step would pull for itself, and that pull would land the team's
/// merged work inside the staged tree - which is deleted when the share ends,
/// while the base snapshot advances past it, leaving the folder permanently
/// behind its own base with no pull that would ever bring it back. So the share
/// refuses instead, and says to run it again.
#[tokio::test]
async fn an_upstream_move_after_the_share_pulled_refuses_instead_of_merging_into_staging() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");
    let state_dir = tmp.path().join("origins").join("team");
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;

    let settled = crystalline_remote::state::OriginState::load(&state_dir)
        .unwrap()
        .unwrap()
        .base_commit;

    // The team merges something the instant after this share has pulled.
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("notes/merged.md", engram("Merged", "merged", "already in")),
    ]));
    mock.move_branch_after_head_probes(mock.branch_head_calls() + 1, "main", &moved);

    let err = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("a share cannot merge the team's work into a tree it is about to delete");
    let text = err.to_string();
    assert!(text.contains("run it again"), "{text}");

    // Nothing moved: not the folder, not the base record.
    assert!(!root.join("notes/merged.md").exists());
    let after = crystalline_remote::state::OriginState::load(&state_dir)
        .unwrap()
        .unwrap();
    assert_eq!(after.base_commit, settled, "the base record stands still");
    assert!(!after.files.contains_key("notes/merged.md"));
    assert!(after.conflicts.is_empty(), "no conflict was recorded");

    // Nothing was lost either: the draft is still the actor's.
    {
        let store = eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        let entries = store.overlay_entries(id, "owner").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "notes/fresh.md");
    }

    // And running it again, after the pull has landed, shares as it should.
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(root.join("notes/merged.md").exists());
    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(result["added"], serde_json::json!(["notes/fresh.md"]));
}

/// An actor holding no drafts has nothing to share, and hears that rather than
/// a proposal of the folder they never wrote in.
#[tokio::test]
async fn an_actor_with_no_drafts_has_nothing_to_share() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    // Somebody else is drafting; this actor is not.
    draft(&eng, "alice", "notes/alice.md", DRAFT_ALICE).await;

    let result = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "nothing_to_share", "{result}");
}

/// A shared draft is still a draft.
///
/// Sharing proposes the work; it does not take it out of the overlay. The rows
/// stay exactly as they stood, which is the precondition convergence is defined
/// against: a merged proposal pulled back is what clears them, and nothing else.
/// So a second share of the same untouched drafts proposes the same paths again,
/// against the same base.
#[tokio::test]
async fn a_shared_draft_is_still_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;

    let before = {
        let store = eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        store.overlay_entries(id, "owner").await.unwrap()
    };
    assert_eq!(before.len(), 2);

    let first = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(first["outcome"], "proposed");
    assert_eq!(first["added"], serde_json::json!(["notes/fresh.md"]));
    assert_eq!(first["updated"], serde_json::json!(["notes/plan.md"]));

    let after = {
        let store = eng.store();
        let store = store.lock().await;
        let id = store.domain_id("team").await.unwrap().unwrap();
        store.overlay_entries(id, "owner").await.unwrap()
    };
    assert_eq!(after, before, "the share cleared nothing");

    // The assertion above is the design's own requirement: the rows survive.
    // What a second share then ANSWERS is a property of the forge shape - this
    // mock serves no stacks, so the share is detected against the trunk and
    // updates the one open proposal with the same paths. On a forge that serves
    // stacks the same untouched drafts would be nothing new to stack, because a
    // layer is detected against the chain tip; that is the question Task 11
    // owns, and either answer is the same surviving rows.
    let second = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(second["outcome"], "updated", "{second}");
    assert_eq!(
        second["proposal"]["added"],
        serde_json::json!(["notes/fresh.md"])
    );
    assert_eq!(
        second["proposal"]["updated"],
        serde_json::json!(["notes/plan.md"])
    );
}

/// The preview a person confirms a share on lists the actor's overlay paths, and
/// nothing is preselected for them.
///
/// Preselection is a guess at which files in a mixed delta are the caller's own,
/// read off each file's `generated` block. A reviewing domain's plan is built
/// from the caller's own drafts, so every path in it is already theirs and the
/// guess has nothing left to answer - which is why `last_author` is absent here
/// and present for the same file in a domain that takes changes directly.
#[tokio::test]
async fn the_share_preview_lists_the_overlay_paths_without_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "owner", "notes/authored.md", AUTHORED).await;
    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    draft(&eng, "alice", "notes/alice.md", DRAFT_ALICE).await;

    let plan = eng
        .origin_share_preview(
            "team",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    let changes = plan["changes"].as_array().unwrap();
    let paths: Vec<&str> = changes
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["notes/authored.md", "notes/plan.md"]);
    for change in changes {
        assert_eq!(
            change["last_author"],
            serde_json::Value::Null,
            "a reviewing domain preselects nothing: {plan}"
        );
    }

    // The same file in a domain that takes changes directly DOES carry its
    // provenance, so the absence above is a decision and not an empty column.
    let direct_tmp = tempfile::tempdir().unwrap();
    let direct_mock = Arc::new(MockProvider::new());
    let commit = direct_mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    direct_mock.set_branch("main", &commit);
    let direct_root = direct_tmp.path().join("kb");
    let direct = engine_with(
        &direct_tmp.path().join("config.yaml"),
        &direct_tmp.path().join("origins"),
        direct_mock,
        true,
        false,
    )
    .await;
    direct
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(direct_root.to_str().unwrap()),
        )
        .await
        .unwrap();
    std::fs::create_dir_all(direct_root.join("notes")).unwrap();
    std::fs::write(direct_root.join("notes/authored.md"), AUTHORED).unwrap();
    let direct_plan = direct
        .origin_share_preview(
            "kb",
            None,
            None,
            None,
            ShareActor::Owner,
            PreviewCredential::ActingIdentity,
        )
        .await
        .unwrap();
    assert_eq!(
        direct_plan["changes"][0]["last_author"], "human:ada",
        "{direct_plan}"
    );
}

/// A base path the state directory holds no copy of is an error naming the way
/// out, never a quiet read of the folder instead.
///
/// The base snapshot's copies are what makes "exactly this actor's draft" true:
/// read the folder for one of them and a stray direct edit of that path would
/// travel as this actor's work. Every path a pull records is written to both in
/// lockstep, so a missing copy is a state directory somebody damaged, and a
/// resync is the answer.
#[tokio::test]
async fn a_base_path_with_no_recorded_copy_asks_for_a_resync() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;

    // The recorded copy goes missing while the path stays in the snapshot.
    let copy = tmp
        .path()
        .join("origins")
        .join("team")
        .join("base")
        .join("notes")
        .join("plan.md");
    assert!(copy.exists(), "the pull recorded a base copy");
    std::fs::remove_file(&copy).unwrap();

    let err = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("a damaged state directory is not a share");
    let text = err.to_string();
    assert!(text.contains("notes/plan.md"), "{text}");
    assert!(text.contains("resync"), "{text}");
}

/// A draft path is held to the rule every write and move verb holds one to, and
/// to no stricter rule of the share's own.
///
/// `notes/plan: v2.md` is a filename a person can choose and this product
/// indexes like any other, so the staging screen is `is_within_domain` rather
/// than the archive-and-attachment rule beside it. What such a path then meets
/// is the repository path rule in `crystalline_remote`, which refuses a colon
/// segment because it is a drive or stream marker on Windows - and it refuses it
/// for a file in a folder exactly as it does for a draft. So the two domains
/// answer the same thing, which is the point: review mode adds no screen of its
/// own.
#[tokio::test]
async fn a_draft_path_is_screened_the_way_a_file_in_the_folder_is() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "owner", "notes/plan: v2.md", DRAFT_FRESH).await;

    let drafted = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("the repository path rule refuses a colon segment");

    // The same filename, as a file in a domain that takes changes directly.
    let direct_tmp = tempfile::tempdir().unwrap();
    let direct_mock = Arc::new(MockProvider::new());
    let commit = direct_mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    direct_mock.set_branch("main", &commit);
    let direct_root = direct_tmp.path().join("kb");
    let direct = engine_with(
        &direct_tmp.path().join("config.yaml"),
        &direct_tmp.path().join("origins"),
        direct_mock,
        true,
        false,
    )
    .await;
    direct
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(direct_root.to_str().unwrap()),
        )
        .await
        .unwrap();
    std::fs::create_dir_all(direct_root.join("notes")).unwrap();
    std::fs::write(direct_root.join("notes/plan: v2.md"), DRAFT_FRESH).unwrap();
    let on_disk = direct
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .expect_err("the same rule, for a file in the folder");

    assert_eq!(
        drafted.to_string(),
        on_disk.to_string(),
        "review mode adds no screen of its own"
    );
    assert!(
        !drafted.to_string().contains("not inside the domain"),
        "the engine does not invent a refusal for a legal domain path: {drafted}"
    );
}

// --- convergence, conflicts and withdraw into the overlay ---------------------
//
// A draft ends one of two ways: the team merges it and a later pull brings it
// back, which takes it out of the overlay, or it goes on standing against a
// folder that has moved under it, which is its author's conflict and nobody
// else's. Both are decided by the convergence pass that runs after a pull that
// advanced the base, and both are asserted here in the row AND in the mirror
// beside it: a row cleared while its mirror stays would put the draft back on
// the next `reindex --wipe`.

/// The file one actor's draft is mirrored in, under the state directory these
/// fixtures point at: `<tmp>/overlays/team/<actor>/<path>`.
fn journal_file(tmp: &Path, actor: &str, path: &str) -> std::path::PathBuf {
    let mut file = tmp.join("overlays").join("team").join(actor);
    for seg in path.split('/') {
        file.push(seg);
    }
    file
}

/// Mirror a draft the way `write_overlay_entry` does after the row lands. The
/// `draft` helper above writes the row alone on purpose; a scenario about
/// convergence needs both halves, because clearing exactly one of them is the
/// bug it exists to catch.
fn mirror(tmp: &Path, actor: &str, path: &str, text: &str) {
    let file = journal_file(tmp, actor, path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// One actor's overlay rows in `team`, ordered as the store returns them.
async fn overlay_entries(eng: &Engine, actor: &str) -> Vec<crystalline_index::StoredEngram> {
    let store = eng.store();
    let store = store.lock().await;
    let id = store
        .domain_id("team")
        .await
        .unwrap()
        .expect("team is indexed");
    store.overlay_entries(id, actor).await.unwrap()
}

/// The paths one actor holds, for a readable assertion.
async fn overlay_paths(eng: &Engine, actor: &str) -> Vec<String> {
    overlay_entries(eng, actor)
        .await
        .into_iter()
        .map(|e| e.path)
        .collect()
}

/// The team merges a draft and the next pull brings it back: the draft is the
/// folder now, so it stops being a draft.
///
/// The row and the mirror go together. A draft at a path the pull never touched
/// is left exactly where it stands.
#[tokio::test]
async fn a_merged_and_pulled_draft_converges_out_of_the_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);

    // The team reviewed the rewrite and merged it.
    let merged = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", DRAFT_PLAN.as_bytes().to_vec()),
    ]));
    mock.set_branch("main", &merged);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/fresh.md".to_string()],
        "the merged draft left the overlay and the unmerged one stayed"
    );
    assert!(
        !journal_file(tmp.path(), "owner", "notes/plan.md").exists(),
        "the mirror went with the row, or the next wipe puts the draft back"
    );
    assert!(
        journal_file(tmp.path(), "owner", "notes/fresh.md").exists(),
        "the draft that is still a draft still has its mirror"
    );

    let status = eng
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &status["domains"][0];
    assert_eq!(domain["converged"]["cleared"], 1, "{status}");
    assert_eq!(domain["converged"]["diverged"], 0, "{status}");
    assert_eq!(domain["my_drafts"], 1, "the count reads the rows: {status}");
}

/// What one actor's files overlay holds, as `(path, tombstone)` pairs ordered
/// by path.
fn files_held(tmp: &Path, actor: &str) -> Vec<(String, bool)> {
    let dir = tmp.join("overlays/team").join(actor).join("files");
    let mut out = Vec::new();
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, bool)>) {
        let Ok(listed) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in listed.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.path().is_dir() {
                walk(&entry.path(), &rel, out);
            } else if let Some(base) = rel.strip_suffix(".tombstone") {
                out.push((base.to_string(), true));
            } else {
                out.push((rel, false));
            }
        }
    }
    walk(&dir, "", &mut out);
    out.sort();
    out
}

/// A pull catches the folder up with a draft file, and the draft stops being
/// one - both ways round.
///
/// A file whose bytes are now the team's own is settled exactly as a merged
/// page is, and a deletion of a file the team has now deleted too is settled
/// with nothing left to mark: a marker over a base nobody holds says nothing
/// at all.
#[tokio::test]
async fn a_pull_converges_a_byte_equal_file_and_a_sidecar_of_a_gone_base_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            ("assets/old.png", OLD_PNG.to_vec()),
        ],
    )
    .await;

    file(&eng, "alice", "assets/deck.png", DECK_PNG).await;
    delete_file(&eng, "alice", "assets/old.png").await;
    assert_eq!(
        files_held(tmp.path(), "alice"),
        vec![
            ("assets/deck.png".to_string(), false),
            ("assets/old.png".to_string(), true),
        ],
        "both halves stand before the pull"
    );

    // The team merged her deck and deleted the old one.
    let merged = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("assets/deck.png", DECK_PNG.to_vec()),
    ]));
    mock.set_branch("main", &merged);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        files_held(tmp.path(), "alice"),
        Vec::new(),
        "the folder says what she said, so she is drafting neither of them now"
    );
    let status = eng
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &status["domains"][0];
    assert_eq!(domain["converged"]["cleared"], 2, "{status}");
    assert_eq!(domain["converged"]["diverged"], 0, "{status}");
}

/// The team changes a file under the actor drafting it, and that is their
/// conflict and nobody else's - settled by the next write of the same path.
#[tokio::test]
async fn a_file_the_team_changed_under_the_actor_is_that_actors_divergence() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    file(&eng, "alice", "assets/deck.png", DECK_PNG).await;
    file(&eng, "bob", "assets/notes.png", DECK_PNG).await;

    // Somebody else's deck landed at the same path instead.
    let theirs = b"\x89PNG\r\n\x1a\n\x00the team's own deck".to_vec();
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
        ("assets/deck.png", theirs.clone()),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // Nothing was taken away from her.
    assert_eq!(
        files_held(tmp.path(), "alice"),
        vec![("assets/deck.png".to_string(), false)],
        "a conflict is reported, never resolved behind the author's back"
    );

    let hers = eng
        .origin_status(Some("team"), false, &scope_of("alice"))
        .await
        .unwrap();
    assert_eq!(
        hers["domains"][0]["converged"]["mine"],
        serde_json::json!(["assets/deck.png"]),
        "her own conflict, named beside the pages: {hers}"
    );
    let his = eng
        .origin_status(Some("team"), false, &scope_of("bob"))
        .await
        .unwrap();
    assert_eq!(
        his["domains"][0]["converged"]["mine"],
        serde_json::json!([]),
        "and nobody else's: {his}"
    );

    // A conflict resolution settles an engram, so it says what to do with a
    // file instead of normalizing the path into a miss.
    let refused = eng
        .origin_resolve(
            "team",
            "assets/deck.png",
            Some("mine"),
            None,
            ShareActor::Owner,
        )
        .await
        .expect_err("a resolution settles an engram's markdown");
    let text = refused.to_string();
    assert!(
        text.contains("upload the file again") && text.contains("delete it"),
        "and it teaches the pair of verbs that do settle one: {text}"
    );

    // Her next write of the same path is what settles it.
    file(&eng, "alice", "assets/deck.png", b"a third deck").await;
    let hers = eng
        .origin_status(Some("team"), false, &scope_of("alice"))
        .await
        .unwrap();
    // Nothing converged on that pass and nothing conflicts any more, which is
    // exactly what this key's absence means.
    assert!(
        hers["domains"][0]["converged"].is_null(),
        "writing the path again is the answer, and it leaves the conflict list: {hers}"
    );
    assert!(
        !hers.to_string().contains("assets/deck.png"),
        "her conflict is named nowhere now: {hers}"
    );
}

/// The folder moves under a draft and the draft is its author's conflict.
///
/// Whoever owns the domain is told how many each actor is holding open, and
/// never what any of them says.
#[tokio::test]
async fn a_diverged_draft_is_its_authors_conflict_only() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);
    // A draft at a path this pull never touches: not merged, not in conflict.
    draft(&eng, "alice", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "alice", "notes/fresh.md", DRAFT_FRESH);

    // Somebody else's change to the same page landed instead.
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the plan as the team now has it"),
        ),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // Nothing was taken away from her.
    assert_eq!(
        overlay_paths(&eng, "alice").await,
        vec!["notes/fresh.md".to_string(), "notes/plan.md".to_string()],
        "a conflict is reported, never resolved behind the author's back"
    );

    let hers = eng
        .origin_status(
            Some("team"),
            false,
            &Scope::User {
                account: "alice".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    let domain = &hers["domains"][0];
    assert_eq!(domain["converged"]["diverged"], 1, "{hers}");
    assert_eq!(
        domain["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "her own conflict, named: {hers}"
    );
    // Who may see the per-actor counts is Task 8's rule and not a second one
    // of this key's own: the same gate `drafts` already answers to, so the two
    // keys can never disagree about who is allowed to read them.
    assert_eq!(
        domain["converged"]["actors"].is_null(),
        domain["drafts"].is_null(),
        "the counts follow the draft counts beside them: {hers}"
    );

    // Somebody who is drafting nothing here has no conflict of their own.
    let theirs = eng
        .origin_status(
            Some("team"),
            false,
            &Scope::User {
                account: "bob".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        theirs["domains"][0]["converged"]["mine"],
        serde_json::json!([]),
        "{theirs}"
    );

    // Whoever owns the domain sees the counts, and no content at all.
    let owners = eng
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &owners["domains"][0];
    assert_eq!(
        domain["converged"]["actors"],
        serde_json::json!([{ "actor": "alice", "entries": 1 }]),
        "{owners}"
    );
    assert!(
        !domain["converged"]["actors"].to_string().contains("plan"),
        "an owner counts another actor's conflicts and reads none of them: {owners}"
    );
}

/// A pull that renames a base path takes the draft standing over it along.
///
/// The address is the signal: the base at the old path is gone and a base the
/// pull brought in answers to the address the draft holds, so the page moved
/// and the draft still applies to it. Row and mirror move as one.
#[tokio::test]
async fn a_renamed_base_path_takes_the_draft_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);

    // The team filed the same page under a new name.
    let renamed = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan-v2.md", team_plan()),
    ]));
    mock.set_branch("main", &renamed);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "alice").await,
        vec!["notes/plan-v2.md".to_string()],
        "the draft travelled with the page it is a draft of"
    );
    assert!(
        journal_file(tmp.path(), "alice", "notes/plan-v2.md").exists(),
        "the mirror moved with the row"
    );
    assert!(
        !journal_file(tmp.path(), "alice", "notes/plan.md").exists(),
        "and nothing was left at the old path for a wipe to resurrect"
    );
}

/// A pull that brings a base file answering to an address a live draft already
/// holds elsewhere is that author's conflict.
///
/// It is never a silent drop and never a rewrite of what the team reviewed:
/// search merges its hits by permalink, so two rows answering to one address
/// would lose one of them without a word.
#[tokio::test]
async fn a_pulled_address_collision_is_the_drafters_divergence() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(tmp.path(), mock.clone(), &[("MANIFEST.md", manifest())]).await;
    let root = tmp.path().join("team-knowledge");

    // A page only this actor has, answering to `plan`.
    draft(&eng, "owner", "notes/mine.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/mine.md", DRAFT_PLAN);

    // The team adds a page of its own at that address, under another name.
    let collided = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
    ]));
    mock.set_branch("main", &collided);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/mine.md".to_string()],
        "the draft stands where its author left it"
    );
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        team_plan(),
        "and the folder still says what the team reviewed"
    );

    let status = eng
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/mine.md"]),
        "{status}"
    );
}

/// Withdrawing a shared proposal with `revert` puts the actor's own view back,
/// and the folder the team reviewed is never touched.
#[tokio::test]
async fn withdraw_revert_lands_in_the_overlay_while_the_tree_stays() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);

    let shared = eng
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(shared["outcome"], "proposed", "{shared}");

    let report = eng
        .origin_withdraw("team", None, true, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(report["closed"], true, "{report}");

    assert!(
        overlay_paths(&eng, "owner").await.is_empty(),
        "the revert put this actor's view back to the team's folder"
    );
    assert!(
        !journal_file(tmp.path(), "owner", "notes/plan.md").exists()
            && !journal_file(tmp.path(), "owner", "notes/fresh.md").exists(),
        "row and mirror went together"
    );

    // The folder is exactly what it was: a revert in a reviewing domain has
    // nothing to say about what the team reviewed.
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        team_plan()
    );
    assert!(!root.join("notes/fresh.md").exists());
}

/// A conflict resolved in a reviewing domain is a draft, not a write of the
/// folder: keeping the team's version ends the draft, and merged content
/// becomes the author's new draft.
#[tokio::test]
async fn a_resolution_in_a_reviewing_domain_is_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);

    let merged = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            None,
            Some(DRAFT_FRESH.as_bytes()),
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(merged["draft"], true, "{merged}");
    let entries = overlay_entries(&eng, "owner").await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].content, DRAFT_FRESH, "{merged}");
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        team_plan(),
        "the folder the team reviewed is never what a resolution writes"
    );

    // Taking the team's version ends the draft entirely.
    let theirs = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(theirs["draft"], true, "{theirs}");
    assert!(overlay_paths(&eng, "owner").await.is_empty());
    assert!(!journal_file(tmp.path(), "owner", "notes/plan.md").exists());
}

/// Conflicts and withdrawals are somebody's, so an agent with no identity is
/// refused in the words every draft verb refuses in.
#[tokio::test]
async fn an_http_agent_without_identity_cannot_resolve_or_withdraw_a_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;

    let err = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            Some("theirs"),
            None,
            ShareActor::HttpAgent,
        )
        .await
        .expect_err("nobody's conflict is nobody's to settle");
    assert_eq!(err.to_string(), crystalline_service::OVERLAY_NEEDS_IDENTITY);

    let err = eng
        .origin_withdraw("team", None, true, ShareActor::HttpAgent)
        .await
        .expect_err("nobody's proposal is nobody's to withdraw");
    assert_eq!(err.to_string(), crystalline_service::OVERLAY_NEEDS_IDENTITY);
}

/// A domain that takes changes directly holds no drafts, so the pass a pull
/// runs has nothing to do and touches nothing.
#[tokio::test]
async fn a_pull_into_a_domain_that_takes_changes_directly_converges_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);
    let root = tmp.path().join("kb");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        mock.clone(),
        true,
        false,
    )
    .await;
    eng.origin_add(
        "acme/kb",
        Some("kb"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", team_plan()),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();

    let status = eng
        .origin_status(Some("kb"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let domain = &status["domains"][0];
    assert!(
        domain["converged"].is_null(),
        "a domain nobody drafts in says nothing about drafts: {status}"
    );
    assert!(
        !tmp.path().join("overlays").exists(),
        "and no journal was made"
    );
}

/// The clear-only pass ends what has landed and calls nothing a conflict.
///
/// It is the whole of what can be said with no pull to attribute a divergence
/// to: a draft the folder has caught up with is over, and a draft that says
/// something else is unshared work, which is not a conflict and never was.
#[tokio::test]
async fn the_clear_only_pass_ends_what_has_landed_and_flags_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", DRAFT_PLAN.as_bytes().to_vec()),
            ("notes/old.md", engram("Old", "old", "on its way out")),
        ],
    )
    .await;

    // A draft the folder already says, a draft it does not, and a deletion of
    // a page the folder still has.
    draft(&eng, "owner", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "owner", "notes/plan.md", DRAFT_PLAN);
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);
    tombstone(&eng, "owner", "notes/old.md").await;

    let report = eng.converge_overlays("team").await.unwrap();
    assert_eq!(report.cleared, 1, "the draft the folder caught up with");
    assert_eq!(report.diverged, 0, "and nothing is anybody's conflict");
    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/fresh.md".to_string(), "notes/old.md".to_string()]
    );

    // A domain that takes changes directly answers the same call with zeroes
    // and touches nothing at all.
    let quiet = eng.converge_overlays("no-such-domain").await.unwrap();
    assert_eq!(quiet, crystalline_service::ConvergenceReport::default());
}

/// A conflict the pull left in the reviewed folder is still settled against the
/// folder, even while the domain reviews changes.
///
/// It is nobody's draft: somebody edited the team's own files out of band and
/// upstream then changed the same file, so what settles it is the resolution
/// the verb has always performed. Review mode is not a reason to leave a
/// conflict standing in the team's files with no verb that can reach it - and
/// the drafts beside it are untouched by that resolution.
#[tokio::test]
async fn an_out_of_band_conflict_is_still_settled_against_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    let root = tmp.path().join("team-knowledge");

    // Somebody's draft, which takes no part in this at all.
    draft(&eng, "owner", "notes/fresh.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "owner", "notes/fresh.md", DRAFT_FRESH);

    // A direct edit of the reviewed folder, and an upstream change to the same
    // file: the pull records a conflict nobody drafted.
    std::fs::write(
        root.join("notes/plan.md"),
        engram("Plan", "plan", "edited straight on disk"),
    )
    .unwrap();
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", engram("Plan", "plan", "changed upstream")),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    let state = OriginState::load(&tmp.path().join("origins").join("team"))
        .unwrap()
        .unwrap();
    assert_eq!(state.conflicts.len(), 1, "the pull recorded a conflict");

    let settled = eng
        .origin_resolve(
            "team",
            "notes/plan.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
        .await
        .unwrap();
    assert_eq!(settled["remaining"], 0, "{settled}");
    assert!(
        settled["draft"].is_null(),
        "settling the folder's conflict is not a draft: {settled}"
    );
    assert_eq!(
        std::fs::read(root.join("notes/plan.md")).unwrap(),
        engram("Plan", "plan", "changed upstream"),
        "the folder took the team's version"
    );
    assert_eq!(
        overlay_paths(&eng, "owner").await,
        vec!["notes/fresh.md".to_string()],
        "and the draft beside it was never part of the question"
    );

    // A path that is neither a draft of this caller's nor a recorded conflict
    // teaches the way in rather than resolving something.
    let err = eng
        .origin_resolve(
            "team",
            "notes/nothing.md",
            Some("theirs"),
            None,
            ShareActor::Owner,
        )
        .await
        .expect_err("there is nothing there to settle");
    let text = err.to_string();
    assert!(
        text.contains("not drafting") && text.contains("draft the change first"),
        "{text}"
    );
}

// --- the convergence record survives the pulls that are about something else -
//
// A conflict is a fact about one actor's draft, not about the pull that
// happened to notice it. So the record is merged rather than rebuilt, and it
// lives beside the journal that mirrors the drafts it describes, which is what
// makes it survive a restart.

/// A pull about an unrelated path leaves a standing conflict standing.
#[tokio::test]
async fn an_unrelated_pull_leaves_a_standing_conflict_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[
            ("MANIFEST.md", manifest()),
            ("notes/plan.md", team_plan()),
            (
                "notes/other.md",
                engram("Other", "other", "untouched so far"),
            ),
        ],
    )
    .await;

    draft(&eng, "ada", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN);

    // Pull A moves the page ada is drafting: her draft is a conflict.
    let first = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
        (
            "notes/other.md",
            engram("Other", "other", "untouched so far"),
        ),
    ]));
    mock.set_branch("main", &first);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    let status = eng.origin_status(Some("team"), false, &ada).await.unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "{status}"
    );

    // Pull B is about a page nobody is drafting.
    let second = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
        (
            "notes/other.md",
            engram("Other", "other", "changed upstream"),
        ),
    ]));
    mock.set_branch("main", &second);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    let status = eng.origin_status(Some("team"), false, &ada).await.unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "a pull about another page does not settle ada's conflict: {status}"
    );
    assert_eq!(status["domains"][0]["converged"]["diverged"], 1, "{status}");
}

/// The clear-only pass ends what has landed and leaves every conflict standing.
#[tokio::test]
async fn the_clear_only_pass_leaves_the_conflicts_it_did_not_settle() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "ada", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN);

    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // Somebody else's draft has since become the folder, and a maintenance
    // sweep ends it. Ada's conflict is nobody's business of that sweep's.
    draft(
        &eng,
        "bo",
        "notes/plan.md",
        &String::from_utf8(engram("Plan", "plan", "the team moved it on")).unwrap(),
    )
    .await;
    mirror(
        tmp.path(),
        "bo",
        "notes/plan.md",
        &String::from_utf8(engram("Plan", "plan", "the team moved it on")).unwrap(),
    );
    let swept = eng.converge_overlays("team").await.unwrap();
    assert_eq!(swept.cleared, 1, "bo's draft is the folder now");

    let status = eng
        .origin_status(
            Some("team"),
            false,
            &Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "the sweep settled nothing of ada's: {status}"
    );
}

/// The record lives beside the journal, so a restart still knows the conflicts.
#[tokio::test]
async fn a_restart_still_names_the_conflicts_the_last_pull_found() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "ada", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/plan.md", DRAFT_PLAN);
    let moved = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/plan.md",
            engram("Plan", "plan", "the team moved it on"),
        ),
    ]));
    mock.set_branch("main", &moved);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    // A second engine over the same database, the same config file and the
    // same state directory: this process forgot everything it held in memory.
    let restarted = Engine::new(
        eng.store(),
        config(true),
        None,
        Some(tmp.path().join("config.yaml")),
    )
    .with_origin_provider(mock.clone())
    .with_origins_dir(tmp.path().join("origins"))
    .with_state_dir(tmp.path().to_path_buf());

    let status = restarted
        .origin_status(
            Some("team"),
            false,
            &Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["converged"]["mine"],
        serde_json::json!(["notes/plan.md"]),
        "a restart reads the conflicts off disk: {status}"
    );
}

/// While a domain reviews, an open proposal is somebody's, and the refusal says
/// whose.
///
/// Two different answers, because there are two different ways forward: the
/// proposal is yours and you may withdraw it, or it is somebody else's and it
/// is not yours to withdraw.
#[tokio::test]
async fn an_open_proposal_refuses_the_next_share_in_the_words_of_whose_it_is() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    mock.enable_stacks();
    let eng = reviewing_domain(
        tmp.path(),
        mock,
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;

    draft(&eng, "ada", "notes/ada.md", DRAFT_FRESH).await;
    mirror(tmp.path(), "ada", "notes/ada.md", DRAFT_FRESH);
    let first = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("ada".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(first["outcome"], "proposed", "{first}");

    // Bo has never shared anything. The refusal must not tell him to do the
    // thing he is doing.
    draft(&eng, "bo", "notes/bo.md", DRAFT_ALICE).await;
    mirror(tmp.path(), "bo", "notes/bo.md", DRAFT_ALICE);
    let err = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("bo".to_string()),
        )
        .await
        .expect_err("somebody else's proposal is open");
    let text = err.to_string();
    assert!(
        text.contains("ada") && text.contains("one proposal at a time"),
        "the refusal names whose it is and what to wait for: {text}"
    );
    assert!(
        !text.contains("share a fresh proposal instead"),
        "and never tells him to do what he was doing: {text}"
    );

    // Ada's own second share is the one that may withdraw and share again.
    draft(&eng, "ada", "notes/more.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "ada", "notes/more.md", DRAFT_PLAN);
    let err = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("ada".to_string()),
        )
        .await
        .expect_err("stacking on her own open layer is not served while the domain reviews");
    let text = err.to_string();
    assert!(
        text.contains("this domain reviews changes")
            && text.contains("withdraw it and share again"),
        "{text}"
    );

    // The record is written whole, the conflicts and the proposal owners beside
    // them, so a pass that only settles drafts must not forget whose the
    // proposal was.
    eng.converge_overlays("team").await.unwrap();
    let err = eng
        .origin_share(
            "team",
            None,
            None,
            None,
            None,
            ShareActor::Account("ada".to_string()),
        )
        .await
        .expect_err("her own proposal is still open");
    assert!(
        err.to_string().contains("withdraw it and share again"),
        "a convergence pass did not write over who owns the proposal: {err}"
    );
}

/// The accounts database wired into an engine, with one share-link on one
/// actor's draft already redeemed by `bob`.
///
/// Engine-level rather than over HTTP, because what is under test is the
/// ENGINE seam: a pull converging or renaming somebody's draft never passes a
/// route at all, and a test that drove one would be asserting about the wrong
/// layer.
async fn granted(eng: &Engine, tmp: &Path, actor: &str, path: &str) -> Arc<AuthStore> {
    let auth = Arc::new(AuthStore::open(&tmp.join("web-auth.db")).await.unwrap());
    for name in [actor, "bob"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    eng.set_domain_access(Arc::new(DomainAccess::new(auth.clone())));
    let minted = auth
        .mint_overlay_grant("team", path, actor, None)
        .await
        .unwrap();
    auth.redeem_overlay_grant(&minted.token, "bob")
        .await
        .unwrap()
        .expect("bob holds the link");
    auth
}

/// A pull that converges a granted draft out of the overlay ends its links.
///
/// The draft is the folder now, so there is nothing left to share: everybody
/// who can read the domain reads that text anyway. A link left live would be a
/// link to a draft that is not there, waiting to spring back onto whatever its
/// author drafts at that path next - and this path never touches the discard,
/// the fold or the move verb, which is exactly why the ending belongs at the
/// seam they all pass through rather than at each of them.
#[tokio::test]
async fn a_pulled_convergence_ends_the_links_on_the_draft_it_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);
    let auth = granted(&eng, tmp.path(), "alice", "notes/plan.md").await;
    assert_eq!(
        auth.overlay_grants_held("bob", "team").await.unwrap().len(),
        1,
        "bob holds the link before the pull"
    );

    // The team reviewed the rewrite and merged it.
    let merged = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan.md", DRAFT_PLAN.as_bytes().to_vec()),
    ]));
    mock.set_branch("main", &merged);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "the link ended with the draft the pull cleared"
    );

    // And a later draft at the same path revives nothing.
    draft(&eng, "alice", "notes/plan.md", DRAFT_FRESH).await;
    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "a second draft at that path is alice's alone"
    );
}

/// And the rename a pull performs when the base carries the draft along ends
/// them too.
///
/// That rename is `move_draft_with_the_base`, which never goes through the move
/// verb - so it is the path a hook on the verb alone would miss, and the reason
/// the ending is at the drop rather than at the verbs.
#[tokio::test]
async fn convergences_rename_ends_the_links_on_the_path_it_left() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    draft(&eng, "alice", "notes/plan.md", DRAFT_PLAN).await;
    mirror(tmp.path(), "alice", "notes/plan.md", DRAFT_PLAN);
    let auth = granted(&eng, tmp.path(), "alice", "notes/plan.md").await;

    // The team filed the same page under a new name.
    let renamed = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan-v2.md", team_plan()),
    ]));
    mock.set_branch("main", &renamed);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert_eq!(
        overlay_paths(&eng, "alice").await,
        vec!["notes/plan-v2.md".to_string()],
        "the draft travelled with the page it is a draft of"
    );
    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "and the link on the path it left ended with it: the author re-shares \
         the page under its new name"
    );
}

/// And the rename that makes the draft the folder ends them too, although it
/// writes no destination at all.
///
/// The other half of the same function, and the one a reader would skip. When
/// the team files the page under a new name and what it says there is word for
/// word what this actor was drafting, the draft has become the folder: there is
/// nothing left to write and nothing left to share, since everybody who can
/// read the domain reads that text anyway. The ending waits for the destination
/// on the path that has one, so this path has to end them where it returns -
/// which is the line a restructure loses.
#[tokio::test]
async fn a_convergence_that_makes_the_draft_the_folder_ends_its_links() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let eng = reviewing_domain(
        tmp.path(),
        mock.clone(),
        &[("MANIFEST.md", manifest()), ("notes/plan.md", team_plan())],
    )
    .await;
    // Her draft says exactly what the team's page says, which is what makes the
    // rename below land rather than move.
    let agreed = String::from_utf8(team_plan()).unwrap();
    draft(&eng, "alice", "notes/plan.md", &agreed).await;
    mirror(tmp.path(), "alice", "notes/plan.md", &agreed);
    let auth = granted(&eng, tmp.path(), "alice", "notes/plan.md").await;

    let renamed = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/plan-v2.md", team_plan()),
    ]));
    mock.set_branch("main", &renamed);
    eng.origin_update(Some("team"), &Scope::Unrestricted)
        .await
        .unwrap();

    assert!(
        overlay_paths(&eng, "alice").await.is_empty(),
        "her draft is the folder now, under its new name: {:?}",
        overlay_paths(&eng, "alice").await
    );
    assert!(
        auth.overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "and the link ended with the draft it was a link to"
    );
}
