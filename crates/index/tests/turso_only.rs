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
    // v1 initial, v2 vector chunk storage, v3 domain kind, v4 domain host lock,
    // v5 title-lower expression index.
    assert_eq!(info.schema_version, 5);
}

#[tokio::test]
async fn title_match_resolution_seeks_the_promoted_index() {
    let store = open().await;
    // Seed a domain so the query is over a real table.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "a.md",
        &engram("Alpha", "a", "engram", "", "b\n"),
    );
    sync_domain(&store, "d", dir.path()).await.unwrap();

    // The correlated title subquery shape `resolve_pending_relations` runs to
    // match a relation target by lowercased title within a domain. Without the
    // expression index this is a full engram scan per unresolved reference.
    let plan = store
        .explain_query_plan(
            "SELECT e.id FROM engram e WHERE lower(e.title) = lower('Alpha') AND e.domain_id = 1 LIMIT 1",
        )
        .await
        .unwrap();
    let joined = plan.join(" | ");
    assert!(
        joined.contains("USING INDEX") && joined.contains("idx_engram_title_lower"),
        "title match should seek the promoted index, plan was: {joined}"
    );
    assert!(
        !joined.contains("SCAN engram") || joined.contains("USING INDEX"),
        "title match should not be a bare full scan, plan was: {joined}"
    );
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
