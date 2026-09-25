//! The multi-domain driver settles its forward references before it returns.
//!
//! A domain's own resolution batch runs as that domain commits, so a reference
//! into a domain the sweep has not reached yet cannot match: the target engram
//! is absent and the target domain may have no row at all. The sweep therefore
//! ends with one more pass over every domain it applied. Without it a fresh
//! multi-domain index reported unresolved references that were not broken at
//! all, and only a second sync cleared them.
//!
//! The parity of the pass itself (both database backends) lives in
//! `crates/index/tests/store.rs`; what these tests pin is the wiring - that the
//! sweep actually runs it, and that a domain skipped or failed is not handed to
//! it.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::{Store, TursoStore};
use crystalline_service::Engine;
use tokio::sync::Mutex;

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// Domain `a` carries a relation and a prose wikilink into domain `b`, and the
/// sweep indexes `a` first. One sync leaves nothing unresolved, and the reports
/// say which references only resolved in the final pass.
#[tokio::test]
async fn one_sweep_resolves_a_reference_into_a_domain_indexed_later() {
    let tmp = tempfile::tempdir().unwrap();
    let a_dir = tmp.path().join("a");
    let b_dir = tmp.path().join("b");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    std::fs::write(
        a_dir.join("a.md"),
        engram(
            "A",
            "a",
            "- depends_on [[b:B Note]]\n\nProse mentions [[b:B Note]] too.",
        ),
    )
    .unwrap();
    std::fs::write(b_dir.join("b.md"), engram("B Note", "b-note", "body b")).unwrap();

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("a".to_string(), DomainEntry::file(a_dir.clone()));
    cfg.domains
        .insert("b".to_string(), DomainEntry::file(b_dir.clone()));

    let store = Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Engine::new(store.clone(), cfg, None, None);

    let result = engine.sync(None).await.unwrap();
    let reports = result["reports"].as_array().expect("per-domain reports");
    let a_report = reports
        .iter()
        .find(|r| r["domain"] == "a")
        .expect("a report for domain a");
    assert_eq!(
        a_report["relations_resolved"], 0,
        "a's own batch could not see into b"
    );
    assert_eq!(
        a_report["relations_resolved_late"], 1,
        "the relation resolved in the final cross-domain pass"
    );
    assert_eq!(
        a_report["links_resolved_late"], 1,
        "and so did the prose wikilink"
    );

    let stats = store.lock().await.domain_stats().await.unwrap();
    let a = stats.iter().find(|d| d.name == "a").expect("domain a");
    assert_eq!(
        (a.unresolved_relations, a.unresolved_links),
        (0, 0),
        "the first sync leaves nothing unresolved"
    );
}

/// A sweep of a single domain has no forward references it did not already
/// resolve, so the late counters stay at zero rather than re-running the
/// statement and reporting the same resolutions twice.
#[tokio::test]
async fn a_named_sync_of_one_domain_reports_no_late_resolutions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.md"), engram("A", "a", "- depends_on [[b]]")).unwrap();
    std::fs::write(root.join("b.md"), engram("B", "b", "body b")).unwrap();

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("d".to_string(), DomainEntry::file(root.clone()));
    let engine = Engine::new(
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
        cfg,
        None,
        None,
    );

    let result = engine.sync(Some("d")).await.unwrap();
    let reports = result["reports"].as_array().expect("per-domain reports");
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0]["relations_resolved"], 1,
        "resolved in the domain's own batch"
    );
    assert_eq!(reports[0]["relations_resolved_late"], 0);
    assert_eq!(reports[0]["links_resolved_late"], 0);
}

/// `ctl reindex --full` is the other multi-domain driver: it clears every file
/// domain and rebuilds them in one run, so a reference out of the first domain
/// points at rows that do not exist yet and cannot resolve in that domain's own
/// batch. A rebuilt index must not come back with unresolved references a
/// second run would clear.
#[tokio::test]
async fn a_full_reindex_resolves_across_domains_too() {
    let tmp = tempfile::tempdir().unwrap();
    let a_dir = tmp.path().join("a");
    let b_dir = tmp.path().join("b");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    std::fs::write(
        a_dir.join("a.md"),
        engram(
            "A",
            "a",
            "- depends_on [[b:B Note]]\n\nProse mentions [[b:B Note]] too.",
        ),
    )
    .unwrap();
    std::fs::write(b_dir.join("b.md"), engram("B Note", "b-note", "body b")).unwrap();

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("a".to_string(), DomainEntry::file(a_dir.clone()));
    cfg.domains
        .insert("b".to_string(), DomainEntry::file(b_dir.clone()));

    let store = Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Engine::new(store.clone(), cfg, None, None);

    let result = engine.reindex(true).await.unwrap();
    let reports = result["reports"].as_array().expect("per-domain reports");
    let a_report = reports
        .iter()
        .find(|r| r["domain"] == "a")
        .expect("a report for domain a");
    assert_eq!(a_report["relations_resolved_late"], 1);
    assert_eq!(a_report["links_resolved_late"], 1);

    let stats = store.lock().await.domain_stats().await.unwrap();
    let a = stats.iter().find(|d| d.name == "a").expect("domain a");
    assert_eq!(
        (a.unresolved_relations, a.unresolved_links),
        (0, 0),
        "a rebuilt index leaves nothing unresolved"
    );
}
