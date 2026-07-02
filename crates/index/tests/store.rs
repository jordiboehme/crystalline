//! Integration tests for the store, sync engine and search planner.

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

async fn open() -> TursoStore {
    TursoStore::open_in_memory().await.unwrap()
}

#[tokio::test]
async fn full_sync_counts_engrams_observations_relations() {
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

    let store = open().await;
    let report = sync_domain(&store, "eng", root).await.unwrap();
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

#[tokio::test]
async fn warm_sync_reports_all_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "body a\n"));
    write(root, "b.md", &engram("B", "b", "engram", "", "body b\n"));
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();
    let warm = sync_domain(&store, "d", root).await.unwrap();
    assert_eq!(warm.added, 0);
    assert_eq!(warm.updated, 0);
    assert_eq!(warm.unchanged, 2);
}

#[tokio::test]
async fn edit_then_sync_updates() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram("A", "a", "engram", "", "original body\n"),
    );
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();

    // Rewrite with different content and bump the mtime past the prefilter.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(
        root,
        "a.md",
        &engram("A", "a", "engram", "", "revised body\n"),
    );
    let report = sync_domain(&store, "d", root).await.unwrap();
    assert_eq!(report.updated, 1);
    assert_eq!(report.added, 0);

    let page = store.search(&SearchQuery::text("revised")).await.unwrap();
    assert_eq!(page.total, 1);
}

#[tokio::test]
async fn delete_then_sync_removes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "body a\n"));
    write(root, "b.md", &engram("B", "b", "engram", "", "body b\n"));
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();

    std::fs::remove_file(root.join("b.md")).unwrap();
    let report = sync_domain(&store, "d", root).await.unwrap();
    assert_eq!(report.deleted, 1);
    let stats = store.domain_stats().await.unwrap();
    assert_eq!(stats[0].engrams, 1);
}

#[tokio::test]
async fn move_is_rename_without_reparse() {
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
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();
    assert!(store.lookup_id("d", "old/name").await.unwrap().is_some());

    // Move the file: identical bytes at a new path.
    std::fs::create_dir_all(root.join("new")).unwrap();
    std::fs::rename(root.join("old/name.md"), root.join("new/name.md")).unwrap();
    let report = sync_domain(&store, "d", root).await.unwrap();
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

#[tokio::test]
async fn forward_reference_resolves_on_later_sync() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram("A", "a", "engram", "", "- depends_on [[target-b]]\n"),
    );
    let store = open().await;
    let first = sync_domain(&store, "d", root).await.unwrap();
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
    let second = sync_domain(&store, "d", root).await.unwrap();
    assert_eq!(second.relations_resolved, 1, "now resolved");
    assert_eq!(
        store.domain_stats().await.unwrap()[0].unresolved_relations,
        0
    );
}

#[tokio::test]
async fn duplicate_permalink_is_collected_as_failure() {
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
    let store = open().await;
    let report = sync_domain(&store, "d", root).await.unwrap();
    assert_eq!(report.added, 1, "one wins");
    assert_eq!(
        report.failed.len(),
        1,
        "the other fails: {:?}",
        report.failed
    );
    assert!(report.failed[0].1.contains("permalink"));
}

#[tokio::test]
async fn search_finds_by_title_content_and_observation() {
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
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();

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

#[tokio::test]
async fn search_applies_type_status_tag_and_metadata_filters() {
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
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();

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

    // $between on a custom date field routed through json_extract, parsed from
    // the JSON wire form the M5 tools will hand in.
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

#[tokio::test]
async fn canonical_temporal_filter_returns_only_currently_valid() {
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
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();

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

#[tokio::test]
async fn search_paginates() {
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
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();

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

#[tokio::test]
async fn neighbors_depth_and_cross_domain() {
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

    let store = open().await;
    // Sync the target domain first so the cross-domain ref resolves.
    sync_domain(&store, "domain2", d2.path()).await.unwrap();
    let r1 = sync_domain(&store, "domain1", d1.path()).await.unwrap();
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

#[tokio::test]
async fn recent_returns_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "old.md", &engram("Old", "old", "engram", "", "b\n"));
    write(
        root,
        "new.md",
        "---\ntype: engram\ntitle: New\npermalink: new\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-06-01\n---\n\nb\n",
    );
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();
    let recent = store
        .recent(&RecentFilter {
            limit: 10,
            ..RecentFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(recent[0].permalink, "new", "2026-06-01 before 2026-01-01");
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

#[tokio::test]
async fn wipe_clears_everything() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "b\n"));
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();
    assert_eq!(store.domain_stats().await.unwrap()[0].engrams, 1);
    store.wipe().await.unwrap();
    assert!(store.domain_stats().await.unwrap().is_empty());
    let page = store.search(&SearchQuery::text("b")).await.unwrap();
    assert_eq!(page.total, 0);
}

#[tokio::test]
async fn store_info_reports_candidate_scan_fallback() {
    let store = open().await;
    let info = store.store_info().await.unwrap();
    assert_eq!(info.fts_mode, crystalline_index::FtsMode::CandidateScan);
    assert_eq!(info.schema_version, 2);
}

#[tokio::test]
async fn title_and_permalink_search_modes() {
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
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();

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

#[tokio::test]
async fn seed_ids_are_stable_across_lookups() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "engram", "", "b\n"));
    let store = open().await;
    sync_domain(&store, "d", root).await.unwrap();
    let id1 = store.lookup_id("d", "a").await.unwrap();
    let id2 = store.lookup_id("d", "a").await.unwrap();
    assert_eq!(id1, id2);
    assert!(matches!(id1, Some(EngramId(_))));
}
