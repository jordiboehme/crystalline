//! The engine keeps one name table over every registered domain and pushes its
//! spellings into the index, so a domain answers to its MANIFEST `domain_name`
//! as well as to its local name: in the tool parameters (through
//! `Engine::local_domain_name`) and in the references content carries (through
//! the index's `domain_spelling` table).
//!
//! The table follows the MANIFEST: a sync, an edit of a file domain's MANIFEST
//! and an edit of a virtual domain's MANIFEST engram each rebuild it and push
//! what changed. The push compares before it writes, so an unchanged table
//! costs no write and two instances sharing one index do not take a contested
//! spelling from each other on every refresh.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::{Store, TursoStore};
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{EditParams, ReadParams, WriteParams};
use serde_json::Value;
use tokio::sync::Mutex;

const TODAY: &str = "2026-01-01";

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: {TODAY}\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A file domain folder under `dir` holding a MANIFEST that declares
/// `domain_name` (the template writes it) and the given engrams.
fn file_domain(
    dir: &Path,
    folder: &str,
    declared: &str,
    engrams: &[(&str, String)],
) -> DomainEntry {
    let root = dir.join(folder);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("MANIFEST.md"),
        crystalline_core::manifest_template(declared, TODAY),
    )
    .unwrap();
    for (file, text) in engrams {
        std::fs::write(root.join(file), text).unwrap();
    }
    DomainEntry::file(root)
}

type SharedStore = Arc<Mutex<dyn Store>>;

async fn memory_store() -> SharedStore {
    Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()))
}

/// An engine over `domains`, reading its configuration file from `dir` (which
/// holds none), so no test ever reads the developer's real configuration.
fn engine(store: SharedStore, dir: &Path, domains: Vec<(&str, DomainEntry)>) -> Engine {
    let mut cfg = GlobalConfig::default();
    for (name, entry) in domains {
        cfg.domains.insert(name.to_string(), entry);
    }
    Engine::new(store, cfg, None, Some(dir.join("config.yaml"))).with_state_dir(dir.join("state"))
}

async fn read(engine: &Engine, domain: &str, identifier: &str) -> Value {
    engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some(domain.to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// Whether the one prose wikilink of an engram resolved, as the read
/// receipt reports it.
async fn link_resolved(engine: &Engine, domain: &str, identifier: &str) -> bool {
    let read = read(engine, domain, identifier).await;
    let links = read["links"].as_array().expect("a links array");
    assert_eq!(links.len(), 1, "one link in {identifier}: {links:?}");
    links[0]["resolved"].as_bool().unwrap()
}

fn replace_manifest_name(domain: &str, from: &str, to: &str) -> EditParams {
    EditParams {
        identifier: "manifest".to_string(),
        domain: domain.to_string(),
        operation: "find_replace".to_string(),
        find_text: Some(format!("domain_name: {from}")),
        content: Some(format!("domain_name: {to}")),
        expected_replacements: Some(1),
        ..EditParams::default()
    }
}

/// `eng-knowledge` declares `domain_name: eng`; after one sync the name
/// resolves to it and a link spelled with it binds.
#[tokio::test]
async fn a_manifest_domain_name_resolves_after_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = file_domain(
        tmp.path(),
        "eng-knowledge",
        "eng",
        &[("runbook.md", engram("Runbook", "runbook", "the steps"))],
    );
    let ops = file_domain(
        tmp.path(),
        "ops",
        "ops",
        &[(
            "oncall.md",
            engram("Oncall", "oncall", "Follow [[eng:runbook]] first."),
        )],
    );
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("eng-knowledge", eng), ("ops", ops)],
    );

    engine.sync(None).await.unwrap();

    assert_eq!(
        engine.local_domain_name("eng").as_deref(),
        Some("eng-knowledge")
    );
    assert_eq!(
        engine.local_domain_name("eng-knowledge").as_deref(),
        Some("eng-knowledge")
    );
    assert_eq!(engine.local_domain_name("nowhere"), None);
    assert_eq!(engine.localize("eng"), "eng-knowledge");
    assert_eq!(engine.localize("nowhere"), "nowhere");
    assert!(
        link_resolved(&engine, "ops", "oncall").await,
        "a link spelled with the canonical name binds"
    );
}

