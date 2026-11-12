//! Read connections beside the writer: what a reader sees while the writer
//! holds a transaction open, and that it never waits for it.

use std::sync::Arc;
use std::time::Duration;

use crystalline_index::{DomainKind, ReadStore, Store, TursoStore};

/// How long a reader may take beside an open write transaction before the
/// test calls it waiting.
const ANSWER: Duration = Duration::from_secs(10);

/// `reader.domain_id(name)` on a task of its own: a write transaction is
/// open on this task, and the debug build refuses a reader here.
async fn domain_id_elsewhere(reader: &Arc<dyn ReadStore>, name: &str) -> bool {
    let reader = reader.clone();
    let name = name.to_string();
    tokio::time::timeout(
        ANSWER,
        tokio::spawn(async move { reader.domain_id(&name).await.unwrap().is_some() }),
    )
    .await
    .expect("the reader answered beside the open write transaction")
    .unwrap()
}

async fn reader_beside_open_transaction(writer: &dyn Store, reader: Arc<dyn ReadStore>) {
    writer
        .upsert_domain("before", None, DomainKind::Virtual)
        .await
        .unwrap();
    writer.begin().await.unwrap();
    writer
        .upsert_domain("inside", None, DomainKind::Virtual)
        .await
        .unwrap();
    // The writer sees its own row; a reader answers at once, from the state
    // before the transaction.
    assert!(writer.domain_id("inside").await.unwrap().is_some());
    assert!(domain_id_elsewhere(&reader, "before").await);
    assert!(!domain_id_elsewhere(&reader, "inside").await);
    writer.commit().await.unwrap();
    // Written and committed, then read: the reader sees it.
    assert!(reader.domain_id("inside").await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turso_reader_answers_beside_an_open_write_transaction_and_sees_only_commits() {
    let dir = tempfile::tempdir().unwrap();
    let store = TursoStore::open_with_readers(&dir.path().join("index.db"), 2)
        .await
        .unwrap();
    let reader = store
        .read_pool()
        .expect("a file store opens read connections");
    reader_beside_open_transaction(&store, reader).await;
}

#[tokio::test]
async fn a_turso_reader_works_on_a_single_threaded_runtime_too() {
    let dir = tempfile::tempdir().unwrap();
    let store = TursoStore::open_with_readers(&dir.path().join("index.db"), 2)
        .await
        .unwrap();
    let reader = store.read_pool().unwrap();
    store
        .upsert_domain("one", None, DomainKind::Virtual)
        .await
        .unwrap();
    assert_eq!(
        reader.domain_names().await.unwrap(),
        vec!["one".to_string()]
    );
}

#[tokio::test]
async fn a_store_with_no_readers_hands_out_no_pool() {
    let dir = tempfile::tempdir().unwrap();
    let store = TursoStore::open_with_readers(&dir.path().join("index.db"), 0)
        .await
        .unwrap();
    assert!(store.read_pool().is_none());
    let memory = TursoStore::open_in_memory().await.unwrap();
    assert!(memory.read_pool().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turso_readers_and_a_write_loop_run_side_by_side() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        TursoStore::open_with_readers(&dir.path().join("index.db"), 4)
            .await
            .unwrap(),
    );
    let reader = store.read_pool().unwrap();
    let writer = {
        let store = store.clone();
        tokio::spawn(async move {
            for i in 0..200 {
                store.begin().await.unwrap();
                store
                    .upsert_domain(&format!("d{i}"), None, DomainKind::Virtual)
                    .await
                    .unwrap();
                store.commit().await.unwrap();
                store.checkpoint_wal().await.unwrap();
            }
        })
    };
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let reader = reader.clone();
            tokio::spawn(async move {
                let mut last = 0;
                for _ in 0..200 {
                    let n = reader.domain_names().await.unwrap().len();
                    assert!(n >= last, "a later read never sees fewer committed rows");
                    last = n;
                }
            })
        })
        .collect();
    writer.await.unwrap();
    for r in readers {
        r.await.unwrap();
    }
    assert_eq!(reader.domain_names().await.unwrap().len(), 200);
}
