//! Corruption recovery: a garbaged database file is discarded and rebuilt from
//! the files on disk, and search results match the pre-corruption snapshot.

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
async fn corrupt_database_recovers_via_full_reindex() {
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

    // reindex --full recovery path: open resiliently (discarding the corrupt
    // file), then resync from the files on disk.
    let store = TursoStore::open_resilient(&db_path).await.unwrap();
    let _ = store.wipe().await; // harmless on a fresh db, part of the reindex path
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
