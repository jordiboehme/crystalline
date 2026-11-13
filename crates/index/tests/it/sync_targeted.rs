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
    ChunkParams, DomainKind, PathChangeKind, ReflectionPathChange, ReflectionPathKind, SearchQuery,
    Store, TursoStore, apply_scan, scan_paths, sync_domain_with,
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
                if let Some(scratch) = crate::pg::scratch("ct") {
                    let store = crystalline_index::PostgresStore::open_in_schema(
                        scratch.url(),
                        scratch.name(),
                    )
                    .await
                    .expect("open the postgres test schema");
                    $body(&store).await;
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

/// The report names every path the apply moved, with the permalink and the
/// checksum a subscriber needs, and names nothing for a path it only counted
/// as unchanged. Four kinds in one pass: a revised file, a renamed one, a
/// deleted one and a new one.
async fn the_report_names_every_path_it_moved(store: &dyn Store) {
    let dir = seed(store, 3).await;
    let root = dir.path();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(root, "f1.md", &engram("F1", "f1", "revised token"));
    std::fs::rename(root.join("f2.md"), root.join("g2.md")).unwrap();
    std::fs::remove_file(root.join("f0.md")).unwrap();
    write(root, "new.md", &engram("New", "brand-new", "fresh token"));

    let report = target(store, root, &["f0.md", "f1.md", "f2.md", "g2.md", "new.md"]).await;
    let mut kinds: Vec<(PathChangeKind, String)> = report
        .changes
        .iter()
        .map(|change| (change.kind, change.path.clone()))
        .collect();
    kinds.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(
        kinds,
        vec![
            (PathChangeKind::Deleted, "f0.md".to_string()),
            (PathChangeKind::Modified, "f1.md".to_string()),
            (PathChangeKind::Moved, "g2.md".to_string()),
            (PathChangeKind::Added, "new.md".to_string()),
        ]
    );
    let by_kind = |kind: PathChangeKind| report.changes.iter().find(|c| c.kind == kind).unwrap();
    let moved = by_kind(PathChangeKind::Moved);
    assert_eq!(moved.from.as_deref(), Some("f2.md"));
    assert_eq!(moved.from_permalink.as_deref(), Some("f2"));
    // `f2` was in step with its path, so the store lets it follow the move
    // (`Store::rename_engram`); the report reads it back after the rename
    // rather than carrying the old one forward.
    assert_eq!(
        moved.permalink.as_deref(),
        Some("g2"),
        "a path-derived permalink follows the rename"
    );
    assert!(
        moved.checksum.is_some(),
        "a rename keeps the content's checksum"
    );
    let deleted = by_kind(PathChangeKind::Deleted);
    assert_eq!(
        deleted.permalink.as_deref(),
        Some("f0"),
        "read before the row went"
    );
    assert!(deleted.checksum.is_none());
    let domain = upsert_domain(store, "d", root).await;
    let stamps = store.file_stamps(domain).await.unwrap();
    let added = by_kind(PathChangeKind::Added);
    assert_eq!(added.permalink.as_deref(), Some("brand-new"));
    assert_eq!(
        added.checksum,
        stamps.get("new.md").map(|s| s.sha256.clone())
    );
    let modified = by_kind(PathChangeKind::Modified);
    assert_eq!(
        modified.checksum,
        stamps.get("f1.md").map(|s| s.sha256.clone())
    );

    // An unchanged path is counted and never named.
    let quiet = target(store, root, &["f1.md"]).await;
    assert_eq!(quiet.unchanged, 1);
    assert!(quiet.changes.is_empty(), "{:?}", quiet.changes);
}
parity!(
    the_report_names_every_path_the_apply_moved,
    the_report_names_every_path_it_moved
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

// --- attachments -------------------------------------------------------------

/// A few bytes that are not valid UTF-8, so a test asset is genuinely binary.
const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00];

fn write_bytes(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// A targeted pass over one asset path touches exactly that attachment: the
/// rewritten file's row refreshes, every other row stays as the full scan left
/// it, and targeting a path whose file is gone removes just its row.
async fn targeted_asset_touches_only_it(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "body token"));
    write_bytes(root, "assets/shot.png", PNG);
    write_bytes(root, "assets/deck.pdf", b"%PDF-1.7 deck");
    sync_domain_with(store, "d", root, &params()).await.unwrap();

    let domain = upsert_domain(store, "d", root).await;
    let before = store
        .get_attachment(domain, "assets/deck.pdf")
        .await
        .unwrap()
        .expect("the pdf row exists");

    // A rewrite of a different length moves the size half of the stat prefilter,
    // so the targeted pass hashes the file again whatever the mtime resolution is.
    let rewritten: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x02, 0x03, 0x04,
    ];
    write_bytes(root, "assets/shot.png", rewritten);
    target(store, root, &["assets/shot.png"]).await;

    let shot = store
        .get_attachment(domain, "assets/shot.png")
        .await
        .unwrap()
        .expect("the png row exists");
    assert_eq!(
        shot.size,
        rewritten.len() as u64,
        "the targeted row refreshed"
    );
    assert_eq!(
        store
            .get_attachment(domain, "assets/deck.pdf")
            .await
            .unwrap()
            .expect("the pdf row survives"),
        before,
        "a row outside the targeted batch is untouched"
    );

    // A targeted path whose file is gone deletes just that row.
    std::fs::remove_file(root.join("assets/deck.pdf")).unwrap();
    target(store, root, &["assets/deck.pdf"]).await;
    let rows: Vec<String> = store
        .list_attachments(domain)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(rows, ["assets/shot.png"], "only the targeted row is gone");
}
parity!(
    a_targeted_asset_path_touches_only_it,
    targeted_asset_touches_only_it
);

