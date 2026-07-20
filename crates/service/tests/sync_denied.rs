//! The full sweep contains a domain whose root cannot be read: one denied
//! domain lands in `failed`, its neighbor still syncs and a named sync of the
//! denied domain surfaces the error rather than silently emptying its index.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use tokio::sync::Mutex;

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

fn seed_domain(dir: &std::path::Path, token: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("a.md"), engram("A", "a", token)).unwrap();
}

#[tokio::test]
async fn full_sweep_continues_past_a_denied_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let a_dir = tmp.path().join("a");
    let b_dir = tmp.path().join("b");
    seed_domain(&a_dir, "alpha body");
    seed_domain(&b_dir, "beta body");

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("a".to_string(), DomainEntry::file(a_dir.clone()));
    cfg.domains
        .insert("b".to_string(), DomainEntry::file(b_dir.clone()));

    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, None);

    // Both domains sync cleanly the first time.
    engine.sync(None).await.unwrap();

    // Deny domain A's root so its walk cannot read its entries.
    std::fs::set_permissions(&a_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    // The full sweep must contain A's failure and still report B; a named sync of
    // the denied domain must surface the error.
    let swept = engine.sync(None).await;
    let named = engine.sync(Some("a")).await;

    // Restore before any assertion can unwind past the tempdir drop.
    std::fs::set_permissions(&a_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    let swept = swept.expect("a full sweep past a denied domain is Ok");
    let failed = swept["failed"]
        .as_array()
        .expect("the sweep result carries a failed array");
    assert!(
        failed.iter().any(|f| f["domain"] == "a"
            && f["error"]
                .as_str()
                .unwrap_or_default()
                .contains("io error at")),
        "A is listed under failed with an io error: {swept}"
    );
    let reports = swept["reports"]
        .as_array()
        .expect("the sweep result carries a reports array");
    assert!(
        reports.iter().any(|r| r["domain"] == "b"),
        "B still synced during the sweep: {swept}"
    );

    let named = named.expect_err("a named sync of a denied domain must error");
    assert!(
        named.to_string().contains("io error at"),
        "the named sync error carries the io error: {named}"
    );
}
