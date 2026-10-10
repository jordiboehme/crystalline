//! Evidence for `$contains` at scale: a generated domain of 50,000 engrams,
//! each citing five `sources` anchors, on the embedded backend. Every test
//! here is ignored and never a gate: the numbers depend on the machine and go
//! into the task report. Everything is written to temp dirs; nothing here
//! opens the real index. Run with
//! `cargo test -p crystalline-index --test it --release -- perf_meta:: --ignored --nocapture --test-threads=1`.

use std::path::Path;
use std::time::{Duration, Instant};

use crystalline_index::{
    ChunkParams, NoReindexHooks, RebuildKind, SearchOrder, SearchQuery, Store, TursoStore,
    meta_value_union_sql, parse_metadata_filters, reindex_domains,
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

/// The filter-only `$contains` searches an ingesting agent runs when a block
/// changed or went away, on any backend: the whole `Store::search` (the
/// count, the page and the tag hydration), unscoped and scoped to the
/// domain, as the median of 21 warm runs per case. The target is under
/// 50 ms.
async fn time_contains(store: &dyn Store, backend: &str) -> Vec<String> {
    let mut missed = Vec::new();
    let notes = Some(vec!["notes".to_string()]);
    let any = serde_json::json!({ "sources": { "$contains_any": [
        "notedown://jordi/n1/p0#b0",
        "notedown://jordi/n2/p0#b0",
        "notedown://jordi/shared/p8#b0",
    ] } });
    let any_100 = serde_json::json!({ "sources": { "$contains_any": (0..100)
        .map(|i| format!("notedown://jordi/n{i}/p0#b0"))
        .collect::<Vec<_>>() } });
    let shared = serde_json::json!({ "sources": { "$contains": "notedown://jordi/shared/p7#b0" } });
    let cases = [
        (
            "one own anchor, unscoped",
            serde_json::json!({ "sources": { "$contains": "notedown://jordi/n31337/p0#b2" } }),
            None,
            1usize,
        ),
        (
            "one anchor nobody cites (miss), unscoped",
            serde_json::json!({ "sources": { "$contains": "notedown://jordi/none/p0#b0" } }),
            None,
            0,
        ),
        ("one shared anchor, unscoped", shared.clone(), None, 50),
        ("one shared anchor, scoped", shared, notes.clone(), 50),
        ("any of three anchors, unscoped", any.clone(), None, 52),
        ("any of three anchors, scoped", any, notes.clone(), 52),
        ("any of 100 anchors, unscoped", any_100.clone(), None, 100),
        ("any of 100 anchors, scoped", any_100, notes, 100),
    ];
    for (label, wire, domains, hits) in cases {
        let query = SearchQuery {
            metadata_filters: parse_metadata_filters(&wire).unwrap(),
            domains,
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        };
        assert_eq!(store.search(&query).await.unwrap().total, hits, "{label}");
        let mut times: Vec<Duration> = Vec::with_capacity(21);
        for _ in 0..21 {
            let started = Instant::now();
            store.search(&query).await.unwrap();
            times.push(started.elapsed());
        }
        times.sort();
        let median = times[10];
        eprintln!(
            "PERF meta 50k {backend} $contains, {label}: {hits} hits, median {} us, slowest {} us",
            median.as_micros(),
            times[20].as_micros()
        );
        if median >= Duration::from_millis(50) {
            missed.push(format!(
                "{label}: the median was {median:?}, the target is under 50 ms"
            ));
        }
    }
    missed
}

/// `$contains` on the embedded backend. Turso has no counter of rows read, so
/// the page statement's plan is printed beside the hit counts.
#[tokio::test]
#[ignore = "perf evidence: run by hand with --ignored --nocapture"]
async fn contains_filter_at_50k() {
    let corpus = tempfile::tempdir().unwrap();
    generate_sourced_domain(corpus.path());
    let db = tempfile::tempdir().unwrap();
    let (store, first_ms) = index_sourced_domain(corpus.path(), db.path()).await;
    let store = store.lock().await;
    let missed = time_contains(&*store, "turso").await;

    // The predicate is the union builder the turso store issues (one value
    // gives the same text as the match builder).
    let anchor = "'\"notedown://jordi/shared/p7#b0\"'";
    let unscoped = crystalline_index::turso::filter_only_sql(
        "e.actor = ''",
        &format!("AND {}", meta_value_union_sql("?1", &["?2".to_string()])),
        SearchOrder::RecordedDesc,
        10,
        0,
    )
    .replace("?2", anchor)
    .replace("?1", "'sources'");
    let scoped = crystalline_index::turso::filter_only_sql(
        "e.actor = ''",
        &format!(
            "AND {} AND {}",
            crystalline_index::turso::domain_scope_sql("?1", true),
            meta_value_union_sql("?2", &["?3".to_string()])
        ),
        SearchOrder::RecordedDesc,
        10,
        0,
    )
    .replace("?3", anchor)
    .replace("?2", "'sources'")
    .replace("?1", "'notes'");
    // The count statement of `turso::search::filter_only`, spelled out here for
    // the report only: it runs before every page, with the same filters. This
    // copy guards nothing; the median above is what would catch a scan.
    let count = format!(
        "SELECT count(*) FROM engram e JOIN domain d ON d.id=e.domain_id WHERE e.actor = '' AND {}",
        meta_value_union_sql("'sources'", &[anchor.to_string()])
    );
    for (label, sql) in [
        ("page, unscoped", unscoped),
        ("page, scoped", scoped),
        ("count, unscoped", count),
    ] {
        let plan = store.explain_query_plan(&sql).await.unwrap();
        eprintln!("PERF meta 50k plan, {label}: {}", plan.join(" | "));
    }
    eprintln!("PERF meta 50k: first index {first_ms} ms");
    assert!(missed.is_empty(), "{missed:?}");
}

/// The same timings on Postgres, when `CRYSTALLINE_TEST_POSTGRES_URL` is set.
/// Postgres is analyzed after the index, as autovacuum would; the page plan
/// is printed as JSON.
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "perf evidence: run by hand with --ignored --nocapture"]
async fn contains_filter_at_50k_postgres() {
    let Some(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is unset");
        return;
    };
    let corpus = tempfile::tempdir().unwrap();
    generate_sourced_domain(corpus.path());
    let schema = format!("perf_meta_{}", std::process::id());
    let store = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
        .await
        .unwrap();
    let store = Mutex::new(store);
    let targets = vec![("notes".to_string(), corpus.path().to_path_buf())];
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
    let first_ms = started.elapsed().as_millis();
    assert_eq!(reports[0].added, SOURCED_ENGRAMS, "every engram is indexed");
    let store = store.into_inner();
    store.analyze().await.unwrap();
    let missed = time_contains(&store, "postgres").await;

    let anchor = "'\"notedown://jordi/shared/p7#b0\"'";
    let page = crystalline_index::postgres::filter_only_sql(
        "e.actor = ''",
        &format!(
            "AND {}",
            crystalline_index::meta_value_match_sql("'sources'", &[anchor.to_string()])
        ),
        SearchOrder::RecordedDesc,
        10,
        0,
    );
    let plan = store.explain_json(&page).await.unwrap();
    eprintln!("PERF meta 50k postgres plan, page, unscoped: {plan}");
    eprintln!("PERF meta 50k postgres: first index {first_ms} ms");
    store.drop_schema().await.unwrap();
    assert!(missed.is_empty(), "{missed:?}");
}