/// A symlinked attachment is indexed by neither entry point.
///
/// The complete walk uses `WalkDir`'s default `follow_links(false)`, so it
/// never sees a symlink at all. The targeted pass has to take the same posture
/// or the two disagree: a link indexed by a watcher event would lose its row on
/// the next full scan and get it back on the next event, a row that exists
/// depending on which pass ran last.
#[cfg(unix)]
mod symlinked_asset {
    use std::os::unix::fs::symlink;

    use super::*;

    async fn a_symlink_is_indexed_by_neither_pass(store: &dyn Store) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", &engram("A", "a", "body token"));
        write_bytes(root, "outside.png", PNG);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        symlink(root.join("outside.png"), root.join("assets/link.png")).unwrap();

        // The full scan: nothing, because it does not follow links.
        sync_domain_with(store, "d", root, &params()).await.unwrap();
        let domain = upsert_domain(store, "d", root).await;
        assert!(
            store.list_attachments(domain).await.unwrap().is_empty(),
            "the complete walk indexes nothing through a symlink"
        );

        // The targeted pass, the one a watcher event drives: the same answer,
        // and no failure reported either.
        let report = target(store, root, &["assets/link.png"]).await;
        assert!(report.failed.is_empty(), "no failures: {report:?}");
        assert!(
            store.list_attachments(domain).await.unwrap().is_empty(),
            "a targeted pass must not index what the walk refuses to see"
        );

        // A real file beside it still indexes, so this is a symlink rule and
        // not a broken assets branch.
        write_bytes(root, "assets/shot.png", PNG);
        target(store, root, &["assets/shot.png"]).await;
        let rows: Vec<String> = store
            .list_attachments(domain)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert_eq!(rows, ["assets/shot.png"]);
    }
    parity!(
        a_symlinked_attachment_is_indexed_by_neither_pass,
        a_symlink_is_indexed_by_neither_pass
    );
}

/// The engram rows and the attachment rows of domain `d`, each sorted.
async fn rows(store: &dyn Store, root: &Path) -> (Vec<String>, Vec<String>) {
    let domain = upsert_domain(store, "d", root).await;
    let mut engrams: Vec<String> = store
        .file_stamps(domain)
        .await
        .unwrap()
        .into_keys()
        .collect();
    engrams.sort();
    let mut assets: Vec<String> = store
        .list_attachments(domain)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assets.sort();
    (engrams, assets)
}

/// The OKF reserved names under `assets/` are neither engrams nor attachments,
/// for the full walk and for a targeted pass, present or vanished. The layout
/// calls them reserved, not attachments; markdown was never attachable, so
/// nothing about either pass moves.
async fn reserved_names_under_assets_index_nothing(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "body token"));
    write(root, "assets/index.md", "# Assets\n");
    write(root, "assets/log.md", "# Log\n");
    write_bytes(root, "assets/shot.png", PNG);
    sync_domain_with(store, "d", root, &params()).await.unwrap();
    let expected = (
        vec!["a.md".to_string()],
        vec!["assets/shot.png".to_string()],
    );
    assert_eq!(rows(store, root).await, expected, "the full walk");

    let present = target(store, root, &["assets/index.md", "assets/log.md"]).await;
    assert!(
        present.changes.is_empty() && present.failed.is_empty(),
        "present: {present:?}"
    );
    assert_eq!(
        rows(store, root).await,
        expected,
        "a targeted pass, present"
    );

    std::fs::remove_file(root.join("assets/index.md")).unwrap();
    std::fs::remove_file(root.join("assets/log.md")).unwrap();
    let gone = target(store, root, &["assets/index.md", "assets/log.md"]).await;
    assert!(
        gone.changes.is_empty() && gone.failed.is_empty() && gone.deleted == 0,
        "vanished: {gone:?}"
    );
    assert_eq!(
        rows(store, root).await,
        expected,
        "a targeted pass, vanished"
    );
}
parity!(
    reserved_names_under_assets_index_nothing_on_either_pass,
    reserved_names_under_assets_index_nothing
);

