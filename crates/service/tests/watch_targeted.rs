//! Engine-level coverage for the watcher's path-targeted sync,
//! [`Engine::sync_paths`]: the pass the debounce flush runs over just the dirty
//! paths of a domain instead of a full rescan.
//!
//! The watcher's own accumulation and escalation decision (DirtyPaths, the 256
//! cap, classify_event, the rescan fallback) is unit-tested in the daemon module
//! directly, because driving the live notify loop against real filesystem events
//! is not deterministic in a harness. What these tests pin is the other half:
//! that `sync_paths` over one path reindexes exactly that path and leaves every
//! other row untouched, while a full `sync` remains the fallback the watcher
//! escalates to.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::params::SearchParams;
use tokio::sync::Mutex;

fn manifest() -> String {
    "---\ntype: manifest\ntitle: Notes\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Notes\n\n## Scope\n\n- notes\n\n## When to Use\n\n- always\n".to_string()
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

async fn search_total(engine: &Engine, query: &str) -> u64 {
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some(query.to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    hits["total"].as_u64().unwrap()
}

/// A one-file edit synced through `sync_paths` reindexes exactly that file: the
/// report counts one update and zero unchanged (the walk-free marker - a full
/// scan would count the untouched files as unchanged), and every other engram
/// survives verbatim in the index.
#[tokio::test]
async fn sync_paths_reindexes_only_the_targeted_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();
    for i in 0..5 {
        std::fs::write(
            root.join(format!("f{i}.md")),
            engram(
                &format!("F{i}"),
                &format!("f{i}"),
                &format!("body{i} token"),
            ),
        )
        .unwrap();
    }

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::file(root.to_path_buf()));
    let engine = Engine::new(
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
        cfg,
        None,
        None,
    );

    // Full sync seeds every engram.
    engine.sync(None).await.unwrap();
    assert_eq!(search_total(&engine, "body2").await, 1);

    // Edit f2.md on disk; bump the mtime past the prefilter.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(root.join("f2.md"), engram("F2", "f2", "revised token")).unwrap();

    // Targeted pass over just that path.
    let report = engine
        .sync_paths("notes", vec!["f2.md".to_string()])
        .await
        .unwrap();
    assert_eq!(report.domain, "notes");
    assert_eq!(report.updated, 1, "exactly the edited file reindexed");
    assert_eq!(report.added, 0);
    assert_eq!(report.deleted, 0);
    assert_eq!(
        report.unchanged, 0,
        "unchanged counts only the given path, never the unvisited others"
    );

    // The edit landed and the other four engrams survive untouched.
    assert_eq!(search_total(&engine, "revised").await, 1);
    for i in [0u32, 1, 3, 4] {
        assert_eq!(
            search_total(&engine, &format!("body{i}")).await,
            1,
            "the untouched f{i}.md row survives"
        );
    }
}

/// A new file and a deleted file, each synced through `sync_paths` by its own
/// path, are added and removed without walking the rest of the domain.
#[tokio::test]
async fn sync_paths_handles_a_targeted_add_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(root.join("keep.md"), engram("Keep", "keep", "keep token")).unwrap();
    std::fs::write(root.join("drop.md"), engram("Drop", "drop", "drop token")).unwrap();

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::file(root.to_path_buf()));
    let engine = Engine::new(
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
        cfg,
        None,
        None,
    );
    engine.sync(None).await.unwrap();

    // Add fresh.md and target it.
    std::fs::write(
        root.join("fresh.md"),
        engram("Fresh", "fresh", "fresh token"),
    )
    .unwrap();
    let added = engine
        .sync_paths("notes", vec!["fresh.md".to_string()])
        .await
        .unwrap();
    assert_eq!(added.added, 1);
    assert_eq!(search_total(&engine, "fresh").await, 1);

    // Delete drop.md and target it.
    std::fs::remove_file(root.join("drop.md")).unwrap();
    let deleted = engine
        .sync_paths("notes", vec!["drop.md".to_string()])
        .await
        .unwrap();
    assert_eq!(deleted.deleted, 1);
    assert_eq!(search_total(&engine, "drop").await, 0);

    // keep.md was never a target and is still there.
    assert_eq!(search_total(&engine, "keep").await, 1);
}

/// A targeted sync of an unknown or virtual domain is a clean no-op rather than
/// an error, so a stray watcher event never fails the flush.
#[tokio::test]
async fn sync_paths_on_a_virtual_domain_is_a_noop() {
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("v".to_string(), DomainEntry::virtual_domain());
    let engine = Engine::new(
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
        cfg,
        None,
        None,
    );

    let report = engine
        .sync_paths("v", vec!["anything.md".to_string()])
        .await
        .unwrap();
    assert_eq!(report.added, 0);
    assert_eq!(report.updated, 0);
    assert_eq!(report.deleted, 0);
    assert_eq!(report.unchanged, 0);
}
