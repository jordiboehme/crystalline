//! Evidence for `$contains` at scale: a generated domain of 50,000 engrams,
//! each citing five `sources` anchors, on the embedded backend. Every test
//! here is ignored and never a gate: the numbers depend on the machine and go
//! into the task report. Everything is written to temp dirs; nothing here
//! opens the real index. Run with
//! `cargo test -p crystalline-index --test it --release -- perf_meta:: --ignored --nocapture --test-threads=1`.

use std::path::Path;
use std::time::{Duration, Instant};

use crystalline_index::ReadStore;
use crystalline_index::{
    ChunkParams, DomainKind, NoReindexHooks, RebuildKind, SearchOrder, SearchQuery, Store,
    SyncReport, TursoStore, apply_scan, meta_value_union_sql, parse_metadata_filters,
    reindex_domains, reparse_due, scan_domain,
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
    generate_sourced_domain_of(root, SOURCED_ENGRAMS);
}

/// How many engrams the reindex timings generate: `CRYSTALLINE_TEST_PERF_ENGRAMS`
/// when set, else [`SOURCED_ENGRAMS`]. A knob for the scaling series (5,000,
/// 20,000, 50,000): a cost that grows with the square of the count is a scan,
/// a cost that grows with the count is per-row work.
fn perf_engrams() -> usize {
    std::env::var("CRYSTALLINE_TEST_PERF_ENGRAMS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(SOURCED_ENGRAMS)
}

/// [`generate_sourced_domain`] with a count of its own.
fn generate_sourced_domain_of(root: &Path, count: usize) {
    for i in 0..count {
        let dir = root.join(format!("dir{}", i % 500));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("note-{i}.md")), sourced_engram(i)).unwrap();
    }
}

/// One pass over the domain in the phases `reindex_domains` runs, timed one by
/// one: the stamps window, the walk and hash, the apply transaction and the tail
/// checkpoint. The same calls in the same order as the driver, so the total is
/// the driver's own cost for one domain (whose cross-domain pass is a no-op).
async fn timed_pass(
    store: &Mutex<dyn Store>,
    name: &str,
    root: &Path,
    rebuild: Option<RebuildKind>,
) -> (SyncReport, String) {
    let started = Instant::now();
    let (domain, snapshot, force) = {
        let locked = store.lock().await;
        let domain = locked
            .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
            .await
            .unwrap();
        if let Some(kind) = rebuild {
            locked
                .begin_rebuild(domain, &chrono::Utc::now().to_rfc3339(), kind)
                .await
                .unwrap();
        }
        // Forced as `reindex_domains` forces it: a rebuild, or a domain whose
        // rows an older parser derived (a new domain is at generation 0).
        let force = rebuild.is_some() || reparse_due(&*locked, domain).await.unwrap();
        (domain, locked.file_stamps(domain).await.unwrap(), force)
    };
    let stamps = started.elapsed();
    let scan = scan_domain(name, root, snapshot, &ChunkParams::default(), force)
        .await
        .unwrap();
    let scanned = started.elapsed();
    let report = {
        let locked = store.lock().await;
        apply_scan(&*locked, domain, scan).await.unwrap()
    };
    let applied = started.elapsed();
    {
        let locked = store.lock().await;
        locked.checkpoint_wal().await.unwrap();
    }
    let total = started.elapsed();
    let line = format!(
        "stamps {} ms, walk and hash {} ms, apply {} ms, checkpoint {} ms, total {} ms \
         ({} added, {} updated)",
        stamps.as_millis(),
        (scanned - stamps).as_millis(),
        (applied - scanned).as_millis(),
        (total - applied).as_millis(),
        total.as_millis(),
        report.added,
        report.updated,
    );
    (report, line)
}

