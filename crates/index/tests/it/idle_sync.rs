//! A sync of a domain whose files did not change writes nothing to the index.
//!
//! The daemon syncs a domain on every watcher flush, every catch-up and every
//! `crystalline sync`, and most of those find nothing new. Each one used to
//! rewrite the domain row, the tag-alias map and the sync stamp, which on an
//! idle home server added up to gigabytes a day. These pin that an unchanged
//! sync is read-only, on the full walk and on the targeted path.
//!
//! The signal on Turso is the write-ahead log of a file-backed store: the
//! store runs in WAL mode, so every transaction that commits a changed page
//! appends at least one frame (a 24-byte frame header and the page) to
//! `<db>-wal` before it commits, while a read, or a `BEGIN`/`COMMIT` with
//! nothing dirty, appends none. The log is truncated with a checkpoint after
//! the first sync, so a later sync that wrote anything leaves frames in it.
//! The 32-byte log header alone is not a frame: the engine writes it into an
//! emptied log once, when a statement that could write first runs, whether
//! or not it changes a page. Each test has its own database file, so nothing
//! else writes to it, and each also edits one file at the end and checks
//! that frames appear, so the signal is known to see a real write.
//!
//! Postgres has no such file, so the parity body there reads the system
//! columns of every row in the test's own schema: an insert, an update, a
//! delete or a row lock each change the set of `(ctid, xmin, xmax)` triples,
//! and a statement that would only have burned a sequence value moves the
//! sequence. Both are exact at the moment they are read, unlike the
//! statistics views, which the server flushes lazily.

use std::path::{Path, PathBuf};

use crystalline_index::{
    ChunkParams, DomainKind, Store, TursoStore, apply_scan, scan_paths, sync_domain_with,
};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A domain that exercises every part of the apply: a MANIFEST with tag
/// aliases, a resolved and a dangling wikilink, a relation, and an
/// attachment under `assets/`.
fn seed_files(root: &Path) {
    write(
        root,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: Manifest\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Manifest\n\n## Scope\n\n- covers things\n\n## When to Use\n\n- when routing\n\n## Tag Aliases\n\n- old -> new\n",
    );
    write(
        root,
        "a.md",
        &engram(
            "Alpha",
            "alpha",
            "links to [[Beta]] and to [[Nowhere Yet]]\n\n## Relations\n\n- relates_to [[Beta]]\n- relates_to [[Missing Page]]",
        ),
    );
    write(root, "b.md", &engram("Beta", "beta", "plain body"));
    write(root, "notes/c.md", &engram("Gamma", "gamma", "nested body"));
    write(root, "assets/deck.pdf", "%PDF-1.7 deck");
}

fn params() -> ChunkParams {
    ChunkParams::default()
}

/// The size of the log's 32-byte file header, which holds no page.
const WAL_HEADER: u64 = 32;

/// The bytes of frames in the database's write-ahead log: zero when no
/// transaction committed a page since the last checkpoint emptied it.
fn wal_frames(db: &Path) -> u64 {
    let mut wal = db.as_os_str().to_os_string();
    wal.push("-wal");
    std::fs::metadata(PathBuf::from(wal))
        .map(|m| m.len())
        .unwrap_or(0)
        .saturating_sub(WAL_HEADER)
}

/// A file-backed store and its database path, synced once and checkpointed,
/// so the log is empty when the test starts measuring.
async fn synced_store(root: &Path, db_dir: &Path) -> (TursoStore, PathBuf) {
    let db = db_dir.join("index.db");
    let store = TursoStore::open(&db).await.unwrap();
    seed_files(root);
    let report = sync_domain_with(&store, "d", root, &params())
        .await
        .unwrap();
    assert_eq!(report.added, 4, "the first sync indexed every engram");
    store.checkpoint_wal().await.unwrap();
    assert_eq!(wal_frames(&db), 0, "the checkpoint emptied the log");
    (store, db)
}

/// Edit one file past the stat prefilter and sync it, so the test proves its
/// signal sees a real write.
async fn edit_grows_the_log(store: &TursoStore, root: &Path, db: &Path) {
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(root, "b.md", &engram("Beta", "beta", "edited body"));
    let report = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(report.updated, 1);
    assert!(
        wal_frames(db) > 0,
        "a sync that changed a file wrote to the log"
    );
}

#[tokio::test]
async fn an_unchanged_full_sync_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let (store, db) = synced_store(root.path(), db_dir.path()).await;

    for _ in 0..3 {
        let report = sync_domain_with(&store, "d", root.path(), &params())
            .await
            .unwrap();
        assert_eq!(report.unchanged, 4);
        assert_eq!(
            report.added + report.updated + report.deleted + report.moved,
            0
        );
    }
    assert_eq!(
        wal_frames(&db),
        0,
        "an unchanged full sync wrote to the index"
    );

    edit_grows_the_log(&store, root.path(), &db).await;
}

#[tokio::test]
async fn an_unchanged_targeted_sync_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let (store, db) = synced_store(root.path(), db_dir.path()).await;

    // The watcher's path: the domain row, the stamps, then a scan of exactly
    // the paths it heard about, including the MANIFEST and an attachment.
    for _ in 0..3 {
        let domain = store
            .upsert_domain("d", Some(&root.path().to_string_lossy()), DomainKind::File)
            .await
            .unwrap();
        let snapshot = store.file_stamps(domain).await.unwrap();
        let scan = scan_paths(
            "d",
            root.path(),
            snapshot,
            vec![
                "a.md".to_string(),
                "MANIFEST.md".to_string(),
                "assets/deck.pdf".to_string(),
            ],
            &params(),
        )
        .await;
        let report = apply_scan(&store, domain, scan).await.unwrap();
        assert_eq!(
            report.added + report.updated + report.deleted + report.moved,
            0
        );
    }
    assert_eq!(
        wal_frames(&db),
        0,
        "an unchanged targeted sync wrote to the index"
    );

    edit_grows_the_log(&store, root.path(), &db).await;
}

