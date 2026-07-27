//! The slabbed apply: the changed set is read, parsed and upserted in bounded
//! slabs so a sync of a huge domain never holds the whole changed set in memory.
//!
//! The slab size is a cut of the work, never a change of the result, so the pin
//! here is equivalence: the same fixture domain synced with a slab size of 2 and
//! in one shot must leave two fresh stores byte-identical - engram rows and their
//! checksums, file stamps, chunk rows with their fingerprints, resolved relations
//! and links, and the domain counters. The fixture spans every slab-sensitive
//! case at once: engrams that link to each other across slab boundaries, a file
//! deleted before the pass, an untouched file, a move and an edit. Every body is
//! a pure function of a `&dyn Store` so it runs on both backends; Turso
//! (in-memory) always runs, Postgres runs when `CRYSTALLINE_TEST_POSTGRES_URL` is
//! set.

use std::path::Path;

use crystalline_index::{
    ChunkParams, DomainKind, DomainStats, Store, SyncReport, apply_scan_with_slab, scan_domain,
};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// A minimal engram markdown block with an observation and a relation to another
/// permalink, so relation resolution has to span the whole pass.
fn engram(title: &str, permalink: &str, body: &str, relates_to: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n\n- [fact] a fact about {permalink} #t\n\n- relates_to [[{relates_to}]]\n\nSee also [[{relates_to}]] in prose.\n"
    )
}

/// Lay down the fixture domain: seven engrams in a ring of relations, spread over
/// subfolders, plus a MANIFEST so the tag-alias refresh has something to read.
fn fixture(dir: &Path) {
    write(
        dir,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: Fixture\npermalink: manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Fixture\n\n## Tag Aliases\n\n- tee -> t\n",
    );
    for i in 0..7 {
        write(
            dir,
            &format!("dir{}/f{i}.md", i % 3),
            &engram(
                &format!("F{i}"),
                &format!("f{i}"),
                &format!("body{i} token"),
                &format!("f{}", (i + 1) % 7),
            ),
        );
    }
}

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

#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cs_{}_{}", std::process::id(), n)
}

/// Run a parity body against Turso (always) and Postgres (when configured). The
/// body takes two fresh stores because every case here compares one against the
/// other.
macro_rules! parity2 {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let a = crystalline_index::TursoStore::open_in_memory()
                    .await
                    .unwrap();
                let b = crystalline_index::TursoStore::open_in_memory()
                    .await
                    .unwrap();
                $body(&a, &b).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(url) = pg_url() {
                    let (sa, sb) = (unique_schema(), unique_schema());
                    let a = crystalline_index::PostgresStore::open_in_schema(&url, &sa)
                        .await
                        .expect("open the postgres test schema");
                    let b = crystalline_index::PostgresStore::open_in_schema(&url, &sb)
                        .await
                        .expect("open the postgres test schema");
                    $body(&a, &b).await;
                    a.drop_schema().await.expect("drop the postgres schema");
                    b.drop_schema().await.expect("drop the postgres schema");
                }
            }
        }
    };
}

fn params() -> ChunkParams {
    ChunkParams::default()
}

