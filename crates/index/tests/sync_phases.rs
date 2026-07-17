//! The lock-phased sync: `scan_domain` (filesystem, no store) then `apply_scan`
//! (transactional, store only), and the TOCTOU guards that make a concurrent
//! writer between the two deterministic to test without timing games.
//!
//! The guard tests drive the two phases by hand: snapshot the stamps, scan,
//! mutate the store as a stand-in for a concurrent writer (an MCP edit or a
//! second collaboration-mode instance), then apply. Every guard test asserts the
//! stale change is deferred, the concurrent state survives and the next full sync
//! converges. Every body is a pure function of a `&dyn Store` so it runs on both
//! backends; Turso (in-memory) always runs, Postgres runs when
//! `CRYSTALLINE_TEST_POSTGRES_URL` is set.

use std::path::Path;

use crystalline_index::{
    ChunkParams, DomainKind, EngramRecord, FileStamp, SearchQuery, Store, TursoStore, apply_scan,
    scan_domain, sync_domain_with,
};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// A minimal engram markdown block with a searchable body token.
fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A minimal engram record with explicit content and checksum, built without
/// parsing so a test can write it straight through the store as a stand-in for a
/// concurrent writer. `sha` becomes the stamp checksum, so passing a fresh value
/// moves the db stamp exactly as a real mid-scan write would.
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

#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cp_{}_{}", std::process::id(), n)
}

/// Run a parity body against Turso (always) and Postgres (when configured).
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

fn params() -> ChunkParams {
    ChunkParams::default()
}

async fn upsert_domain(store: &dyn Store, name: &str, root: &Path) -> crystalline_index::DomainId {
    store
        .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap()
}

// --- parity bodies -----------------------------------------------------------

/// A change the scan classified against a stale snapshot is deferred when a
/// concurrent writer indexed the same path first; the concurrent content wins
/// and the next full sync brings the file on disk back.
async fn concurrent_write_defers(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // A file on disk the scan classifies as new against the empty snapshot.
    write(root, "a.md", &engram("A", "a", "diskwins body"));

    let domain = upsert_domain(store, "d", root).await;
    let snapshot = store.file_stamps(domain).await.unwrap();
    assert!(snapshot.is_empty(), "nothing indexed yet");
    let scan = scan_domain("d", root, snapshot, &params()).await;

    // A concurrent writer indexes the same path with different content between
    // the snapshot and the apply.
    store
        .upsert_engram(
            domain,
            &record("a.md", "a", "concurrentwins body", "sha-conc"),
        )
        .await
        .unwrap();

    let report = apply_scan(store, domain, scan).await.unwrap();
    assert_eq!(report.deferred, 1, "the stale add was deferred: {report:?}");
    assert_eq!(report.added, 0, "the stale add did not land");

    // The concurrent writer's content survives; the scan's stale content did not
    // overwrite it.
    let live = store
        .search(&SearchQuery::text("concurrentwins"))
        .await
        .unwrap();
    assert_eq!(live.total, 1, "the concurrent write survives");
    let stale = store.search(&SearchQuery::text("diskwins")).await.unwrap();
    assert_eq!(stale.total, 0, "the scan's stale content did not land");

    // The next full sync converges: the file on disk wins again.
    let converge = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(converge.deferred, 0, "the converging pass is uncontended");
    let now = store.search(&SearchQuery::text("diskwins")).await.unwrap();
    assert_eq!(now.total, 1, "the file on disk wins on the next pass");
}
parity!(
    concurrent_write_defers_the_stale_scan_result,
    concurrent_write_defers
);

/// A delete the scan classified is skipped when the row was rewritten in the db
/// between the snapshot and the apply.
async fn concurrent_rewrite_skips_delete(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "original body"));
    sync_domain_with(store, "d", root, &params()).await.unwrap();
    let domain = upsert_domain(store, "d", root).await;

    // The file vanishes from disk, so a scan classifies it as a delete.
    std::fs::remove_file(root.join("a.md")).unwrap();
    let snapshot = store.file_stamps(domain).await.unwrap();
    assert!(snapshot.contains_key("a.md"), "a.md still recorded");
    let scan = scan_domain("d", root, snapshot, &params()).await;

    // A concurrent writer rewrites the row (a new stamp) mid-window.
    store
        .upsert_engram(domain, &record("a.md", "a", "rewrittenrow body", "sha-new"))
        .await
        .unwrap();

    let report = apply_scan(store, domain, scan).await.unwrap();
    assert_eq!(
        report.deferred, 1,
        "the stale delete was deferred: {report:?}"
    );
    assert_eq!(report.deleted, 0, "the row was not deleted");
    let live = store
        .search(&SearchQuery::text("rewrittenrow"))
        .await
        .unwrap();
    assert_eq!(live.total, 1, "the rewritten row survives");
}
parity!(
    concurrent_rewrite_skips_the_scan_delete,
    concurrent_rewrite_skips_delete
);

