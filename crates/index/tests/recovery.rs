//! Corruption recovery: a garbaged database file is set aside, rebuilt from the
//! files on disk, and search results match the pre-corruption snapshot. Nothing
//! on this path deletes: the unreadable bytes are renamed to a timestamped
//! sibling, because they may be the only copy of what a virtual domain holds.

use std::io::Write;
use std::path::Path;

use crystalline_index::{SearchQuery, Store, TursoStore, sync_domain};

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

#[tokio::test]
async fn corrupt_database_recovers_via_reindex_wipe() {
    let corpus = tempfile::tempdir().unwrap();
    let root = corpus.path();
    for i in 0..12 {
        write(
            root,
            &format!("e{i}.md"),
            &engram(
                &format!("Engram {i}"),
                &format!("e{i}"),
                &format!("payload keyword_{i} shared_corpus_term\n"),
            ),
        );
    }

    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("index.db");

    // Initial index and a snapshot of a search.
    {
        let store = TursoStore::open(&db_path).await.unwrap();
        sync_domain(&store, "d", root).await.unwrap();
        let page = store
            .search(&SearchQuery::text("shared_corpus_term"))
            .await
            .unwrap();
        assert_eq!(page.total, 12);
    }
    let snapshot = {
        let store = TursoStore::open(&db_path).await.unwrap();
        let page = store.search(&SearchQuery::text("keyword_7")).await.unwrap();
        page.items
            .iter()
            .map(|h| h.permalink.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(snapshot, vec!["e7".to_string()]);

    // Corrupt the database file: truncate and overwrite with garbage.
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&db_path)
            .unwrap();
        f.write_all(b"this is not a sqlite database, just garbage bytes \x00\x01\x02")
            .unwrap();
        f.flush().unwrap();
    }

    // The `reindex --wipe` recovery path: open resiliently (discarding the
    // corrupt file), wipe what a readable-but-wrong database would still hold,
    // then resync from the files on disk. This is the one verb that still
    // destroys the index, and the only case that needs it - `reindex --full`
    // re-reads every file without a wipe and could not open this file at all.
    let store = TursoStore::open_resilient(&db_path).await.unwrap();
    let _ = store.wipe().await; // harmless on a fresh db, part of the wipe path
    let report = sync_domain(&store, "d", root).await.unwrap();
    assert_eq!(report.added, 12, "rebuilt from disk");

    let page = store
        .search(&SearchQuery::text("shared_corpus_term"))
        .await
        .unwrap();
    assert_eq!(page.total, 12, "results restored");
    let after = store.search(&SearchQuery::text("keyword_7")).await.unwrap();
    let after_perms: Vec<String> = after.items.iter().map(|h| h.permalink.clone()).collect();
    assert_eq!(
        after_perms, snapshot,
        "results match the pre-corruption snapshot"
    );
}

/// The discard is a rename, never a delete. A database that will not open is
/// the last copy of anything a virtual domain holds, and it is also the one
/// state in which nothing can ask the database what it holds - so the bytes are
/// moved aside under a timestamped sibling name and a fresh database is opened
/// beside them, leaving a recovery attempt (or an external sqlite tool) a file
/// to work with rather than free space.
#[tokio::test]
async fn an_unreadable_database_is_set_aside_rather_than_deleted() {
    let corpus = tempfile::tempdir().unwrap();
    let root = corpus.path();
    write(
        root,
        "only.md",
        &engram("Only Copy", "only-copy", "payload_that_must_survive\n"),
    );

    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("index.db");
    {
        let store = TursoStore::open(&db_path).await.unwrap();
        sync_domain(&store, "d", root).await.unwrap();
        // Fold the WAL into the database file so the payload really is in the
        // bytes this test follows.
        store.checkpoint_wal().await.unwrap();
    }
    let before = std::fs::read(&db_path).unwrap();
    assert!(
        contains(&before, b"payload_that_must_survive"),
        "the payload is in the database file to begin with"
    );

    // Two bytes in the header's page-size field: every payload page is intact,
    // so what would destroy the recoverable bytes is the discard and not the
    // corruption.
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&db_path)
            .unwrap();
        f.seek(SeekFrom::Start(16)).unwrap();
        f.write_all(b"\x0d\x0d").unwrap();
        f.flush().unwrap();
    }
    let opens = match TursoStore::open(&db_path).await {
        Ok(store) => store.store_info().await.is_ok(),
        Err(_) => false,
    };
    assert!(!opens, "the corrupted file really will not open");

    let store = TursoStore::open_resilient(&db_path).await.unwrap();
    let aside = store
        .set_aside_database()
        .expect("the store names the database it set aside");
    assert!(
        aside.exists(),
        "the unreadable database is still on disk at {}",
        aside.display()
    );
    assert!(
        aside
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("index.db.unreadable-"),
        "it is set aside under a timestamped sibling name: {}",
        aside.display()
    );
    let kept = std::fs::read(&aside).unwrap();
    assert!(
        contains(&kept, b"payload_that_must_survive"),
        "the payload bytes survive the discard"
    );
    // And the fresh database in its place works.
    assert_eq!(
        sync_domain(&store, "d", root).await.unwrap().added,
        1,
        "the replacement database rebuilds from the file on disk"
    );
}

/// A needle search over bytes, so the payload can be followed through a file
/// that no longer parses as a database.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
