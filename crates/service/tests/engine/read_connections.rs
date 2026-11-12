//! The engine's reader: the store for reads that need no writer guard, and
//! the debug check that keeps it out of an open write transaction.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::{DomainKind, Store, TursoStore};
use crystalline_service::Engine;
use tokio::sync::Mutex;

async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("open".to_string(), DomainEntry::virtual_domain());
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let engine = Arc::new(Engine::new(store, cfg, None, Some(config_path)));
    (tmp, engine)
}

#[tokio::test]
async fn the_reader_of_a_store_without_read_connections_reads_through_the_writer() {
    let (_tmp, engine) = engine().await;
    engine
        .store()
        .lock()
        .await
        .upsert_domain("one", None, DomainKind::Virtual)
        .await
        .unwrap();
    assert_eq!(
        engine.reader().domain_names().await.unwrap(),
        vec!["one".to_string()]
    );
}

#[cfg(debug_assertions)]
#[tokio::test]
#[should_panic(expected = "a reader was used inside an open write transaction")]
async fn a_reader_asked_for_inside_an_open_write_transaction_panics_in_a_debug_build() {
    let (_tmp, engine) = engine().await;
    // On a task of its own, as every daemon request runs: the check knows
    // a transaction by the task that opened it.
    let outcome = tokio::spawn(async move {
        let store = engine.store();
        let writer = store.lock().await;
        writer.begin().await.unwrap();
        let _ = engine.reader();
    })
    .await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}
