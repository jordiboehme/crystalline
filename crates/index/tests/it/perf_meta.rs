//! Evidence for `$contains` at scale: a generated domain of 50,000 engrams,
//! each citing five `sources` anchors, on the embedded backend. Every test
//! here is ignored and never a gate: the numbers depend on the machine and go
//! into the task report. Everything is written to temp dirs; nothing here
//! opens the real index. Run with
//! `cargo test -p crystalline-index --test it --release -- perf_meta:: --ignored --nocapture --test-threads=1`.

use std::path::Path;
use std::time::Instant;

use crystalline_index::{
    ChunkParams, NoReindexHooks, RebuildKind, Store, TursoStore, reindex_domains,
};
use tokio::sync::Mutex;

/// How many engrams the generated domain holds.
const SOURCED_ENGRAMS: usize = 50_000;

/// Engram `i`: four anchors of its own and one it shares with every engram
/// whose number has the same remainder by 1000, so an own anchor is cited by
/// one engram and a shared one by 50.
fn sourced_engram(i: usize) -> String {
    let mut anchors = String::new();
    for b in 0..4 {
        anchors.push_str(&format!("  - notedown://jordi/n{i}/p0#b{b}\n"));
    }
    anchors.push_str(&format!("  - notedown://jordi/shared/p{}#b0\n", i % 1000));
    format!(
        "---\ntype: engram\ntitle: Note {i}\npermalink: note-{i}\ntags:\n  - ocr\nstatus: current\nrecorded_at: 2026-01-01\nsources:\n{anchors}---\n\n# Note {i}\n\nThe text of note {i}.\n"
    )
}

/// Write the generated domain under `root`, a hundred engrams per folder.
fn generate_sourced_domain(root: &Path) {
    for i in 0..SOURCED_ENGRAMS {
        let dir = root.join(format!("dir{}", i % 500));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("note-{i}.md")), sourced_engram(i)).unwrap();
    }
}

/// Index the generated domain into a fresh store on disk under `db_dir`, and
/// say how long that first index took, in milliseconds.
async fn index_sourced_domain(root: &Path, db_dir: &Path) -> (Mutex<TursoStore>, u128) {
    let store = Mutex::new(TursoStore::open(&db_dir.join("index.db")).await.unwrap());
    let targets = vec![("notes".to_string(), root.to_path_buf())];
    let started = Instant::now();
    let reports = {
        let locked: &Mutex<dyn Store> = &store;
        reindex_domains(
            locked,
            &targets,
            &ChunkParams::default(),
            None,
            &NoReindexHooks,
        )
        .await
        .unwrap()
    };
    let ms = started.elapsed().as_millis();
    assert_eq!(reports[0].added, SOURCED_ENGRAMS, "every engram is indexed");
    assert!(
        reports[0].failed.is_empty(),
        "no failures: {:?}",
        reports[0].failed
    );
    (store, ms)
}

/// The write cost: a full reindex of the generated domain, which re-reads
/// and rewrites every engram. Run on the commit before the side table (the
/// BASE) and again after it; both lines go into the task report.
#[tokio::test]
#[ignore = "perf evidence: run by hand with --ignored --nocapture"]
async fn full_reindex_cost_at_50k() {
    let corpus = tempfile::tempdir().unwrap();
    generate_sourced_domain(corpus.path());
    let db = tempfile::tempdir().unwrap();
    let (store, first_ms) = index_sourced_domain(corpus.path(), db.path()).await;
    let targets = vec![("notes".to_string(), corpus.path().to_path_buf())];
    let started = Instant::now();
    let reports = {
        let locked: &Mutex<dyn Store> = &store;
        reindex_domains(
            locked,
            &targets,
            &ChunkParams::default(),
            Some(RebuildKind::Full),
            &NoReindexHooks,
        )
        .await
        .unwrap()
    };
    let full_ms = started.elapsed().as_millis();
    assert!(
        reports[0].failed.is_empty(),
        "no failures: {:?}",
        reports[0].failed
    );
    eprintln!(
        "PERF meta 50k: first index {first_ms} ms, full reindex {full_ms} ms, {:.1} us per engram ({} updated, {} unchanged)",
        full_ms as f64 * 1000.0 / SOURCED_ENGRAMS as f64,
        reports[0].updated,
        reports[0].unchanged,
    );
}
