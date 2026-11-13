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

#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_postgres_reader_answers_beside_the_pinned_transaction_and_sees_only_commits() {
    let Some(scratch) = crate::pg::scratch("rc") else {
        return;
    };
    let store = crystalline_index::PostgresStore::open_in_schema_with_readers(
        scratch.url(),
        scratch.name(),
        2,
    )
    .await
    .unwrap();
    let reader = store
        .read_pool()
        .expect("a postgres store opens read connections");
    reader_beside_open_transaction(&store, reader).await;
}

/// A read whose caller gives up while it waits on a lock: the reader holds a
/// read transaction then, and a caller that is gone must not leave it open.
#[cfg(feature = "postgres")]
async fn cancel_a_postgres_read(
    readers: usize,
) -> (
    crystalline_index::ScratchSchema,
    crystalline_index::PostgresStore,
) {
    use sqlx::{AssertSqlSafe, Connection};
    let scratch = crate::pg::scratch("rv").expect("checked by the caller");
    let store = crystalline_index::PostgresStore::open_in_schema_with_readers(
        scratch.url(),
        scratch.name(),
        readers,
    )
    .await
    .unwrap();
    store
        .upsert_domain("a", None, DomainKind::Virtual)
        .await
        .unwrap();
    let reader = store.read_pool().unwrap();
    reader.domain_names().await.unwrap();
    let mut locker = sqlx::PgConnection::connect(scratch.url()).await.unwrap();
    sqlx::raw_sql(AssertSqlSafe(format!(
        "BEGIN; LOCK TABLE \"{}\".domain IN ACCESS EXCLUSIVE MODE",
        scratch.name()
    )))
    .execute(&mut locker)
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(500), reader.domain_names())
            .await
            .is_err(),
        "the read waits on the lock"
    );
    sqlx::raw_sql("COMMIT").execute(&mut locker).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    (scratch, store)
}

#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_postgres_read_leaves_no_transaction_behind() {
    if crate::pg::pg_url().is_none() {
        return;
    }
    let (_scratch, store) = cancel_a_postgres_read(2).await;
    store
        .upsert_domain("b", None, DomainKind::Virtual)
        .await
        .unwrap();
    let reader = store.read_pool().unwrap();
    for _ in 0..6 {
        assert_eq!(
            reader.domain_names().await.unwrap(),
            vec!["a".to_string(), "b".to_string()],
            "every read sees the write before it"
        );
    }
}

#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_postgres_read_does_not_stall_the_only_reader() {
    if crate::pg::pg_url().is_none() {
        return;
    }
    let (_scratch, store) = cancel_a_postgres_read(1).await;
    store
        .upsert_domain("b", None, DomainKind::Virtual)
        .await
        .unwrap();
    let reader = store.read_pool().unwrap();
    let names = tokio::time::timeout(Duration::from_secs(5), reader.domain_names())
        .await
        .expect("the next read does not wait for a leaked transaction")
        .unwrap();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
}
