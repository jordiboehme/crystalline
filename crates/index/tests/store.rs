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
    DomainKind, EmbeddingRow, EngramId, EngramRecord, FileStamp, FilterOp, HostClaim, IndexError,
    MetadataFilter, RecentFilter, SearchMode, SearchQuery, Store, TursoStore, sync_domain,
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

/// A minimal engram record with an explicit content and checksum, built without
/// parsing so the store methods can be exercised directly on both backends. The
/// `sha` is the CAS token stored in the stamp.
fn record(path: &str, permalink: &str, content: &str, sha: &str) -> EngramRecord {
    EngramRecord {
        path: path.to_string(),
        permalink: permalink.to_string(),
        title: "Title".to_string(),
        engram_type: "engram".to_string(),
        status: "current".to_string(),
        recorded_at: Some("2026-01-01".to_string()),
        valid_from: None,
        valid_to: None,
        timestamp: None,
        description: None,
        content: content.to_string(),
        metadata: serde_json::json!({}),
        tags: Vec::new(),
        observations: Vec::new(),
        relations: Vec::new(),
        links: Vec::new(),
        stamp: FileStamp {
            mtime: 0,
            size: content.len() as u64,
            sha256: sha.to_string(),
        },
    }
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

async fn cas_guarded_upsert(store: &dyn Store) {
    // A virtual domain (no path) holds one engram written straight through the
    // store, no filesystem involved.
    let did = store
        .upsert_domain("v", None, DomainKind::Virtual)
        .await
        .unwrap();
    store
        .upsert_engram_checked(did, &record("n.md", "n", "v1", "sha-v1"), None)
        .await
        .unwrap();

    // A checked write with the matching expected sha succeeds and advances the
    // stored sha.
    store
        .upsert_engram_checked(did, &record("n.md", "n", "v2", "sha-v2"), Some("sha-v1"))
        .await
        .unwrap();
    assert_eq!(
        store.engram_content(did, "n.md").await.unwrap().as_deref(),
        Some("v2")
    );

    // A checked write with a stale expected sha is refused as StaleEdit and does
    // not clobber the stored content.
    let err = store
        .upsert_engram_checked(did, &record("n.md", "n", "v3", "sha-v3"), Some("sha-v1"))
        .await
        .unwrap_err();
    match err {
        IndexError::StaleEdit { expected, found } => {
            assert_eq!(expected, "sha-v1");
            assert_eq!(found, "sha-v2");
        }
        other => panic!("expected StaleEdit, got {other:?}"),
    }
    assert_eq!(
        store.engram_content(did, "n.md").await.unwrap().as_deref(),
        Some("v2"),
        "stale edit must not overwrite"
    );

    // A first write at a brand-new path with an expected sha still succeeds
    // (nothing stored to compare against).
    store
        .upsert_engram_checked(
            did,
            &record("fresh.md", "fresh", "hi", "sha-f"),
            Some("anything"),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .engram_content(did, "fresh.md")
            .await
            .unwrap()
            .as_deref(),
        Some("hi")
    );
}
parity!(cas_guarded_upsert_detects_stale_edits, cas_guarded_upsert);

async fn content_roundtrip(store: &dyn Store) {
    let did = store
        .upsert_domain("v", None, DomainKind::Virtual)
        .await
        .unwrap();
    store
        .upsert_engram(did, &record("a.md", "a", "alpha body", "sha-a"))
        .await
        .unwrap();
    store
        .upsert_engram(did, &record("notes/b.md", "b", "beta body", "sha-b"))
        .await
        .unwrap();

    // engram_content returns the stored content, or None for an absent path.
    assert_eq!(
        store.engram_content(did, "a.md").await.unwrap().as_deref(),
        Some("alpha body")
    );
    assert!(
        store
            .engram_content(did, "missing.md")
            .await
            .unwrap()
            .is_none()
    );

    // all_engram_contents streams the whole domain, ordered by path, with the
    // permalink, content and checksum needed to export it verbatim.
    let all = store.all_engram_contents(did).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].path, "a.md");
    assert_eq!(all[0].permalink, "a");
    assert_eq!(all[0].content, "alpha body");
    assert_eq!(all[0].sha256, "sha-a");
    assert_eq!(all[1].path, "notes/b.md");
}
parity!(content_roundtrips_through_the_store, content_roundtrip);