/// A distinct schema per run: the pid keeps runs apart, the counter keeps
/// tests within a run apart, and the hash salt keeps a recycled pid from
/// adopting a schema a panicking run left behind.
#[cfg(feature = "postgres")]
fn perf_schema() -> String {
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "perf_meta_{}_{}_{:x}",
        std::process::id(),
        n,
        RandomState::new().hash_one(n)
    )
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
/// `CRYSTALLINE_TEST_PERF_ENGRAMS` runs it at another size.
#[tokio::test]
#[ignore = "perf evidence: run by hand with --ignored --nocapture"]
async fn full_reindex_cost_at_50k() {
    let count = perf_engrams();
    let corpus = tempfile::tempdir().unwrap();
    generate_sourced_domain_of(corpus.path(), count);
    let db = tempfile::tempdir().unwrap();
    let store = Mutex::new(TursoStore::open(&db.path().join("index.db")).await.unwrap());
    let locked: &Mutex<dyn Store> = &store;
    let (first, first_line) = timed_pass(locked, "notes", corpus.path(), None).await;
    assert_eq!(first.added, count, "every engram is indexed");
    assert!(first.failed.is_empty(), "no failures: {:?}", first.failed);
    let (full, full_line) =
        timed_pass(locked, "notes", corpus.path(), Some(RebuildKind::Full)).await;
    assert_eq!(full.updated, count, "every engram is rewritten");
    assert!(full.failed.is_empty(), "no failures: {:?}", full.failed);
    eprintln!("PERF reindex turso {count}: first index: {first_line}");
    eprintln!("PERF reindex turso {count}: full reindex: {full_line}");
}

/// [`full_reindex_cost_at_50k`] on Postgres, when `CRYSTALLINE_TEST_POSTGRES_URL`
/// is set, in a schema of its own so two runs never share one.
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "perf evidence: run by hand with --ignored --nocapture"]
async fn full_reindex_cost_at_50k_postgres() {
    let Some(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is unset");
        return;
    };
    let count = perf_engrams();
    let corpus = tempfile::tempdir().unwrap();
    generate_sourced_domain_of(corpus.path(), count);
    let schema = perf_schema();
    let store = Mutex::new(
        crystalline_index::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap(),
    );
    let (first, first_line, full, full_line) = {
        let locked: &Mutex<dyn Store> = &store;
        let (first, first_line) = timed_pass(locked, "notes", corpus.path(), None).await;
        let (full, full_line) =
            timed_pass(locked, "notes", corpus.path(), Some(RebuildKind::Full)).await;
        (first, first_line, full, full_line)
    };
    // Read everything the asserts need, then drop the schema, so a failed
    // assert below never leaves it behind.
    eprintln!("PERF reindex postgres {count} ({schema}): first index: {first_line}");
    eprintln!("PERF reindex postgres {count} ({schema}): full reindex: {full_line}");
    if std::env::var("CRYSTALLINE_TEST_PERF_KEEP").is_ok() {
        eprintln!("PERF kept schema {schema}");
    } else {
        store.into_inner().drop_schema().await.unwrap();
    }
    assert!(first.failed.is_empty(), "no failures: {:?}", first.failed);
    assert_eq!(first.added, count, "every engram is indexed");
    assert!(full.failed.is_empty(), "no failures: {:?}", full.failed);
    assert_eq!(full.updated, count, "every engram is rewritten");
}