/// Editing the MANIFEST to `domain_name: platform` moves the name: the old
/// one resolves nowhere, a link spelled with it unbinds, and a link written
/// with the new one binds.
#[tokio::test]
async fn a_manifest_edit_changes_the_table() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = file_domain(
        tmp.path(),
        "eng-knowledge",
        "eng",
        &[("runbook.md", engram("Runbook", "runbook", "the steps"))],
    );
    let ops = file_domain(
        tmp.path(),
        "ops",
        "ops",
        &[(
            "oncall.md",
            engram("Oncall", "oncall", "Follow [[eng:runbook]] first."),
        )],
    );
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("eng-knowledge", eng), ("ops", ops)],
    );
    engine.sync(None).await.unwrap();
    assert!(link_resolved(&engine, "ops", "oncall").await);

    engine
        .edit_engram(&replace_manifest_name("eng-knowledge", "eng", "platform"))
        .await
        .unwrap();

    assert_eq!(
        engine.local_domain_name("platform").as_deref(),
        Some("eng-knowledge")
    );
    assert_eq!(engine.local_domain_name("eng"), None);
    assert!(
        !link_resolved(&engine, "ops", "oncall").await,
        "a link spelled with the former canonical name no longer binds"
    );

    engine
        .write_engram(&WriteParams {
            domain: "ops".to_string(),
            title: "Escalation".to_string(),
            content: "Then [[platform:runbook]].".to_string(),
            folder: None,
            engram_type: None,
            tags: Vec::new(),
            status: None,
            metadata: None,
            overwrite: false,
            share_link: None,
            model: None,
        })
        .await
        .unwrap();
    assert!(
        link_resolved(&engine, "ops", "escalation").await,
        "a link spelled with the new canonical name binds"
    );
}

/// A virtual domain's MANIFEST engram declares its name the same way.
#[tokio::test]
async fn a_virtual_domain_declares_its_name_too() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let engine = engine(
        store.clone(),
        tmp.path(),
        vec![("scratch", DomainEntry::virtual_domain())],
    );
    engine.domain_add_virtual("scratch").await.unwrap();
    assert_eq!(engine.local_domain_name("notes"), None);

    engine
        .edit_engram(&replace_manifest_name("scratch", "scratch", "notes"))
        .await
        .unwrap();

    assert_eq!(
        engine.local_domain_name("notes").as_deref(),
        Some("scratch")
    );
    let id = store
        .lock()
        .await
        .domain_id("scratch")
        .await
        .unwrap()
        .expect("the virtual domain has a row");
    let spellings = store.lock().await.domain_spellings().await.unwrap();
    assert!(
        spellings.contains(&("notes".to_string(), id)),
        "the index resolves the declared name too: {spellings:?}"
    );
}

/// A refresh whose table matches what the index already holds writes
/// nothing: the comparison runs before the replace, not inside it.
#[tokio::test]
async fn an_unchanged_table_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = file_domain(tmp.path(), "eng-knowledge", "eng", &[]);
    let engine = engine(
        memory_store().await,
        tmp.path(),
        vec![("eng-knowledge", eng)],
    );
    engine.sync(None).await.unwrap();
    let after_sync = engine.spelling_replaces_issued();
    assert_eq!(after_sync, 1, "the first push records the canonical name");

    engine.refresh_names().await;
    engine.sync(None).await.unwrap();
    engine.refresh_names().await;
    assert_eq!(engine.spelling_replaces_issued(), after_sync);
}

/// Two instances over one index, each registering its own domain that
/// declares the same canonical name. The first to push holds it; neither
/// takes it from the other on later refreshes.
#[tokio::test]
async fn instances_sharing_an_index_do_not_flip_a_contested_name() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let a_dir = tmp.path().join("a-home");
    let b_dir = tmp.path().join("b-home");
    let a = engine(
        store.clone(),
        &a_dir,
        vec![("alpha", file_domain(tmp.path(), "alpha", "shared", &[]))],
    );
    let b = engine(
        store.clone(),
        &b_dir,
        vec![("beta", file_domain(tmp.path(), "beta", "shared", &[]))],
    );
    a.sync(None).await.unwrap();
    b.sync(None).await.unwrap();
    let settled = store.lock().await.domain_spellings().await.unwrap();
    let (a_pushes, b_pushes) = (a.spelling_replaces_issued(), b.spelling_replaces_issued());

    for _ in 0..2 {
        a.refresh_names().await;
        b.refresh_names().await;
    }

    assert_eq!(
        store.lock().await.domain_spellings().await.unwrap(),
        settled,
        "the contested spelling stays where it is"
    );
    assert_eq!(a.spelling_replaces_issued(), a_pushes);
    assert_eq!(b.spelling_replaces_issued(), b_pushes);
}

/// A canonical name equal to the own name of a domain row this instance no
/// longer registers can never land, because the replace re-claims every
/// row's own name last. The comparison knows it and does not retry on every
/// refresh.
#[tokio::test]
async fn a_name_held_by_an_unregistered_row_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let store = memory_store().await;
    let before = engine(
        store.clone(),
        &tmp.path().join("before"),
        vec![("eng", file_domain(tmp.path(), "old-eng", "eng", &[]))],
    );
    before.sync(None).await.unwrap();

    let after = engine(
        store.clone(),
        &tmp.path().join("after"),
        vec![(
            "eng-knowledge",
            file_domain(tmp.path(), "eng-knowledge", "eng", &[]),
        )],
    );
    after.sync(None).await.unwrap();
    let pushes = after.spelling_replaces_issued();
    after.refresh_names().await;
    after.refresh_names().await;
    assert_eq!(after.spelling_replaces_issued(), pushes);
}
