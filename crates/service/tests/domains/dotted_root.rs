//! A domain whose folder sits under a hidden parent, as every domain does
//! under the default root `~/.crystalline`. The walks classify a path
//! relative to the domain root, so the dot in a parent folder must never
//! make a file look hidden: sync indexes every engram, the folder listings
//! are generated, and validate and export see every file.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{ReadParams, ValidateParams};
use tokio::sync::Mutex;

const MANIFEST: &str = "---\ntype: manifest\ntitle: dom\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-11-25\n---\n\n# dom\n\n## Scope\n\n- Hidden root checks\n\n## When to Use\n\n- Route here for the hidden root\n";

fn note(title: &str, permalink: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\nstatus: stable\nrecorded_at: 2026-11-25\n---\n\n# {title}\n\nA note about {title}.\n"
    )
}

/// A domain `dom` at `<tmp>/.crystalline/dom`, synced once. The temp folder's
/// own name carries no dot, so the only hidden segment is the parent of the
/// domain root.
async fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Arc<Engine>) {
    // `tempfile` names its folders `.tmpXXXX`; this one carries no dot.
    let tmp = tempfile::Builder::new().prefix("dotted").tempdir().unwrap();
    assert!(
        !tmp.path()
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.')),
        "{}",
        tmp.path().display()
    );
    let dir = tmp.path().join(".crystalline").join("dom");
    let files = [
        ("MANIFEST.md", MANIFEST.to_string()),
        ("alpha.md", note("Alpha", "alpha")),
        ("topics/beta.md", note("Beta", "topics/beta")),
        ("topics/deep/gamma.md", note("Gamma", "topics/deep/gamma")),
    ];
    for (rel, body) in files {
        let abs = dir.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
    }
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("dom".to_string(), DomainEntry::file(dir.clone()));
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(tmp.path().join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, dir, engine)
}

async fn readable(engine: &Engine, identifier: &str) -> bool {
    engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some("dom".to_string()),
                share_link: None,
                include_images: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .is_ok()
}

#[tokio::test]
async fn sync_indexes_every_engram_under_a_hidden_parent() {
    let (_tmp, dir, engine) = fixture().await;
    for id in ["alpha", "topics/beta", "topics/deep/gamma"] {
        assert!(readable(&engine, id).await, "{id} is indexed");
    }
    assert!(
        dir.join("index.md").is_file(),
        "the root listing is generated"
    );
    assert!(
        dir.join("topics/index.md").is_file(),
        "a folder listing is generated"
    );
}

#[tokio::test]
async fn validate_checks_every_engram_under_a_hidden_parent() {
    let (_tmp, _dir, engine) = fixture().await;
    let report = engine
        .validate_engrams(
            &ValidateParams {
                domain: "dom".to_string(),
                identifier: None,
                engram_type: None,
                drift: false,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    // The MANIFEST and the three notes.
    assert_eq!(report["checked"], 4, "{report}");
}

#[tokio::test]
async fn export_writes_every_file_under_a_hidden_parent() {
    let (tmp, _dir, engine) = fixture().await;
    let dest = tmp.path().join("export");
    let report = engine
        .export_domain("dom", &dest, false, false)
        .await
        .unwrap();
    let paths: Vec<&str> = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    for rel in [
        "MANIFEST.md",
        "alpha.md",
        "topics/beta.md",
        "topics/deep/gamma.md",
    ] {
        assert!(paths.contains(&rel), "{rel} in {paths:?}");
        assert!(Path::new(&dest).join(rel).is_file(), "{rel} written");
    }
}