#[tokio::test]
async fn the_first_sync_of_an_empty_domain_still_records_it() {
    // A domain with nothing in it changes no row when it syncs, but its first
    // sync is still recorded, so it does not read as never synced.
    let root = tempfile::tempdir().unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    sync_domain_with(&store, "empty", root.path(), &params())
        .await
        .unwrap();
    let stats = store.domain_stats().await.unwrap();
    let row = stats.iter().find(|d| d.name == "empty").unwrap();
    assert!(row.last_sync.is_some(), "the first sync is recorded");
}

#[tokio::test]
async fn a_sync_that_changed_something_moves_the_sync_stamp() {
    let root = tempfile::tempdir().unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    seed_files(root.path());
    sync_domain_with(&store, "d", root.path(), &params())
        .await
        .unwrap();
    let stamp = |stats: &[crystalline_index::DomainStats]| {
        stats
            .iter()
            .find(|d| d.name == "d")
            .and_then(|d| d.last_sync.clone())
            .unwrap()
    };
    let first = stamp(&store.domain_stats().await.unwrap());

    std::thread::sleep(std::time::Duration::from_millis(1100));
    sync_domain_with(&store, "d", root.path(), &params())
        .await
        .unwrap();
    assert_eq!(
        stamp(&store.domain_stats().await.unwrap()),
        first,
        "a sync that changed nothing keeps the stamp it found"
    );

    write(root.path(), "b.md", &engram("Beta", "beta", "edited body"));
    sync_domain_with(&store, "d", root.path(), &params())
        .await
        .unwrap();
    assert!(
        stamp(&store.domain_stats().await.unwrap()) > first,
        "a sync that changed a file moves the stamp"
    );
}

/// Every row version in a Postgres schema and every sequence position: an
/// insert, an update, a delete or a row lock changes a row's
/// `(ctid, xmin, xmax)`, and an upsert that inserts nothing still moves the
/// id sequence. Read over a connection of its own, so it sees what was
/// committed and nothing is cached between reads.
#[cfg(feature = "postgres")]
async fn pg_fingerprint(url: &str, schema: &str) -> Vec<(String, String)> {
    use sqlx::Connection;
    let mut raw = sqlx::PgConnection::connect(url).await.unwrap();
    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema=$1 AND table_type='BASE TABLE' ORDER BY table_name",
    )
    .bind(schema)
    .fetch_all(&mut raw)
    .await
    .unwrap();
    assert!(!tables.is_empty(), "the schema has tables");
    let mut print = Vec::new();
    for (table,) in tables {
        let (rows,): (Option<String>,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT string_agg(ctid::text || '/' || xmin::text || '/' || xmax::text, ',' \
             ORDER BY ctid) FROM \"{schema}\".\"{table}\""
        )))
        .fetch_one(&mut raw)
        .await
        .unwrap();
        print.push((table, rows.unwrap_or_default()));
    }
    let sequences: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT sequencename::text, last_value FROM pg_sequences \
         WHERE schemaname=$1 ORDER BY sequencename",
    )
    .bind(schema)
    .fetch_all(&mut raw)
    .await
    .unwrap();
    for (sequence, last) in sequences {
        print.push((format!("sequence {sequence}"), format!("{last:?}")));
    }
    print
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn an_unchanged_sync_writes_nothing_on_postgres() {
    let Some(scratch) = crate::pg::scratch("idle") else {
        return;
    };
    let store = crystalline_index::PostgresStore::open_in_schema(scratch.url(), scratch.name())
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    seed_files(root.path());
    let report = sync_domain_with(&store, "d", root.path(), &params())
        .await
        .unwrap();
    assert_eq!(report.added, 4);
    let before = pg_fingerprint(scratch.url(), scratch.name()).await;

    // The full walk.
    for _ in 0..2 {
        let report = sync_domain_with(&store, "d", root.path(), &params())
            .await
            .unwrap();
        assert_eq!(report.unchanged, 4);
    }
    assert_eq!(
        pg_fingerprint(scratch.url(), scratch.name()).await,
        before,
        "an unchanged full sync wrote to the index"
    );

    // The targeted path.
    let domain = store
        .upsert_domain("d", Some(&root.path().to_string_lossy()), DomainKind::File)
        .await
        .unwrap();
    let snapshot = store.file_stamps(domain).await.unwrap();
    let scan = scan_paths(
        "d",
        root.path(),
        snapshot,
        vec![
            "a.md".to_string(),
            "MANIFEST.md".to_string(),
            "assets/deck.pdf".to_string(),
        ],
        &params(),
    )
    .await;
    apply_scan(&store, domain, scan).await.unwrap();
    assert_eq!(
        pg_fingerprint(scratch.url(), scratch.name()).await,
        before,
        "an unchanged targeted sync wrote to the index"
    );

    // The signal sees a real write.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(root.path(), "b.md", &engram("Beta", "beta", "edited body"));
    let report = sync_domain_with(&store, "d", root.path(), &params())
        .await
        .unwrap();
    assert_eq!(report.updated, 1);
    assert_ne!(
        pg_fingerprint(scratch.url(), scratch.name()).await,
        before,
        "a sync that changed a file wrote to the index"
    );
}