/// One full sync of `root` into `store` with an explicit slab size, driving the
/// two phases by hand exactly as the daemon does.
async fn sync_with_slab(
    store: &dyn Store,
    root: &Path,
    slab_files: usize,
) -> (SyncReport, crystalline_index::DomainId) {
    let domain = store
        .upsert_domain("d", Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap();
    let snapshot = store.file_stamps(domain).await.unwrap();
    let scan = scan_domain("d", root, snapshot, &params()).await.unwrap();
    let report = apply_scan_with_slab(store, domain, scan, slab_files)
        .await
        .unwrap();
    (report, domain)
}

/// Everything a sync writes for one domain, in a comparable shape: the engram
/// rows with content and checksum, the file stamps, the chunk rows with their
/// fingerprints and every outbound reference with its resolution state.
type Snapshot = (
    Vec<(String, String, String, String)>,
    Vec<(String, i64, u64, String)>,
    Vec<(String, i64, String, String)>,
    Vec<(String, usize, String, Option<String>, String, bool)>,
);

async fn snapshot(store: &dyn Store, domain: crystalline_index::DomainId) -> Snapshot {
    let mut engrams: Vec<(String, String, String, String)> = store
        .all_engram_contents(domain)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.path, e.permalink, e.content, e.sha256))
        .collect();
    engrams.sort();

    let mut stamps: Vec<(String, i64, u64, String)> = store
        .file_stamps(domain)
        .await
        .unwrap()
        .into_iter()
        .map(|(path, s)| (path, s.mtime, s.size, s.sha256))
        .collect();
    stamps.sort();

    // No embedding pass runs here, so every chunk row is still pending and the
    // backlog query is a complete listing of them. Keyed by permalink, never by
    // row id, because ids are assigned in apply order and the point of the test
    // is that the apply order may differ.
    let mut chunks: Vec<(String, i64, String, String)> = Vec::new();
    let mut refs: Vec<(String, usize, String, Option<String>, String, bool)> = Vec::new();
    let jobs = store
        .chunks_needing_embedding(&params().model_id, None, 100_000, None)
        .await
        .unwrap();
    for (_, permalink, _, _) in &engrams {
        let id = store.lookup_id("d", permalink).await.unwrap().unwrap();
        for job in jobs.iter().filter(|j| j.engram_id == id.0) {
            chunks.push((
                permalink.clone(),
                job.seq,
                job.text.clone(),
                job.text_hash.clone(),
            ));
        }
        for r in store.outbound_refs(id).await.unwrap() {
            refs.push((
                permalink.clone(),
                r.line,
                format!("{:?}", r.kind),
                r.rel_type,
                r.to_target,
                r.resolved,
            ));
        }
    }
    chunks.sort();
    refs.sort();
    (engrams, stamps, chunks, refs)
}

/// The domain counters, with the fields a sync cannot make deterministic (the
/// wall-clock last sync) left out.
fn counters(stats: &DomainStats) -> (i64, i64, i64, i64, i64, i64) {
    (
        stats.engrams,
        stats.observations,
        stats.relations,
        stats.unresolved_relations,
        stats.links,
        stats.unresolved_links,
    )
}

fn assert_same(left: &Snapshot, right: &Snapshot) {
    assert_eq!(left.0, right.0, "engram rows differ");
    assert_eq!(left.1, right.1, "file stamps differ");
    assert_eq!(left.2, right.2, "chunk rows differ");
    assert_eq!(left.3, right.3, "relations and links differ");
}

// --- parity bodies -----------------------------------------------------------

/// A cold sync of the fixture domain lands the identical store contents whether
/// the changed set is cut into slabs of two or applied in one shot.
async fn cold_sync_is_slab_independent(slabbed: &dyn Store, one_shot: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fixture(root);

    let (small, dom_small) = sync_with_slab(slabbed, root, 2).await;
    let (big, dom_big) = sync_with_slab(one_shot, root, 100_000).await;

    assert_eq!(small.added, 8, "seven engrams plus the MANIFEST: {small:?}");
    assert_eq!(small.added, big.added);
    assert_eq!(small.failed.len(), 0, "no failures: {:?}", small.failed);
    assert_eq!(
        (small.relations_resolved, small.links_resolved),
        (big.relations_resolved, big.links_resolved),
        "forward references resolve after the last slab, not per slab"
    );
    assert!(
        small.relations_resolved > 0,
        "the ring of relations resolved: {small:?}"
    );

    assert_same(
        &snapshot(slabbed, dom_small).await,
        &snapshot(one_shot, dom_big).await,
    );
    assert_eq!(
        counters(&slabbed.domain_stats().await.unwrap()[0]),
        counters(&one_shot.domain_stats().await.unwrap()[0]),
        "domain counters differ"
    );
    assert_eq!(
        slabbed.tag_aliases(None).await.unwrap(),
        one_shot.tag_aliases(None).await.unwrap(),
        "the MANIFEST tag aliases differ"
    );
}
parity2!(
    a_cold_sync_is_identical_across_slab_sizes,
    cold_sync_is_slab_independent
);

