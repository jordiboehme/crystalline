//! Performance: a 1000-engram domain must sync fast on the warm no-change path.
//!
//! The warm sync (no files changed) is the load-bearing requirement: it must
//! finish well under 2 seconds because it only walks, stats and compares. The
//! cold sync number is reported for the record. Run with `--nocapture` to see
//! the timings.

use std::path::Path;

use crystalline_index::{TursoStore, sync_domain};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn thousand_engram_warm_sync_under_two_seconds() {
    let corpus = tempfile::tempdir().unwrap();
    let root = corpus.path();
    const N: usize = 1000;
    for i in 0..N {
        // Spread across folders and give each a resolvable relation so the
        // relation-resolution batch is exercised at scale.
        let rel = format!("dir{}/engram_{i}.md", i % 20);
        let body = format!(
            "# Engram {i}\n\n- [fact] synthetic fact number {i} #synthetic\n\n- relates_to [[engram-{}]]\n\nBody text for engram {i} with some searchable words.\n",
            (i + 1) % N
        );
        write(
            root,
            &rel,
            &format!(
                "---\ntype: engram\ntitle: Engram {i}\npermalink: engram-{i}\ntags:\n  - synthetic\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}"
            ),
        );
    }

    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("index.db");
    let store = TursoStore::open(&db_path).await.unwrap();

    let cold = std::time::Instant::now();
    let report = sync_domain(&store, "big", root).await.unwrap();
    let cold_ms = cold.elapsed().as_millis();
    assert_eq!(report.added, N, "all engrams indexed");
    assert_eq!(report.failed.len(), 0, "no failures");

    let warm = std::time::Instant::now();
    let warm_report = sync_domain(&store, "big", root).await.unwrap();
    let warm_ms = warm.elapsed().as_millis();
    assert_eq!(warm_report.unchanged, N, "warm sync sees no changes");
    assert_eq!(warm_report.added, 0);
    assert_eq!(warm_report.updated, 0);

    eprintln!(
        "PERF 1k engrams: cold sync {cold_ms} ms, warm sync {warm_ms} ms (resolved {} relations)",
        report.relations_resolved
    );
    assert!(
        warm_ms < 2000,
        "warm sync must be under 2s, was {warm_ms} ms"
    );
}