async fn clear_domain_is_scoped(store: &dyn Store) {
    let keep = store
        .upsert_domain("keep", None, DomainKind::Virtual)
        .await
        .unwrap();
    let gone = store
        .upsert_domain("gone", None, DomainKind::Virtual)
        .await
        .unwrap();
    store
        .upsert_engram(keep, &record("k.md", "k", "keepterm body", "sha-k"))
        .await
        .unwrap();
    store
        .upsert_engram(gone, &record("g.md", "g", "goneterm body", "sha-g"))
        .await
        .unwrap();

    // Clearing one domain leaves the other, and the domain rows themselves,
    // untouched.
    store.clear_domain(gone).await.unwrap();
    assert!(store.all_engram_contents(gone).await.unwrap().is_empty());
    assert_eq!(store.all_engram_contents(keep).await.unwrap().len(), 1);
    assert_eq!(
        store.domain_stats().await.unwrap().len(),
        2,
        "clear_domain keeps the domain rows"
    );

    // The kept domain's engram is still searchable; the cleared one's is gone.
    let kept = store.search(&SearchQuery::text("keepterm")).await.unwrap();
    assert_eq!(kept.total, 1);
    let cleared = store.search(&SearchQuery::text("goneterm")).await.unwrap();
    assert_eq!(cleared.total, 0);
}
parity!(clear_domain_scopes_to_one_domain, clear_domain_is_scoped);

// --- host locks (shared-database collaboration) ------------------------------

async fn host_claim_and_contest(store: &dyn Store) {
    // The lock FKs to a real domain row, so register a file domain first. Two
    // instances are simulated by two instance-id strings against one store,
    // exactly the single-writer-per-domain rule the daemon relies on. Times are
    // fixed-width ISO strings, compared lexically like every temporal column.
    let did = store
        .upsert_domain("eng", Some("/k/eng"), DomainKind::File)
        .await
        .unwrap();

    // No lock yet.
    assert!(store.domain_host(did).await.unwrap().is_none());

    // First claim on an unheld lock: instance A acquires.
    let a_at = "2026-07-03T10:00:00+00:00";
    let stale_before = "2026-07-03T09:59:00+00:00"; // nothing is stale relative to this
    let claim = store
        .claim_domain_host(did, "inst-a", "node-a", a_at, stale_before, false)
        .await
        .unwrap();
    assert_eq!(claim, HostClaim::Acquired);
    let host = store.domain_host(did).await.unwrap().unwrap();
    assert_eq!(host.instance_id, "inst-a");
    assert_eq!(host.label, "node-a");
    assert_eq!(host.heartbeat_at, a_at);

    // Contested claim: B tries while A's heartbeat is fresh and no takeover is
    // asked, so B is refused and A keeps the lock unchanged.
    let b_at = "2026-07-03T10:00:20+00:00";
    let stale_fresh = "2026-07-03T09:59:30+00:00"; // A's 10:00:00 is after this: fresh
    match store
        .claim_domain_host(did, "inst-b", "node-b", b_at, stale_fresh, false)
        .await
        .unwrap()
    {
        HostClaim::HeldByOther(h) => {
            assert_eq!(h.instance_id, "inst-a");
            assert_eq!(h.heartbeat_at, a_at);
        }
        HostClaim::Acquired => panic!("B must not acquire a domain A holds with a fresh heartbeat"),
    }
    assert_eq!(
        store.domain_host(did).await.unwrap().unwrap().instance_id,
        "inst-a",
        "A still holds it after a refused contest"
    );

    // domain_stats surfaces the kind and the current host.
    let stats = store.domain_stats().await.unwrap();
    let s = stats.iter().find(|d| d.name == "eng").unwrap();
    assert_eq!(s.kind, DomainKind::File);
    assert_eq!(s.host_instance_id.as_deref(), Some("inst-a"));
    assert_eq!(s.host_heartbeat_at.as_deref(), Some(a_at));
}
parity!(host_claim_acquires_and_contests, host_claim_and_contest);