/// A delete the scan classified is skipped when the file reappeared on disk
/// before the apply, even though the db stamp never moved: the disk-side guard.
async fn recreated_file_skips_delete(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "recreated body"));
    sync_domain_with(store, "d", root, &params()).await.unwrap();
    let domain = upsert_domain(store, "d", root).await;

    // The file vanishes, the scan classifies the delete, then it reappears on
    // disk before the apply. The db is never touched, so only the disk re-stat
    // can catch this.
    std::fs::remove_file(root.join("a.md")).unwrap();
    let snapshot = store.file_stamps(domain).await.unwrap();
    let scan = scan_domain("d", root, snapshot, &params()).await;
    write(root, "a.md", &engram("A", "a", "recreated body"));

    let report = apply_scan(store, domain, scan).await.unwrap();
    assert_eq!(
        report.deferred, 1,
        "the delete was deferred because the file reappeared: {report:?}"
    );
    assert_eq!(report.deleted, 0, "the row was not deleted");
    let live = store.search(&SearchQuery::text("recreated")).await.unwrap();
    assert_eq!(live.total, 1, "the row survives the reappearance");
}
parity!(
    recreated_file_on_disk_skips_the_delete,
    recreated_file_skips_delete
);

/// A move the scan classified is skipped when either end's db stamp moved
/// mid-window; both ends are left for the next pass, which converges.
async fn move_with_moved_end_defers(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "old.md", &engram("Mover", "mover", "moveme body"));
    sync_domain_with(store, "d", root, &params()).await.unwrap();
    let domain = upsert_domain(store, "d", root).await;

    // Rename on disk: identical bytes at a new path, so the scan classifies a
    // move.
    std::fs::rename(root.join("old.md"), root.join("new.md")).unwrap();
    let snapshot = store.file_stamps(domain).await.unwrap();
    let scan = scan_domain("d", root, snapshot, &params()).await;

    // A concurrent writer mutates the move's `from` end mid-window.
    store
        .upsert_engram(
            domain,
            &record("old.md", "mover", "mutatedend body", "sha-mut"),
        )
        .await
        .unwrap();

    let report = apply_scan(store, domain, scan).await.unwrap();
    assert_eq!(report.deferred, 1, "the move was deferred: {report:?}");
    assert_eq!(report.moved, 0, "the move did not land");

    // The next full sync converges on the file on disk: the original content is
    // present once, the concurrent mutation is gone and only one engram remains.
    let converge = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(converge.deferred, 0, "the converging pass is uncontended");
    assert!(
        store.lookup_id("d", "mover").await.unwrap().is_some(),
        "the engram survives under its permalink"
    );
    let moved = store.search(&SearchQuery::text("moveme")).await.unwrap();
    assert_eq!(moved.total, 1, "the moved content is present once");
    let stale = store
        .search(&SearchQuery::text("mutatedend"))
        .await
        .unwrap();
    assert_eq!(stale.total, 0, "the stale concurrent mutation is gone");
    assert_eq!(
        store.domain_stats().await.unwrap()[0].engrams,
        1,
        "exactly one engram after convergence"
    );
}
parity!(
    move_with_a_moved_end_is_deferred,
    move_with_moved_end_defers
);

/// The composition path defers nothing when uncontended: a fresh add, a warm
/// no-change pass, an edit, a delete and a move each report their usual counts
/// with `deferred == 0`.
async fn composition_identical_uncontended(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "alpha body"));
    write(root, "b.md", &engram("B", "b", "beta body"));

    let added = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(added.added, 2);
    assert_eq!(added.deferred, 0);

    let warm = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(warm.unchanged, 2);
    assert_eq!(warm.added, 0);
    assert_eq!(warm.deferred, 0);

    // Edit a.md; bump the mtime past the prefilter.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(root, "a.md", &engram("A", "a", "revised body"));
    let edited = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(edited.updated, 1);
    assert_eq!(edited.deferred, 0);

    // Delete b.md.
    std::fs::remove_file(root.join("b.md")).unwrap();
    let deleted = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(deleted.deleted, 1);
    assert_eq!(deleted.deferred, 0);

    // Move a.md to c.md.
    std::fs::rename(root.join("a.md"), root.join("c.md")).unwrap();
    let moved = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(moved.moved, 1);
    assert_eq!(moved.deferred, 0);
}
parity!(
    the_composition_is_identical_when_uncontended,
    composition_identical_uncontended
);
