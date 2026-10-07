//! Performance: a 1000-engram domain must sync fast on the warm no-change path.
//!
//! The warm sync (no files changed) is the load-bearing requirement: it must
//! finish well under 2 seconds because it only walks, stats and compares. The
//! cold sync number is reported for the record. Run with `--nocapture` to see
//! the timings.

use std::path::Path;

use crystalline_index::{
    ChunkParams, DomainKind, Store, TursoStore, apply_scan, scan_paths, sync_domain,
};

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

fn synthetic(i: usize, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: Engram {i}\npermalink: engram-{i}\ntags:\n  - synthetic\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Engram {i}\n\n{body}\n"
    )
}

/// The relative path engram `i` is seeded at in the 5k corpus.
fn corpus_rel(i: usize) -> String {
    format!("dir{}/engram_{i}.md", i % 50)
}

/// Perf evidence for the path-targeted watcher pass: a one-file targeted sync
/// versus a full warm sync over a 5000-file domain. Not asserted (numbers vary
/// by machine), just recorded. Run with
/// `cargo test -p crystalline-index --test perf --release -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "perf evidence: run manually with --ignored --nocapture"]
async fn targeted_one_file_versus_full_warm_sync_at_5k() {
    let corpus = tempfile::tempdir().unwrap();
    let root = corpus.path();
    const N: usize = 5000;
    for i in 0..N {
        write(
            root,
            &corpus_rel(i),
            &synthetic(
                i,
                &format!("Body text for engram {i} with searchable words."),
            ),
        );
    }

    let db_dir = tempfile::tempdir().unwrap();
    let store = TursoStore::open(&db_dir.path().join("index.db"))
        .await
        .unwrap();

    let cold = std::time::Instant::now();
    let report = sync_domain(&store, "big", root).await.unwrap();
    let cold_ms = cold.elapsed().as_millis();
    assert_eq!(report.added, N, "all engrams indexed");

    let domain = store
        .upsert_domain("big", Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap();

    // Apples to apples: both routes apply exactly one edit, so the only
    // difference is how they find it - a full walk of all 5000 files versus a
    // single stat of the one dirty path. Editing one file and running a FULL
    // sync is what the watcher does today for a one-file change.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let file_a = corpus_rel(7);
    write(root, &file_a, &synthetic(7, "Edit A body."));
    let full = std::time::Instant::now();
    let full_report = sync_domain(&store, "big", root).await.unwrap();
    let full_us = full.elapsed().as_micros();
    assert_eq!(full_report.updated, 1, "full sync applied the one edit");
    assert_eq!(full_report.unchanged, N - 1, "full sync walked all 5000");

    // Edit a second file and run a TARGETED sync of just its path - the
    // walk-free route the watcher now takes for a small debounce batch.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let file_b = corpus_rel(11);
    write(root, &file_b, &synthetic(11, "Edit B body."));
    let targeted = std::time::Instant::now();
    let snapshot = store.file_stamps(domain).await.unwrap();
    let scan = scan_paths(
        "big",
        root,
        snapshot,
        vec![file_b.clone()],
        &ChunkParams::default(),
    )
    .await;
    let treport = apply_scan(&store, domain, scan).await.unwrap();
    let targeted_us = targeted.elapsed().as_micros();
    assert_eq!(treport.updated, 1, "the one edited file reindexed");
    assert_eq!(
        treport.unchanged, 0,
        "the other 4999 files were never visited"
    );

    eprintln!(
        "PERF 5k engrams, one-file edit: cold sync {cold_ms} ms | full sync {full_us} us | targeted pass {targeted_us} us | speedup {:.1}x",
        full_us as f64 / targeted_us.max(1) as f64
    );
}
