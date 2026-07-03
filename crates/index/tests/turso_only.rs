//! Turso-specific store assertions that do not generalize across backends: the
//! Turso schema version, the on-disk file size and the `EXPLAIN QUERY PLAN`
//! index-seek check (a Turso-only diagnostic). The behavioral parity suite lives
//! in `store.rs` and runs against both backends.

use std::path::Path;

use crystalline_index::{Store, TursoStore, sync_domain};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, ftype: &str, extra_fm: &str, body: &str) -> String {
    format!(
        "---\ntype: {ftype}\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n{extra_fm}---\n\n# {title}\n\n{body}\n"
    )
}

async fn open() -> TursoStore {
    TursoStore::open_in_memory().await.unwrap()
}

#[tokio::test]
async fn store_info_reports_turso_schema_version() {
    let store = open().await;
    let info = store.store_info().await.unwrap();
    assert_eq!(info.fts_mode, crystalline_index::FtsMode::CandidateScan);
    assert_eq!(info.schema_version, 2);
}

#[tokio::test]
async fn temporal_current_filter_uses_the_promoted_index() {
    let store = open().await;
    // Seed a domain so the query is over a real table.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", &engram("A", "a", "engram", "", "b\n"));
    sync_domain(&store, "d", dir.path()).await.unwrap();

    let plan = store
        .explain_query_plan(
            "SELECT id FROM engram WHERE status='current' AND (valid_from IS NULL OR valid_from <= '2026-07-02') AND (valid_to IS NULL OR valid_to > '2026-07-02')",
        )
        .await
        .unwrap();
    let joined = plan.join(" | ");
    assert!(
        joined.contains("USING INDEX") && joined.contains("idx_engram_current"),
        "current filter should seek the promoted index, plan was: {joined}"
    );
    assert!(
        !joined.contains("SCAN engram") || joined.contains("USING INDEX"),
        "current filter should not be a bare full scan, plan was: {joined}"
    );
}
