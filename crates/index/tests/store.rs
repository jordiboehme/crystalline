//! Cross-backend behavioral parity suite for the store, sync engine and search
//! planner.
//!
//! Every test body is a pure function of a `&dyn Store`, so the same assertions
//! run against both backends. Turso (in-memory) always runs. Postgres runs when
//! `CRYSTALLINE_TEST_POSTGRES_URL` is set (each test gets its own schema via
//! `search_path`, dropped afterwards); when it is unset the Postgres leg is
//! skipped with a one-time note and the suite stays green. Backend-specific
//! assertions (Turso schema version, the query-plan index seek, the on-disk file)
//! live in `turso_only.rs`.

use std::path::Path;

use crystalline_index::{
    EngramId, FilterOp, MetadataFilter, RecentFilter, SearchMode, SearchQuery, Store, TursoStore,
    sync_domain,
};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// A minimal engram markdown block.
fn engram(title: &str, permalink: &str, ftype: &str, extra_fm: &str, body: &str) -> String {
    format!(
        "---\ntype: {ftype}\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n{extra_fm}---\n\n# {title}\n\n{body}\n"
    )
}

// --- backend runner ----------------------------------------------------------

#[cfg(feature = "postgres")]
fn pg_url() -> Option<String> {
    use std::sync::Once;
    static NOTE: Once = Once::new();
    match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            NOTE.call_once(|| {
                eprintln!(
                    "note: skipping the postgres parity leg (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
                )
            });
            None
        }
    }
}

/// A distinct schema name per test invocation. The pid keeps runs apart, the
/// counter keeps tests within a run apart; both stay well under Postgres's
/// 63-byte identifier limit.
#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ct_{}_{}", std::process::id(), n)
}

/// Run a parity body against Turso (always) and Postgres (when configured),
/// giving each backend a fresh, isolated store.
macro_rules! parity {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                $body(&store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(url) = pg_url() {
                    let schema = unique_schema();
                    let store = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .expect("open the postgres test schema");
                    $body(&store).await;
                    store
                        .drop_schema()
                        .await
                        .expect("drop the postgres test schema");
                }
            }
        }
    };
}

// --- parity bodies -----------------------------------------------------------

async fn full_sync_counts(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "MANIFEST.md",
        &engram(
            "Manifest",
            "manifest",
            "manifest",
            "",
            "## Scope\n\n- covers things\n\n## When to Use\n\n- when routing\n",
        ),
    );
    write(
        root,
        "alpha.md",
        &engram(
            "Alpha",
            "alpha",
            "engram",
            "",
            "- [fact] the sky is blue #color (observed)\n\n- relates_to [[Beta]]\n\nProse mentions [[Beta]] once.\n",
        ),
    );
    write(
        root,
        "notes/beta.md",
        &engram("Beta", "beta", "engram", "", "Beta body content.\n"),
    );

    let report = sync_domain(store, "eng", root).await.unwrap();
    assert_eq!(report.added, 3, "three files added");
    assert_eq!(report.updated, 0);
    assert_eq!(report.failed.len(), 0, "no failures: {:?}", report.failed);
    assert!(report.relations_resolved >= 1, "Alpha->Beta resolved");

    let stats = store.domain_stats().await.unwrap();
    assert_eq!(stats.len(), 1);
    let s = &stats[0];
    assert_eq!(s.engrams, 3);
    assert_eq!(s.observations, 1);
    assert_eq!(s.relations, 1);
    assert_eq!(s.unresolved_relations, 0);
    assert!(s.last_sync.is_some());
}
parity!(
    full_sync_counts_engrams_observations_relations,
    full_sync_counts
);

async fn warm_sync_unchanged(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "body a\n"));
    write(root, "b.md", &engram("B", "b", "engram", "", "body b\n"));
    sync_domain(store, "d", root).await.unwrap();
    let warm = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(warm.added, 0);
    assert_eq!(warm.updated, 0);
    assert_eq!(warm.unchanged, 2);
}
parity!(warm_sync_reports_all_unchanged, warm_sync_unchanged);

async fn edit_then_sync(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram("A", "a", "engram", "", "original body\n"),
    );
    sync_domain(store, "d", root).await.unwrap();

    // Rewrite with different content and bump the mtime past the prefilter.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(
        root,
        "a.md",
        &engram("A", "a", "engram", "", "revised body\n"),
    );
    let report = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(report.updated, 1);
    assert_eq!(report.added, 0);

    let page = store.search(&SearchQuery::text("revised")).await.unwrap();
    assert_eq!(page.total, 1);
}
parity!(edit_then_sync_updates, edit_then_sync);

