//! Turso-specific store assertions that do not generalize across backends: the
//! Turso schema version, the on-disk file size and the `EXPLAIN QUERY PLAN`
//! index-seek check (a Turso-only diagnostic). The behavioral parity suite lives
//! in `store.rs` and runs against both backends.
//!
//! It is also where the query-shape guards that can be written from outside the
//! crate live - the plans over statements a test can spell in full, and the
//! source scans that keep a wide column out of an unbounded sorter. The two
//! guards over statements the store BUILDS (the reference resolve pass and the
//! lexical candidate scan) sit in `src` beside their builders instead, in
//! `turso/mod.rs` and `turso/search.rs`: they used to live here over
//! hand-copied literals, and a copy stops guarding the moment the shipped
//! statement moves, which is how the resolve pass lost its title index for a
//! whole wave while its guard stayed green.
//!
//! The folder derivation used to be guarded here the same wrong way, over a
//! hand-copied literal, and is now the `Store::browse_level` entry of the
//! registry in `tests/plans.rs`, which reads the shipped builder and keeps
//! naming `idx_engram_path_actor` in full. `temporal_current_filter_uses_the_promoted_index`
//! stays: it probes a shape nothing issues, rather than a statement the store
//! builds.

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
    // v5 title-lower expression index, v6 link unresolved partial index,
    // v7 case-folded tag identity, v8 tag alias map, v9 engram attachments,
    // v10 raw reference text, v11 domain registration stamp,
    // v12 domain rebuild marker, v13 engram actor dimension,
    // v14 domain rebuild kind.
    assert_eq!(info.schema_version, 14);
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
            "SELECT id FROM engram WHERE status IN ('stable', 'current') AND (valid_from IS NULL OR valid_from <= '2026-07-02') AND (valid_to IS NULL OR valid_to > '2026-07-02')",
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

/// A body projection never reaches an unbounded sorter.
///
/// The plan guard above covers the shape that sorts nothing; this covers the
/// ones that sort. A domain-scoped candidate query does reach a sorter, so the
/// only thing standing between it and the bodies of a whole domain is the
/// `LIMIT` in the same statement, which is what turso's bounded-sorter
/// optimization needs to hold `limit + offset` records instead of the match
/// set. So every query projecting `CANDIDATE_COLUMNS` - the candidate
/// prefilter, the filter-only page and the semantic hydrate - must either
/// order nothing or carry a bound, and none of them may grow a `GROUP BY`,
/// which takes that optimization away. This is the guard that would have
/// caught the spill this project has already paid for once.
#[test]
fn a_body_projection_never_reaches_an_unbounded_sorter() {
    let src = include_str!("../src/turso/search.rs");
    let lines: Vec<&str> = src.lines().collect();
    let mut projections = 0;
    for (n, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("//") || !line.contains("SELECT {CANDIDATE_COLUMNS}") {
            continue;
        }
        projections += 1;
        // The statement is one `format!`, so its tail is the next few lines.
        let statement = lines[n..(n + 4).min(lines.len())].join(" ");
        let sorts = statement.contains("ORDER BY") || statement.contains("GROUP BY");
        assert!(
            !sorts || statement.contains("LIMIT "),
            "search.rs:{} sorts a body projection with no bound: {statement}",
            n + 1
        );
        assert!(
            !statement.contains("GROUP BY"),
            "search.rs:{} groups over a body projection, which unbounds the sorter: {statement}",
            n + 1
        );
    }
    assert_eq!(
        projections, 3,
        "expected the candidate prefilter, the filter-only page and the semantic \
         hydrate to be the only queries projecting bodies; a new one needs its own bound"
    );
}

/// The inbound reference query must never widen its projection to a body.
///
/// A source scan rather than a plan assertion, in the style of the Postgres
/// collation guard, because the danger here is not the plan: the summary pass
/// groups over the same subquery with no `LIMIT` at all, so a `GROUP BY` over a
/// projection carrying `e.content` would be the July spill verbatim - wide
/// column, sorter, one row per reference. The plan would look fine either way.
/// Both backends are scanned, since the two shapes are twins and only one of them
/// would be caught by a Turso-only check.
#[test]
fn the_inbound_reference_query_selects_no_body() {
    for (file, src) in [
        ("turso/mod.rs", include_str!("../src/turso/mod.rs")),
        ("postgres/mod.rs", include_str!("../src/postgres/mod.rs")),
    ] {
        let Some(start) = src.find("async fn inbound_page") else {
            panic!("{file} carries no inbound_page to guard");
        };
        // To the next method at the same indent, which is where this one ends.
        let rest = &src[start..];
        let end = rest[1..]
            .find("\n    async fn ")
            .map(|p| p + 1)
            .unwrap_or(rest.len());
        for (n, line) in rest[..end].lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for body in ["e.content", "e.description", "i.content", "i.description"] {
                assert!(
                    !line.contains(body),
                    "{file}:{} selects a body into the inbound query, whose summary \
                     pass groups without a LIMIT: {line}",
                    n + 1
                );
            }
        }
    }
}