/// A vanished OKF reserved name outside `assets/` is a no-op for a targeted
/// pass: it was never recorded, so it is no delete and no failure, and every
/// row stays.
async fn a_vanished_reserved_name_is_a_no_op(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "body token"));
    write(root, "index.md", "# Index\n");
    write(root, "sub/log.md", "# Log\n");
    write(root, "sub/b.md", &engram("B", "sub/b", "body token"));
    sync_domain_with(store, "d", root, &params()).await.unwrap();
    let expected = (vec!["a.md".to_string(), "sub/b.md".to_string()], Vec::new());
    assert_eq!(rows(store, root).await, expected, "the full walk");

    std::fs::remove_file(root.join("index.md")).unwrap();
    std::fs::remove_file(root.join("sub/log.md")).unwrap();
    let report = target(store, root, &["index.md", "sub/log.md"]).await;
    assert!(
        report.changes.is_empty() && report.failed.is_empty() && report.deleted == 0,
        "{report:?}"
    );
    assert_eq!(rows(store, root).await, expected, "every row stays");
}
parity!(
    a_vanished_reserved_name_is_a_no_op_for_a_targeted_pass,
    a_vanished_reserved_name_is_a_no_op
);

/// An attachment indexed before the MANIFEST declared an artifact folder around
/// it keeps its row until the file goes; once it is gone, a targeted pass
/// deletes the row at once, as it always did, rather than leaving it for the
/// next full walk.
async fn an_artifact_folder_under_assets_still_deletes(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.md", &engram("A", "a", "body token"));
    write_bytes(root, "assets/deploy/x.png", PNG);
    sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(
        rows(store, root).await.1,
        ["assets/deploy/x.png"],
        "indexed before the decl"
    );

    write(
        root,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: KB\npermalink: manifest\ntags: [manifest]\nstatus: stable\n---\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n\n## Provisioning\n\n- skills: assets/deploy\n",
    );
    assert!(
        !crystalline_core::in_root_artifact_dirs(root).is_empty(),
        "the decl names an in-root artifact folder"
    );
    std::fs::remove_file(root.join("assets/deploy/x.png")).unwrap();
    target(store, root, &["assets/deploy/x.png"]).await;
    assert!(
        rows(store, root).await.1.is_empty(),
        "the vanished attachment's row is gone"
    );
}
parity!(
    a_vanished_attachment_in_an_artifact_folder_under_assets_is_deleted,
    an_artifact_folder_under_assets_still_deletes
);

/// An unreadable targeted path is a reported failure, not a delete: a metadata
/// error other than "not found" keeps the row in the index and surfaces the
/// path in the report's `failed` list instead of dropping it.
#[cfg(unix)]
mod unreadable_path {
    use crystalline_index::ReadStore;
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

/// The probe that asks for a whole walk when a Reflection has no row
/// applies the walk's own rules: a file the walk skips (here an attachment
/// over the size cap) asks for nothing, or every check would walk again.
#[test]
fn the_reflection_disk_probe_skips_what_the_walk_skips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("reflections/assets")).unwrap();
    let big = std::fs::File::create(root.join("reflections/assets/huge.png")).unwrap();
    big.set_len(crystalline_core::MAX_ATTACHMENT_BYTES + 1)
        .unwrap();
    assert!(
        !crystalline_index::reflection_files_on_disk(root),
        "an attachment over the cap gets no row"
    );
    std::fs::write(root.join("reflections/assets/small.png"), b"png").unwrap();
    assert!(crystalline_index::reflection_files_on_disk(root));
}

// --- the Reflection's changes, named for the change bus ----------------------

const MANIFEST_ON: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags: [manifest]\nstatus: stable\nreflection: on\n---\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n";

/// A page carries a section marker, so the walk reads it as a page and the
/// Reflection is in effect from the first sync.
const PAGE_ONE: &str = "# A\n\n## One\n<!-- r:aaaa1111 -->\n\nfirst\n";
const PAGE_TWO: &str = "# A\n\n## One\n<!-- r:aaaa1111 -->\n\nsecond, longer\n";