async fn host_renew_takeover_release(store: &dyn Store) {
    let did = store
        .upsert_domain("eng", Some("/k/eng"), DomainKind::File)
        .await
        .unwrap();
    let a_at = "2026-07-03T10:00:00+00:00";
    let stale_before = "2026-07-03T09:59:00+00:00";
    store
        .claim_domain_host(did, "inst-a", "node-a", a_at, stale_before, false)
        .await
        .unwrap();

    // Renew: the holder refreshes its heartbeat; a stranger's renew is a no-op.
    let a_beat = "2026-07-03T10:00:25+00:00";
    assert!(
        store
            .renew_domain_host(did, "inst-a", a_beat)
            .await
            .unwrap()
    );
    assert_eq!(
        store.domain_host(did).await.unwrap().unwrap().heartbeat_at,
        a_beat
    );
    assert!(
        !store
            .renew_domain_host(did, "inst-b", a_beat)
            .await
            .unwrap(),
        "a non-holder renew updates nothing"
    );

    // Stale takeover: B claims with a stale_before after A's last heartbeat, so
    // A reads as stale and B acquires without a takeover flag.
    let b_at = "2026-07-03T10:05:00+00:00";
    let stale_past = "2026-07-03T10:04:00+00:00"; // A's 10:00:25 is before this: stale
    let claim = store
        .claim_domain_host(did, "inst-b", "node-b", b_at, stale_past, false)
        .await
        .unwrap();
    assert_eq!(claim, HostClaim::Acquired);
    assert_eq!(
        store.domain_host(did).await.unwrap().unwrap().instance_id,
        "inst-b"
    );

    // Explicit takeover: A forces the claim back even though B is fresh.
    let a2_at = "2026-07-03T10:05:10+00:00";
    let stale_fresh = "2026-07-03T10:04:59+00:00"; // B's 10:05:00 is fresh vs this
    let claim = store
        .claim_domain_host(did, "inst-a", "node-a", a2_at, stale_fresh, true)
        .await
        .unwrap();
    assert_eq!(claim, HostClaim::Acquired);
    assert_eq!(
        store.domain_host(did).await.unwrap().unwrap().instance_id,
        "inst-a"
    );

    // A same-holder re-claim is idempotent and refreshes the heartbeat.
    let a3_at = "2026-07-03T10:05:20+00:00";
    let claim = store
        .claim_domain_host(did, "inst-a", "node-a", a3_at, stale_fresh, false)
        .await
        .unwrap();
    assert_eq!(claim, HostClaim::Acquired);
    assert_eq!(
        store.domain_host(did).await.unwrap().unwrap().heartbeat_at,
        a3_at
    );

    // Release: a non-holder's release leaves the lock; the holder's clears it.
    store.release_domain_host(did, "inst-b").await.unwrap();
    assert!(
        store.domain_host(did).await.unwrap().is_some(),
        "a non-holder release does not clear the lock"
    );
    store.release_domain_host(did, "inst-a").await.unwrap();
    assert!(
        store.domain_host(did).await.unwrap().is_none(),
        "the holder's release clears the lock"
    );
}
parity!(
    host_renews_takes_over_and_releases,
    host_renew_takeover_release
);

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

// --- embedding column width -------------------------------------------------

