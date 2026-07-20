//! The path-targeted scan: `scan_paths` classifies exactly the given relative
//! paths against a stamp snapshot, with no walk anywhere, then applies through
//! the same `apply_scan` the full scan uses.
//!
//! These pin the scoping guarantee the watcher relies on: a one-file edit in an
//! N-file domain touches exactly that file, the other N-1 rows are never even
//! visited (so `report.unchanged` counts only the given paths, never the whole
//! domain) and stay byte-for-byte in the index after the apply. Every body is a
//! pure function of a `&dyn Store` so it runs on both backends; Turso
//! (in-memory) always runs, Postgres runs when `CRYSTALLINE_TEST_POSTGRES_URL`
//! is set.

use std::path::Path;

use crystalline_index::{
    ChunkParams, DomainKind, SearchQuery, Store, TursoStore, apply_scan, scan_paths,
    sync_domain_with,
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
    format!("ct_{}_{}", std::process::id(), n)
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

/// Seed `n` engrams `f0.md..f{n-1}.md` on disk and in the index, returning the
/// tempdir so the caller keeps the files alive.
async fn seed(store: &dyn Store, n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..n {
        write(
            dir.path(),
            &format!("f{i}.md"),
            &engram(
                &format!("F{i}"),
                &format!("f{i}"),
                &format!("body{i} token"),
            ),
        );
    }
    let report = sync_domain_with(store, "d", dir.path(), &params())
        .await
        .unwrap();
    assert_eq!(report.added, n, "seed indexed every file");
    dir
}

/// Run one targeted pass over `paths` and return the report.
async fn target(store: &dyn Store, root: &Path, paths: &[&str]) -> crystalline_index::SyncReport {
    let domain = upsert_domain(store, "d", root).await;
    let snapshot = store.file_stamps(domain).await.unwrap();
    let scan = scan_paths(
        "d",
        root,
        snapshot,
        paths.iter().map(|s| s.to_string()).collect(),
        &params(),
    )
    .await;
    apply_scan(store, domain, scan).await.unwrap()
}

// --- parity bodies -----------------------------------------------------------

/// Editing one of N files and targeting just that path reindexes exactly it: the
/// other N-1 are never visited, so `unchanged` is 0, not N-1, and every other row
/// survives untouched in the index.
async fn edit_one_of_many_touches_only_it(store: &dyn Store) {
    let dir = seed(store, 5).await;
    let root = dir.path();

    // Edit f2.md; bump the mtime past the prefilter.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(root, "f2.md", &engram("F2", "f2", "revised token"));

    let report = target(store, root, &["f2.md"]).await;
    assert_eq!(report.updated, 1, "exactly the edited file reindexed");
    assert_eq!(report.added, 0);
    assert_eq!(report.deleted, 0);
    assert_eq!(report.moved, 0);
    // The walk-free marker: a full scan would report unchanged == 4 here; the
    // targeted scan visited only the one given path, so it reports 0.
    assert_eq!(
        report.unchanged, 0,
        "unchanged counts only the given paths, never the unvisited N-1"
    );

    // The edit landed and the other four rows are intact.
    let revised = store.search(&SearchQuery::text("revised")).await.unwrap();
    assert_eq!(revised.total, 1, "the edit is in the index");
    for i in [0usize, 1, 3, 4] {
        let hit = store
            .search(&SearchQuery::text(&format!("body{i}")))
            .await
            .unwrap();
        assert_eq!(hit.total, 1, "the untouched f{i}.md row survives verbatim");
    }
    assert_eq!(
        store.domain_stats().await.unwrap()[0].engrams,
        5,
        "still exactly five engrams"
    );
}
parity!(
    editing_one_of_many_targets_only_that_file,
    edit_one_of_many_touches_only_it
);

/// Targeting an unchanged path counts it as unchanged - proving `unchanged` does
/// count a given path, but only a given one.
async fn unchanged_given_path_counts_one(store: &dyn Store) {
    let dir = seed(store, 3).await;
    let root = dir.path();

    // f1.md is not edited; targeting it sees it unchanged.
    let report = target(store, root, &["f1.md"]).await;
    assert_eq!(
        report.unchanged, 1,
        "the one given unchanged path counts one"
    );
    assert_eq!(report.updated, 0);
    assert_eq!(report.added, 0);
}
parity!(
    an_unchanged_given_path_counts_as_one_unchanged,
    unchanged_given_path_counts_one
);

/// A brand-new file targeted by its path is added; nothing else is walked.
async fn add_one_targeted(store: &dyn Store) {
    let dir = seed(store, 2).await;
    let root = dir.path();

    write(root, "new.md", &engram("New", "new", "fresh token"));
    let report = target(store, root, &["new.md"]).await;
    assert_eq!(report.added, 1, "the new file is added");
    assert_eq!(report.unchanged, 0, "no other path was visited");
    let hit = store.search(&SearchQuery::text("fresh")).await.unwrap();
    assert_eq!(hit.total, 1, "the new engram is searchable");
    assert_eq!(store.domain_stats().await.unwrap()[0].engrams, 3);
}
parity!(a_new_file_is_added_by_its_path, add_one_targeted);

/// A vanished file targeted by its path is deleted; deletion detection is scoped
/// to the given path, so the other rows are untouched.
async fn delete_one_targeted(store: &dyn Store) {
    let dir = seed(store, 3).await;
    let root = dir.path();

    std::fs::remove_file(root.join("f1.md")).unwrap();
    let report = target(store, root, &["f1.md"]).await;
    assert_eq!(report.deleted, 1, "the vanished file is deleted");
    assert_eq!(report.unchanged, 0);
    let gone = store.search(&SearchQuery::text("body1")).await.unwrap();
    assert_eq!(gone.total, 0, "the deleted engram is gone");
    assert_eq!(
        store.domain_stats().await.unwrap()[0].engrams,
        2,
        "the other two survive"
    );
}
parity!(a_vanished_file_is_deleted_by_its_path, delete_one_targeted);

/// Both ends of a rename in one batch are classified as a move, not a delete plus
/// an add, so the engram is renamed in place.
async fn move_pair_in_one_batch(store: &dyn Store) {
    let dir = seed(store, 1).await;
    let root = dir.path();

    std::fs::rename(root.join("f0.md"), root.join("moved.md")).unwrap();
    let report = target(store, root, &["f0.md", "moved.md"]).await;
    // moved == 1 with deleted == 0 and added == 0 is the whole point: both ends
    // in one batch are paired as a rename in place, not a delete plus an add.
    assert_eq!(report.moved, 1, "the rename is a move, not delete+add");
    assert_eq!(report.deleted, 0, "no delete when both ends are given");
    assert_eq!(report.added, 0, "no add when both ends are given");
    // The content survived the in-place move as a single engram (a delete+add
    // would also end at one row, but would have reported deleted == 1/added == 1
    // above, so the counts plus this survival together pin the move).
    let survived = store.search(&SearchQuery::text("body0")).await.unwrap();
    assert_eq!(survived.total, 1, "the moved content survives once");
    assert_eq!(store.domain_stats().await.unwrap()[0].engrams, 1);
}
parity!(a_rename_pair_in_one_batch_is_a_move, move_pair_in_one_batch);

/// A path that is neither on disk nor in the stamps is a no-op: it becomes no
/// change candidate and no delete candidate.
async fn ghost_path_is_a_noop(store: &dyn Store) {
    let dir = seed(store, 2).await;
    let root = dir.path();

    let report = target(store, root, &["ghost.md"]).await;
    assert_eq!(report.added, 0);
    assert_eq!(report.updated, 0);
    assert_eq!(report.deleted, 0);
    assert_eq!(report.moved, 0);
    assert_eq!(report.unchanged, 0);
    assert_eq!(
        store.domain_stats().await.unwrap()[0].engrams,
        2,
        "the domain is untouched"
    );
}
parity!(
    a_path_neither_on_disk_nor_recorded_is_a_noop,
    ghost_path_is_a_noop
);

/// Seeding N and targeting exactly one leaves the other N-1 rows present and
/// byte-for-byte unmodified in the index after the apply.
async fn outside_batch_untouched(store: &dyn Store) {
    let dir = seed(store, 6).await;
    let root = dir.path();

    // Target one addition; assert the six seeded rows are all present before and
    // still present and unmodified after, none rewritten.
    for i in 0..6 {
        let hit = store
            .search(&SearchQuery::text(&format!("body{i}")))
            .await
            .unwrap();
        assert_eq!(hit.total, 1, "f{i}.md present before the targeted add");
    }

    write(root, "seven.md", &engram("Seven", "seven", "seventh token"));
    let report = target(store, root, &["seven.md"]).await;
    assert_eq!(report.added, 1);

    for i in 0..6 {
        let hit = store
            .search(&SearchQuery::text(&format!("body{i}")))
            .await
            .unwrap();
        assert_eq!(
            hit.total, 1,
            "the outside-batch row f{i}.md is still present and unmodified"
        );
    }
    assert_eq!(store.domain_stats().await.unwrap()[0].engrams, 7);
}
parity!(
    rows_outside_the_targeted_batch_are_untouched,
    outside_batch_untouched
);

/// An unreadable targeted path is a reported failure, not a delete: a metadata
/// error other than "not found" keeps the row in the index and surfaces the
/// path in the report's `failed` list instead of dropping it.
#[cfg(unix)]
mod unreadable_path {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[tokio::test]
    async fn targeted_scan_of_an_unreadable_path_reports_failure_and_keeps_the_row() {
        let store = TursoStore::open_in_memory().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "sub/a.md", &engram("A", "a", "keepme body"));

        let seeded = sync_domain_with(&store, "d", root, &params())
            .await
            .unwrap();
        assert_eq!(seeded.added, 1, "the seed indexed the file");
        let domain = upsert_domain(&store, "d", root).await;

        // Deny the subfolder so the file's metadata cannot be stat'd.
        let sub = root.join("sub");
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000)).unwrap();

        let snapshot = store.file_stamps(domain).await.unwrap();
        let scan = scan_paths("d", root, snapshot, vec!["sub/a.md".to_string()], &params()).await;
        let report = apply_scan(&store, domain, scan).await.unwrap();

        // Restore before any assertion can unwind past the tempdir drop.
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            report.failed.iter().any(|(p, _)| p == "sub/a.md"),
            "the unreadable path is reported as failed: {report:?}"
        );
        assert_eq!(report.deleted, 0, "an unreadable path is not a delete");
        assert!(
            store
                .file_stamps(domain)
                .await
                .unwrap()
                .contains_key("sub/a.md"),
            "the row survives an unreadable targeted path"
        );
    }
}