/// The scoped lexical candidate statement on Postgres at 20,000 engrams, in the
/// two shapes that decide its plan: the scoped domain as half of the table and
/// as a tenth of it, each with a term in every body and a term in one body.
/// All three domains are indexed through the write path and analyzed. The
/// schema is kept when `CRYSTALLINE_TEST_PERF_KEEP` is set, so the statements
/// can be run with `EXPLAIN (ANALYZE, BUFFERS)` from psql; the test
/// itself prints the plain plans and the timing of the real search.
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "perf evidence: run by hand with --ignored --nocapture"]
async fn scoped_lexical_plan_at_20k_postgres() {
    let Some(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is unset");
        return;
    };
    // 20,000 engrams in three domains: `notes` is half of the table, `small`
    // a tenth of it and the last one written, so its ids come after every
    // other row (the worst case for a walk in id order).
    let layout: [(&str, usize); 3] = [("notes", 10_000), ("other", 8_000), ("small", 2_000)];
    let schema = perf_schema();
    let store = Mutex::new(
        crystalline_index::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap(),
    );
    let mut corpora = Vec::new();
    for (name, count) in layout {
        let corpus = tempfile::tempdir().unwrap();
        generate_sourced_domain_of(corpus.path(), count);
        let locked: &Mutex<dyn Store> = &store;
        let (report, _) = timed_pass(locked, name, corpus.path(), None).await;
        assert_eq!(report.added, count);
        corpora.push(corpus);
    }
    let store = store.into_inner();
    store.analyze().await.unwrap();
    // One word per case, so the store's statement carries exactly one term
    // clause, the one built below. `text` is in every body; `9999` is in one
    // engram of `notes`, `1999` in one engram of `small`.
    for (scope, term, label) in [
        ("notes", "text", "half, common term"),
        ("notes", "9999", "half, rare term"),
        ("small", "text", "tenth, common term"),
        ("small", "1999", "tenth, rare term"),
    ] {
        let query = crystalline_index::SearchQuery {
            text: Some(term.to_string()),
            domains: Some(vec![scope.to_string()]),
            limit: 10,
            page: 1,
            ..crystalline_index::SearchQuery::default()
        };
        let mut times = Vec::with_capacity(11);
        for _ in 0..11 {
            let started = Instant::now();
            store.search(&query).await.unwrap();
            times.push(started.elapsed());
        }
        times.sort();
        let filters = format!(
            "AND d.name IN ('{scope}') AND (lower(e.title) LIKE '%{term}%' ESCAPE '\\' \
             OR lower(e.description) LIKE '%{term}%' ESCAPE '\\' \
             OR lower(ec.content) LIKE '%{term}%' ESCAPE '\\')"
        );
        // The screen `actor_screen` writes for a search with no actor.
        let sql = crystalline_index::postgres::lexical_candidate_sql(
            "e.actor = ''",
            &filters,
            crystalline_index::LEXICAL_CANDIDATE_CAP,
        );
        let plan = store.explain_json(&sql).await.unwrap();
        eprintln!(
            "PERF lexical postgres 20k, {label}: search median {} us; plan: {plan}",
            times[5].as_micros()
        );
        eprintln!("PERF lexical postgres 20k, {label}: statement: {sql}");
    }
    if std::env::var("CRYSTALLINE_TEST_PERF_KEEP").is_ok() {
        eprintln!("PERF kept schema {schema}");
    } else {
        store.drop_schema().await.unwrap();
    }
}

/// The filter-only `$contains` searches an ingesting agent runs when a block
/// changed or went away, on any backend: the whole `ReadStore::search` (the
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
    // The count statement `turso::search::filter_only` runs before every page,
    // with the same filters, from the store's own builder.
    let count = crystalline_index::turso::filter_only_count_sql(
        "e.actor = ''",
        &format!(
            "AND {}",
            meta_value_union_sql("'sources'", &[anchor.to_string()])
        ),
    );
    // The 100-value statements, scoped and unscoped, page and count: the case
    // that walked the domain before the scope stepped aside.
    let hundred: Vec<String> = (0..100)
        .map(|i| format!("'\"notedown://jordi/n{i}/p0#b0\"'"))
        .collect();
    let any_100 = meta_value_union_sql("'sources'", &hundred);
    let scoped_100 = format!(
        "AND {} AND {any_100}",
        crystalline_index::turso::domain_scope_sql("'notes'", true)
    );
    let unscoped_100 = format!("AND {any_100}");
    let page_100 = |filters: &str| {
        crystalline_index::turso::filter_only_sql(
            "e.actor = ''",
            filters,
            SearchOrder::RecordedDesc,
            10,
            0,
        )
    };
    for (label, sql) in [
        ("page, unscoped", unscoped),
        ("page, scoped", scoped),
        ("count, unscoped", count),
        ("100 values, page, unscoped", page_100(&unscoped_100)),
        ("100 values, page, scoped", page_100(&scoped_100)),
        (
            "100 values, count, unscoped",
            crystalline_index::turso::filter_only_count_sql("e.actor = ''", &unscoped_100),
        ),
        (
            "100 values, count, scoped",
            crystalline_index::turso::filter_only_count_sql("e.actor = ''", &scoped_100),
        ),
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
    // A schema of its own per run, so two runs side by side never share one.
    // A run that panics before `drop_schema` below leaves its schema behind
    // under a name no later run picks again.
    let schema = perf_schema();
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