async fn delete_then_sync(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "body a\n"));
    write(root, "b.md", &engram("B", "b", "engram", "", "body b\n"));
    sync_domain(store, "d", root).await.unwrap();

    std::fs::remove_file(root.join("b.md")).unwrap();
    let report = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(report.deleted, 1);
    let stats = store.domain_stats().await.unwrap();
    assert_eq!(stats[0].engrams, 1);
}
parity!(delete_then_sync_removes, delete_then_sync);

async fn move_is_rename(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // No explicit permalink, so it is derived from the path.
    let body = "unique_marker_token in the body\n";
    write(
        root,
        "old/name.md",
        &format!(
            "---\ntype: engram\ntitle: Mover\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}"
        ),
    );
    sync_domain(store, "d", root).await.unwrap();
    assert!(store.lookup_id("d", "old/name").await.unwrap().is_some());

    // Move the file: identical bytes at a new path.
    std::fs::create_dir_all(root.join("new")).unwrap();
    std::fs::rename(root.join("old/name.md"), root.join("new/name.md")).unwrap();
    let report = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(report.moved, 1, "classified as a move");
    assert_eq!(report.added, 0, "not reparsed as an add");
    assert_eq!(report.updated, 0, "not reparsed as an update");
    assert_eq!(report.deleted, 0, "not treated as a delete");

    // The engram kept its content and moved to the new path-derived permalink.
    assert!(store.lookup_id("d", "old/name").await.unwrap().is_none());
    assert!(store.lookup_id("d", "new/name").await.unwrap().is_some());
    let page = store
        .search(&SearchQuery::text("unique_marker_token"))
        .await
        .unwrap();
    assert_eq!(page.total, 1, "content preserved through the move");
}
parity!(move_is_rename_without_reparse, move_is_rename);

async fn forward_reference_resolves(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram("A", "a", "engram", "", "- depends_on [[target-b]]\n"),
    );
    let first = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(first.relations_resolved, 0, "target absent, unresolved");
    assert_eq!(
        store.domain_stats().await.unwrap()[0].unresolved_relations,
        1
    );

    // The target appears in a later sync.
    write(
        root,
        "b.md",
        &engram("B", "target-b", "engram", "", "body b\n"),
    );
    let second = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(second.relations_resolved, 1, "now resolved");
    assert_eq!(
        store.domain_stats().await.unwrap()[0].unresolved_relations,
        0
    );
}
parity!(
    forward_reference_resolves_on_later_sync,
    forward_reference_resolves
);

async fn duplicate_permalink_fails(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "one.md",
        &engram("One", "shared", "engram", "", "body one\n"),
    );
    write(
        root,
        "two.md",
        &engram("Two", "shared", "engram", "", "body two\n"),
    );
    let report = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(report.added, 1, "one wins");
    assert_eq!(
        report.failed.len(),
        1,
        "the other fails: {:?}",
        report.failed
    );
    assert!(report.failed[0].1.contains("permalink"));
}
parity!(
    duplicate_permalink_is_collected_as_failure,
    duplicate_permalink_fails
);

async fn search_finds_across_fields(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "title.md",
        &engram(
            "Photosynthesis basics",
            "title-hit",
            "engram",
            "",
            "generic body\n",
        ),
    );
    write(
        root,
        "content.md",
        &engram(
            "Generic",
            "content-hit",
            "engram",
            "",
            "the mitochondria is the powerhouse\n",
        ),
    );
    write(
        root,
        "obs.md",
        &engram(
            "Generic two",
            "obs-hit",
            "engram",
            "",
            "- [fact] tardigrades survive vacuum #biology\n",
        ),
    );
    sync_domain(store, "d", root).await.unwrap();

    let by_title = store
        .search(&SearchQuery::text("photosynthesis"))
        .await
        .unwrap();
    assert_eq!(by_title.items[0].permalink, "title-hit");

    let by_content = store
        .search(&SearchQuery::text("mitochondria"))
        .await
        .unwrap();
    assert_eq!(by_content.items[0].permalink, "content-hit");

    let by_obs = store
        .search(&SearchQuery::text("tardigrades"))
        .await
        .unwrap();
    assert_eq!(by_obs.items[0].permalink, "obs-hit");
    match by_obs.items[0].kind {
        crystalline_index::HitKind::Observation { line } => assert!(line > 0),
        crystalline_index::HitKind::Engram => panic!("expected an observation-level hit"),
    }
}
parity!(
    search_finds_by_title_content_and_observation,
    search_finds_across_fields
);