fn sha(bytes: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn change(path: &str, kind: ReflectionPathKind, checksum: Option<String>) -> ReflectionPathChange {
    ReflectionPathChange {
        path: path.to_string(),
        kind,
        checksum,
    }
}

/// A threads file touched on disk is reported as its page, by the page's
/// path inside `reflections/`, with the threads file's checksum; it gets no
/// row. A page edited or deleted on disk is named with its own kind, and a
/// file under `reflections/assets/` as a file.
async fn reflection_changes_on_disk_are_named(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "MANIFEST.md", MANIFEST_ON);
    write(root, "reflections/a.md", PAGE_ONE);
    write(root, "reflections/a.threads.md", "# Threads\n");
    let seeded = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(
        seeded.reflection_changes,
        vec![change(
            "a.md",
            ReflectionPathKind::Changed,
            Some(sha(PAGE_ONE))
        )],
        "the walk names the page it indexed; the engine decides whether that is the switch"
    );
    let walk = seeded
        .reflection_walk
        .expect("a complete walk says what it saw");
    assert!(walk.effective);
    assert_eq!(
        walk.threads.into_iter().collect::<Vec<_>>(),
        vec![("a.md".to_string(), sha("# Threads\n"))],
        "every threads file by its page, with its checksum"
    );
    assert_eq!(walk.pages.into_iter().collect::<Vec<_>>(), vec!["a.md"]);

    let threads = "# Threads\n\n## t1\n\nA comment from outside.\n";
    write(root, "reflections/a.threads.md", threads);
    let report = target(store, root, &["reflections/a.threads.md"]).await;
    assert_eq!(
        report.reflection_changes,
        vec![change(
            "a.md",
            ReflectionPathKind::Threads,
            Some(sha(threads))
        )]
    );
    assert_eq!(report.reflections_changed, 0, "a threads file is no row");
    assert_eq!(
        report.reflection_walk, None,
        "a targeted pass walks nothing"
    );
    let domain = upsert_domain(store, "d", root).await;
    let rows: Vec<String> = store
        .list_reflection_stamps(domain)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(
        rows,
        vec!["a.md".to_string()],
        "no row for the threads file"
    );

    std::fs::remove_file(root.join("reflections/a.threads.md")).unwrap();
    let report = target(store, root, &["reflections/a.threads.md"]).await;
    assert_eq!(
        report.reflection_changes,
        vec![change("a.md", ReflectionPathKind::Threads, None)],
        "threads removed by hand beside a page that stays"
    );

    write(root, "reflections/a.md", PAGE_TWO);
    let report = target(store, root, &["reflections/a.md"]).await;
    assert_eq!(
        report.reflection_changes,
        vec![change(
            "a.md",
            ReflectionPathKind::Changed,
            Some(sha(PAGE_TWO))
        )]
    );

    write(root, "reflections/b.md", PAGE_ONE);
    let walked = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(
        walked.reflection_changes,
        vec![change(
            "b.md",
            ReflectionPathKind::Changed,
            Some(sha(PAGE_ONE))
        )],
        "a full walk names the page it found and never a threads file"
    );

    write_bytes(root, "reflections/assets/p.png", b"png");
    let report = target(store, root, &["reflections/assets/p.png"]).await;
    assert_eq!(
        report.reflection_changes,
        vec![change(
            "assets/p.png",
            ReflectionPathKind::Files,
            Some(sha(b"png"))
        )]
    );

    write(root, "reflections/a.threads.md", threads);
    std::fs::remove_file(root.join("reflections/a.md")).unwrap();
    std::fs::remove_file(root.join("reflections/a.threads.md")).unwrap();
    let report = target(
        store,
        root,
        &["reflections/a.md", "reflections/a.threads.md"],
    )
    .await;
    assert_eq!(
        report.reflection_changes,
        vec![change("a.md", ReflectionPathKind::Deleted, None)],
        "a page that went with its threads is one deletion"
    );
}
parity!(
    reflection_changes_found_on_disk_are_named_for_the_bus,
    reflection_changes_on_disk_are_named
);

/// A targeted pass reads a threads path only when it is a regular file, as
/// the walk does: a link is not followed and a pipe is never opened (the
/// read would block the pass).
#[cfg(unix)]
#[tokio::test]
async fn a_threads_path_that_is_a_link_or_a_pipe_is_not_read() {
    let store = TursoStore::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "MANIFEST.md", MANIFEST_ON);
    write(root, "reflections/a.md", PAGE_ONE);
    write(root, "reflections/b.md", PAGE_ONE);
    write(root, "outside.txt", "# Threads\n");
    sync_domain_with(&store, "d", root, &params())
        .await
        .unwrap();
    std::os::unix::fs::symlink(
        root.join("outside.txt"),
        root.join("reflections/a.threads.md"),
    )
    .unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(root.join("reflections/b.threads.md"))
        .status()
        .unwrap();
    assert!(made.success());
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        target(
            &store,
            root,
            &["reflections/a.threads.md", "reflections/b.threads.md"],
        ),
    )
    .await
    .expect("the pass does not block on the pipe");
    assert_eq!(report.reflection_changes, Vec::new());
}
