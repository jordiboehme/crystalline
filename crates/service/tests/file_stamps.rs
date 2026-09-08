//! The ctl `file_stamps` read: the index the daemon serves to a caller that
//! cannot open the index file itself, because this daemon is holding it.
//! `crystalline doctor` is that caller, and its orphan and unindexed checks
//! are only as good as what comes back here.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{self, DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use tokio::sync::Mutex;

fn engram(title: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\nBody.\n"
    )
}

fn seed(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The stamped paths of one domain in a `file_stamps` answer, sorted.
fn stamped(value: &serde_json::Value, domain: &str) -> Vec<String> {
    let mut paths: Vec<String> = value["domains"][domain]
        .as_object()
        .unwrap_or_else(|| panic!("no stamps for '{domain}' in {value}"))
        .keys()
        .cloned()
        .collect();
    paths.sort();
    paths
}

/// Both halves of what the doctor needs: the stamps of a domain this engine
/// synced, and an entry for a domain registered in the config file after the
/// engine started. Omitting the second would report every one of its files as
/// unindexed, which is exactly the false alarm a diagnosis must not raise.
#[tokio::test]
async fn stamps_cover_a_synced_domain_and_one_registered_after_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let notes = tmp.path().join("kb-notes");
    seed(&notes, "MANIFEST.md", &engram("Notes"));
    seed(&notes, "a.md", &engram("A"));

    let config_path = tmp.path().join("config.yaml");
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::file(notes.clone()));
    config::save_yaml(&config_path, &cfg).unwrap();

    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg.clone(),
        None,
        Some(config_path.clone()),
    );
    engine.sync(None).await.unwrap();

    let all = engine.domain_file_stamps(None).await.unwrap();
    assert_eq!(stamped(&all, "notes"), vec!["MANIFEST.md", "a.md"]);
    let stamp = &all["domains"]["notes"]["a.md"];
    assert_eq!(
        stamp["size"].as_u64(),
        Some(std::fs::metadata(notes.join("a.md")).unwrap().len()),
        "a stamp carries the recorded size, not just the path: {stamp}"
    );
    assert_eq!(
        stamp["sha256"].as_str().map(str::len),
        Some(64),
        "and its checksum: {stamp}"
    );

    // Registered by editing the config file, which is all `domain add` does:
    // this engine's startup snapshot has never heard of it.
    let docs = tmp.path().join("kb-docs");
    seed(&docs, "MANIFEST.md", &engram("Docs"));
    cfg.domains
        .insert("docs".to_string(), DomainEntry::file(docs.clone()));
    config::save_yaml(&config_path, &cfg).unwrap();

    let all = engine.domain_file_stamps(None).await.unwrap();
    assert_eq!(stamped(&all, "notes"), vec!["MANIFEST.md", "a.md"]);
    assert_eq!(
        stamped(&all, "docs"),
        Vec::<String>::new(),
        "a registered but never synced domain answers with no stamps, not with an absent entry"
    );

    let one = engine.domain_file_stamps(Some("docs")).await.unwrap();
    assert_eq!(
        one["domains"].as_object().unwrap().len(),
        1,
        "a named request answers for that domain alone: {one}"
    );
    assert_eq!(stamped(&one, "docs"), Vec::<String>::new());

    let refused = engine
        .domain_file_stamps(Some("nope"))
        .await
        .expect_err("a domain nobody registered is an error, not an empty answer");
    assert!(refused.to_string().contains("nope"), "{refused}");
}

/// A virtual domain has no files, so it has no stamps: an empty answer, never
/// an error. `doctor` names one domain at a time when it is filtered, and a
/// refusal there would send it back to opening the index file the daemon
/// holds.
#[tokio::test]
async fn a_virtual_domain_answers_with_nothing_rather_than_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let mut cfg = GlobalConfig::default();
    cfg.domains.insert(
        "ideas".to_string(),
        DomainEntry {
            kind: crystalline_core::config::DomainKind::Virtual,
            path: None,
            origin: None,
            provision: None,
        },
    );
    config::save_yaml(&config_path, &cfg).unwrap();

    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    );

    let answer = engine.domain_file_stamps(Some("ideas")).await.unwrap();
    assert!(
        answer["domains"].as_object().unwrap().is_empty(),
        "a virtual domain contributes no stamps: {answer}"
    );
}
