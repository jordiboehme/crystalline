//! Engine-level tests for the Group D domain-administration verbs:
//! unregister, GitHub status/disconnect, archive file reads and imports.

mod support;

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::params::{ListDomainsParams, SearchParams};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";
const MANIFEST: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n";

/// A file domain "eng" (MANIFEST + alpha), domains_root pinned inside the
/// temp dir so nothing this suite creates can land in a real home folder.
async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    // Pin the domains root (Option<PathBuf>, config.rs:52) inside the temp
    // dir so nothing this suite creates can land in a real home folder.
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// Unregister removes the registration and the index rows and nothing else:
/// the files stay exactly where they were, and the name is free again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregister_keeps_files_and_clears_the_index() {
    let (tmp, engine) = engine().await;
    // Indexed before: the engram is findable.
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("alpha".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(hits.to_string().contains("alpha"), "{hits}");

    let report = engine.domain_remove("eng").await.unwrap();
    assert_eq!(report["unregistered"], true);
    assert_eq!(report["files_kept"], true);

    // The files survive; the registration and the index rows do not.
    assert!(tmp.path().join("eng/alpha.md").exists());
    let listing = engine
        .list_domains(&ListDomainsParams::default())
        .await
        .unwrap();
    assert!(!listing.to_string().contains("\"eng\""), "{listing}");
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("alpha".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !hits.to_string().contains("alpha.md"),
        "index rows cleared: {hits}"
    );

    // Removing again reports the honest miss.
    assert!(engine.domain_remove("eng").await.is_err());
}