async fn search_applies_filters(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        "---\ntype: decision\ntitle: Decision A\npermalink: dec-a\ntags:\n  - arch\n  - keep\nstatus: current\nrecorded_at: 2026-01-01\nevent_date: \"2026-03-15\"\n---\n\nbody\n",
    );
    write(
        root,
        "b.md",
        "---\ntype: guide\ntitle: Guide B\npermalink: guide-b\ntags:\n  - arch\nstatus: draft\nrecorded_at: 2026-02-01\nevent_date: \"2026-09-01\"\n---\n\nbody\n",
    );
    sync_domain(store, "d", root).await.unwrap();

    let by_type = store
        .search(&SearchQuery {
            engram_type: Some("decision".into()),
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(by_type.total, 1);
    assert_eq!(by_type.items[0].permalink, "dec-a");

    let by_status = store
        .search(&SearchQuery {
            status: Some("draft".into()),
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(by_status.total, 1);
    assert_eq!(by_status.items[0].permalink, "guide-b");

    let by_tag = store
        .search(&SearchQuery {
            tags: Some(vec!["keep".into()]),
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(by_tag.total, 1);
    assert_eq!(by_tag.items[0].permalink, "dec-a");

    // $between on a custom date field: json_extract on Turso, metadata->> on
    // Postgres, both ISO-string comparisons, parsed from the JSON wire form.
    let wire = serde_json::json!({ "event_date": { "$between": ["2026-01-01", "2026-06-01"] } });
    let filters = crystalline_index::parse_metadata_filters(&wire).unwrap();
    assert_eq!(
        filters,
        vec![MetadataFilter {
            key: "event_date".into(),
            op: FilterOp::Between("2026-01-01".into(), "2026-06-01".into()),
        }]
    );
    let by_between = store
        .search(&SearchQuery {
            metadata_filters: filters,
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(by_between.total, 1, "only the March event is in range");
    assert_eq!(by_between.items[0].permalink, "dec-a");
}
parity!(
    search_applies_type_status_tag_and_metadata_filters,
    search_applies_filters
);

async fn canonical_temporal_filter(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Unbounded (valid): no valid_to.
    write(
        root,
        "always.md",
        &engram("Always", "always", "engram", "", "body\n"),
    );
    // Expired: valid_to before today.
    write(
        root,
        "past.md",
        &engram("Past", "past", "engram", "valid_to: 2026-01-01\n", "body\n"),
    );
    // Future window still open.
    write(
        root,
        "future.md",
        &engram(
            "Future",
            "future",
            "engram",
            "valid_to: 2027-01-01\n",
            "body\n",
        ),
    );
    // Not current status.
    write(
        root,
        "draft.md",
        "---\ntype: engram\ntitle: Draft\npermalink: draft\ntags:\n  - t\nstatus: draft\nrecorded_at: 2026-01-01\n---\n\nbody\n",
    );
    sync_domain(store, "d", root).await.unwrap();

    let page = store
        .search(&SearchQuery {
            current_only: true,
            today: Some("2026-07-02".into()),
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    let mut perms: Vec<String> = page.items.iter().map(|h| h.permalink.clone()).collect();
    perms.sort();
    assert_eq!(perms, vec!["always".to_string(), "future".to_string()]);
}
parity!(
    canonical_temporal_filter_returns_only_currently_valid,
    canonical_temporal_filter
);

async fn search_pages(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..7 {
        write(
            root,
            &format!("e{i}.md"),
            &engram(
                &format!("Engram {i}"),
                &format!("e{i}"),
                "engram",
                "",
                "shared_term here\n",
            ),
        );
    }
    sync_domain(store, "d", root).await.unwrap();

    let page1 = store
        .search(&SearchQuery {
            text: Some("shared_term".into()),
            limit: 3,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(page1.total, 7);
    assert_eq!(page1.items.len(), 3);

    let page3 = store
        .search(&SearchQuery {
            text: Some("shared_term".into()),
            limit: 3,
            page: 3,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(page3.items.len(), 1, "7 items, page 3 of size 3 has 1");
}
parity!(search_paginates, search_pages);

async fn neighbors_cross_domain(store: &dyn Store) {
    // domain2 holds the cross-domain target C.
    let d2 = tempfile::tempdir().unwrap();
    write(
        d2.path(),
        "c.md",
        &engram("C", "c", "engram", "", "gamma body\n"),
    );
    // domain1 holds A -> B (same domain) and B -> domain2:C (cross-domain).
    let d1 = tempfile::tempdir().unwrap();
    write(
        d1.path(),
        "a.md",
        &engram("A", "a", "engram", "", "- relates_to [[b]]\n"),
    );
    write(
        d1.path(),
        "b.md",
        &engram("B", "b", "engram", "", "- relates_to [[domain2:c]]\n"),
    );

    // Sync the target domain first so the cross-domain ref resolves.
    sync_domain(store, "domain2", d2.path()).await.unwrap();
    let r1 = sync_domain(store, "domain1", d1.path()).await.unwrap();
    assert_eq!(r1.relations_resolved, 2, "A->B and B->C both resolve");

    let a = store.lookup_id("domain1", "a").await.unwrap().unwrap();

    let d1_slice = store.neighbors(&[a], 1).await.unwrap();
    let perms1: Vec<&str> = d1_slice
        .nodes
        .iter()
        .map(|n| n.permalink.as_str())
        .collect();
    assert!(perms1.contains(&"a"));
    assert!(perms1.contains(&"b"), "depth 1 reaches B");
    assert!(!perms1.contains(&"c"), "depth 1 does not reach C");

    let d2_slice = store.neighbors(&[a], 2).await.unwrap();
    let perms2: Vec<&str> = d2_slice
        .nodes
        .iter()
        .map(|n| n.permalink.as_str())
        .collect();
    assert!(perms2.contains(&"c"), "depth 2 reaches cross-domain C");
    let has_cross = d2_slice
        .nodes
        .iter()
        .any(|n| n.permalink == "c" && n.domain == "domain2");
    assert!(has_cross, "C is labeled with its own domain");
    assert!(d2_slice.edges.len() >= 2, "A-B and B-C edges present");
}
parity!(neighbors_depth_and_cross_domain, neighbors_cross_domain);

async fn recent_newest_first(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "old.md", &engram("Old", "old", "engram", "", "b\n"));
    write(
        root,
        "new.md",
        "---\ntype: engram\ntitle: New\npermalink: new\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-06-01\n---\n\nb\n",
    );
    sync_domain(store, "d", root).await.unwrap();
    let recent = store
        .recent(&RecentFilter {
            limit: 10,
            ..RecentFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(recent[0].permalink, "new", "2026-06-01 before 2026-01-01");
}
parity!(recent_returns_newest_first, recent_newest_first);

async fn wipe_clears(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "b\n"));
    sync_domain(store, "d", root).await.unwrap();
    assert_eq!(store.domain_stats().await.unwrap()[0].engrams, 1);
    store.wipe().await.unwrap();
    assert!(store.domain_stats().await.unwrap().is_empty());
    let page = store.search(&SearchQuery::text("b")).await.unwrap();
    assert_eq!(page.total, 0);
}
parity!(wipe_clears_everything, wipe_clears);

async fn store_info_reports_candidate_scan(store: &dyn Store) {
    // Both backends run the LIKE-candidate scan, so hybrid ranking and every
    // search test match. The Turso-only schema version lives in turso_only.rs.
    let info = store.store_info().await.unwrap();
    assert_eq!(info.fts_mode, crystalline_index::FtsMode::CandidateScan);
}
parity!(
    store_info_reports_candidate_scan_fallback,
    store_info_reports_candidate_scan
);

async fn title_and_permalink_modes(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram(
            "Distinct Title Word",
            "alpha-slug",
            "engram",
            "",
            "the body says beta\n",
        ),
    );
    sync_domain(store, "d", root).await.unwrap();

    // Title mode ignores a term that is only in the body.
    let title_miss = store
        .search(&SearchQuery {
            text: Some("beta".into()),
            mode: SearchMode::Title,
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(title_miss.total, 0);

    let perma = store
        .search(&SearchQuery {
            text: Some("alpha-slug".into()),
            mode: SearchMode::Permalink,
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(perma.total, 1);
}
parity!(title_and_permalink_search_modes, title_and_permalink_modes);

async fn seed_ids_stable(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "b\n"));
    sync_domain(store, "d", root).await.unwrap();
    let id1 = store.lookup_id("d", "a").await.unwrap();
    let id2 = store.lookup_id("d", "a").await.unwrap();
    assert_eq!(id1, id2);
    assert!(matches!(id1, Some(EngramId(_))));
}
parity!(seed_ids_are_stable_across_lookups, seed_ids_stable);