/// A second pass over a changed tree - one file deleted, one edited, one moved
/// and the rest untouched - is slab-independent too: the whole-domain deletion
/// and move classification survives being applied around slab boundaries.
async fn incremental_sync_is_slab_independent(slabbed: &dyn Store, one_shot: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fixture(root);

    let (_, dom_small) = sync_with_slab(slabbed, root, 2).await;
    let (_, dom_big) = sync_with_slab(one_shot, root, 100_000).await;

    // Change the tree: f0 is gone, f1 is edited, f2 moves to another folder and
    // the rest are untouched. Past the one-second mtime prefilter granularity so
    // the edit is seen.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::remove_file(root.join("dir0/f0.md")).unwrap();
    write(
        root,
        "dir1/f1.md",
        &engram("F1", "f1", "revised body1 token", "f2"),
    );
    std::fs::rename(root.join("dir2/f2.md"), root.join("dir0/moved.md")).unwrap();

    let (small, _) = sync_with_slab(slabbed, root, 2).await;
    let (big, _) = sync_with_slab(one_shot, root, 100_000).await;
    assert_eq!(
        (small.added, small.updated, small.deleted, small.moved),
        (big.added, big.updated, big.deleted, big.moved),
        "the classification counts differ: {small:?} vs {big:?}"
    );
    assert_eq!(
        (small.deleted, small.moved, small.updated),
        (1, 1, 1),
        "one delete, one move and one edit: {small:?}"
    );
    assert_eq!(small.deferred, 0, "an uncontended pass defers nothing");

    assert_same(
        &snapshot(slabbed, dom_small).await,
        &snapshot(one_shot, dom_big).await,
    );
    assert_eq!(
        counters(&slabbed.domain_stats().await.unwrap()[0]),
        counters(&one_shot.domain_stats().await.unwrap()[0]),
        "domain counters differ"
    );
}
parity2!(
    an_incremental_sync_is_identical_across_slab_sizes,
    incremental_sync_is_slab_independent
);

/// A file whose engram keeps its permalink but moves to a new path and is edited
/// in the same pass is a delete plus an add, not a rename: the delete has to land
/// before the first slab's upserts or the add collides with the permalink still
/// held by the old path. Pinned at a slab size of one, where the two ends can
/// never share a slab.
async fn delete_frees_the_permalink_before_the_slabs(store: &dyn Store, _unused: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "old/a.md", &engram("A", "a", "original body", "b"));
    write(root, "b.md", &engram("B", "b", "other body", "a"));
    let (seed, domain) = sync_with_slab(store, root, 1).await;
    assert_eq!(seed.added, 2);

    // Same permalink, new path, edited content: not a move, so the classifier
    // reports a delete and an add.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::remove_file(root.join("old/a.md")).unwrap();
    write(root, "new/a.md", &engram("A", "a", "revised body", "b"));

    let (report, _) = sync_with_slab(store, root, 1).await;
    assert_eq!(report.deleted, 1, "the old path was deleted: {report:?}");
    assert_eq!(report.added, 1, "the new path was added: {report:?}");
    assert_eq!(
        report.failed.len(),
        0,
        "the permalink was free by then: {:?}",
        report.failed
    );
    let engrams = store.all_engram_contents(domain).await.unwrap();
    assert_eq!(engrams.len(), 2, "two engrams remain");
    assert!(
        engrams.iter().any(|e| e.path == "new/a.md"),
        "the engram lives at its new path"
    );
}
parity2!(
    a_delete_frees_its_permalink_before_the_slabs_upsert,
    delete_frees_the_permalink_before_the_slabs
);

/// Every slab boundary lands the same store: sizes 1, 2, 3, 5 and one shot over a
/// changed set of eight files all agree with each other.
#[tokio::test]
async fn every_slab_size_lands_the_same_store() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fixture(root);

    let mut reference: Option<Snapshot> = None;
    for slab in [1usize, 2, 3, 5, 100_000] {
        let store = crystalline_index::TursoStore::open_in_memory()
            .await
            .unwrap();
        let (report, domain) = sync_with_slab(&store, root, slab).await;
        assert_eq!(report.added, 8, "slab {slab} indexed every file");
        let taken = snapshot(&store, domain).await;
        match &reference {
            None => reference = Some(taken),
            Some(first) => assert_same(&taken, first),
        }
    }
}