/// A deterministic, network-free embedding: hashes each word into one of
/// `dims` buckets and L2-normalizes, so texts sharing vocabulary get similar
/// vectors. Parameterized on `dims` so the same corpus can stand in for a
/// narrow remote provider and for the local default width in the same test.
fn embed_one(text: &str, dims: usize) -> Vec<f32> {
    let mut v = vec![0f32; dims];
    for tok in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
    {
        let mut h: u64 = 0;
        for byte in tok.to_lowercase().bytes() {
            h = h.wrapping_mul(31).wrapping_add(byte as u64);
        }
        v[(h % dims as u64) as usize] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        let mut z = vec![0f32; dims];
        z[0] = 1.0;
        return z;
    }
    v.iter().map(|x| x / norm).collect()
}

fn semantic_query(text: &str, dims: usize, model: &str) -> SearchQuery {
    SearchQuery {
        text: Some(text.to_string()),
        mode: SearchMode::Semantic,
        query_embedding: Some(embed_one(text, dims)),
        active_model: Some(model.to_string()),
        min_similarity: Some(0.0),
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    }
}

/// The `chunk.embedding` column follows the active provider's width rather
/// than being fixed at whatever the initial migration picked. A narrow
/// (8-dim) provider stores and searches fine even though the Postgres column
/// starts at 384; switching to a 384-dim provider resizes it back, also
/// without error. A dims change already invalidates every stored vector
/// through the existing staleness machinery (mixed dims already refuse
/// semantic search and already mark chunks pending re-embedding), so the
/// resize rides that invalidation rather than adding a new failure mode. On
/// Turso this is unchanged behavior (its blob column was never width
/// enforced); the point of running it here is that the same body now passes
/// on Postgres too.
async fn embedding_width_follows_provider(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "db.md",
        &engram(
            "Databases",
            "databases",
            "engram",
            "",
            "postgres postgres index index query query",
        ),
    );
    write(
        root,
        "cook.md",
        &engram(
            "Cooking",
            "cooking",
            "engram",
            "",
            "recipe recipe kitchen kitchen food food",
        ),
    );
    sync_domain(store, "d", root).await.unwrap();

    // A narrow provider stores fine even though the column starts at 384: no
    // error surfaces on either backend.
    let jobs = store
        .chunks_needing_embedding("narrow-8", None)
        .await
        .unwrap();
    assert!(!jobs.is_empty(), "chunks await embedding after sync");
    let pending = jobs.len();
    let rows: Vec<EmbeddingRow> = jobs
        .iter()
        .map(|j| EmbeddingRow {
            chunk_id: j.chunk_id,
            embedding: embed_one(&j.text, 8),
            dims: 8,
        })
        .collect();
    store.store_embeddings(&rows, "narrow-8").await.unwrap();

    let narrow_hits = store
        .search(&semantic_query("postgres index query", 8, "narrow-8"))
        .await
        .unwrap();
    assert_eq!(
        narrow_hits.items[0].permalink, "databases",
        "8-dim embeddings rank correctly once the column narrows"
    );

    // A 384-dim provider resizes the column back and stores fine too. The
    // model swap makes every chunk pending again, dims aside.
    let jobs = store
        .chunks_needing_embedding("wide-384", None)
        .await
        .unwrap();
    assert_eq!(
        jobs.len(),
        pending,
        "the model swap makes every chunk pending again"
    );
    let rows: Vec<EmbeddingRow> = jobs
        .iter()
        .map(|j| EmbeddingRow {
            chunk_id: j.chunk_id,
            embedding: embed_one(&j.text, 384),
            dims: 384,
        })
        .collect();
    store.store_embeddings(&rows, "wide-384").await.unwrap();

    let wide_hits = store
        .search(&semantic_query("postgres index query", 384, "wide-384"))
        .await
        .unwrap();
    assert_eq!(
        wide_hits.items[0].permalink, "databases",
        "384-dim embeddings rank correctly once the column widens back"
    );
    assert!(
        store
            .chunks_needing_embedding("wide-384", None)
            .await
            .unwrap()
            .is_empty(),
        "nothing left pending for the active model"
    );
}
parity!(
    embedding_column_width_follows_provider_dims,
    embedding_width_follows_provider
);
