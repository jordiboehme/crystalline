//! The engine's read connections: which verbs read beside the writer, what
//! they see while the writer holds a transaction open, and the debug check
//! that keeps a reader out of one.
//!
//! The stores here are opened with read connections explicitly
//! (`TursoStore::open_with_readers`), so the suite says the same thing with
//! `CRYSTALLINE_READ_CONNECTIONS=0` set.

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::{DomainKind, Store, TursoStore};
use crystalline_service::params::{ReadParams, SearchParams, WriteParams};
use crystalline_service::{Engine, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

/// How long a read may take beside a held writer before the test calls it
/// waiting. Generous: a waiting read waits for good.
const ANSWER: Duration = Duration::from_secs(10);

/// An engine over a file store with `readers` read connections and one
/// virtual domain, `open`.
async fn engine(readers: usize) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let store = TursoStore::open_with_readers(&tmp.path().join("index.db"), readers)
        .await
        .unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("open".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let engine = Arc::new(Engine::new(store, cfg, None, Some(config_path)));
    (tmp, engine)
}

fn write(title: &str, content: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: "open".to_string(),
        title: title.to_string(),
        content: Some(content.to_string()),
        folder: None,
        engram_type: None,
        tags: vec!["t".to_string()],
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
        expected_checksum: None,
    }
}

async fn search(engine: &Engine, query: &str) -> Vec<String> {
    let out = engine
        .search_engrams(
            &SearchParams {
                query: Some(query.to_string()),
                search_type: Some("text".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    out["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["permalink"].as_str().unwrap().to_string())
        .collect()
}

async fn read(engine: &Engine, identifier: &str) -> Value {
    engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some("open".to_string()),
                ..ReadParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// Run `f` on another task while this one holds the writer with a write
/// transaction open and an uncommitted domain row in it, then roll it back.
/// The answer must come while the writer is held.
async fn beside_held_writer<T, F>(engine: &Arc<Engine>, f: F) -> T
where
    T: Send + 'static,
    F: std::future::Future<Output = T> + Send + 'static,
{
    let store = engine.store();
    let writer = store.lock().await;
    writer.begin().await.unwrap();
    writer
        .upsert_domain("uncommitted", None, DomainKind::Virtual)
        .await
        .unwrap();
    let answer = tokio::time::timeout(ANSWER, tokio::spawn(f)).await;
    writer.rollback().await.unwrap();
    drop(writer);
    answer
        .expect("the read answered while the writer was held")
        .unwrap()
}

#[tokio::test]
async fn the_reader_of_a_store_without_read_connections_reads_through_the_writer() {
    let (_tmp, engine) = engine(0).await;
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
    let (_tmp, engine) = engine(0).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_answers_while_the_writer_holds_an_open_transaction() {
    let (_tmp, engine) = engine(4).await;
    engine
        .write_engram(&write(
            "Docking clamps",
            "The clamps misread below eight degrees.",
        ))
        .await
        .unwrap();
    let hits = {
        let engine = engine.clone();
        beside_held_writer(
            &engine.clone(),
            async move { search(&engine, "clamps").await },
        )
        .await
    };
    assert_eq!(hits, vec!["docking-clamps".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_then_a_search_in_one_caller_finds_the_new_engram() {
    let (_tmp, engine) = engine(4).await;
    for i in 0..10 {
        let title = format!("Note {i} quokka");
        engine
            .write_engram(&write(&title, &format!("body {i} quokka")))
            .await
            .unwrap();
        let hits = search(&engine, "quokka").await;
        assert_eq!(hits.len(), i + 1, "the write before is found: {hits:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_answers_while_the_writer_holds_an_open_transaction() {
    let (_tmp, engine) = engine(4).await;
    engine
        .write_engram(&write(
            "Docking clamps",
            "The clamps misread below eight degrees.",
        ))
        .await
        .unwrap();
    let out = {
        let engine = engine.clone();
        beside_held_writer(&engine.clone(), async move {
            read(&engine, "docking-clamps").await
        })
        .await
    };
    assert_eq!(out["title"], "Docking clamps");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_context_answers_while_the_writer_holds_an_open_transaction() {
    let (_tmp, engine) = engine(4).await;
    engine
        .write_engram(&write("Docking clamps", "Clamp notes. See [[Thrust cut]]."))
        .await
        .unwrap();
    engine
        .write_engram(&write("Thrust cut", "Cut thrust on the green tone."))
        .await
        .unwrap();
    let out = {
        let engine = engine.clone();
        beside_held_writer(&engine.clone(), async move {
            engine
                .build_context(
                    &crystalline_service::params::ContextParams {
                        anchor: "crystalline://open/docking-clamps".to_string(),
                        depth: None,
                        domains: Vec::new(),
                        timeframe: None,
                        max_related: None,
                    },
                    &Scope::Unrestricted,
                )
                .await
                .unwrap()
        })
        .await
    };
    let text = out.to_string();
    assert!(text.contains("thrust-cut"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbound_references_after_a_move_are_current() {
    let (_tmp, engine) = engine(4).await;
    engine
        .write_engram(&write("Thrust cut", "Cut thrust on the green tone."))
        .await
        .unwrap();
    engine
        .write_engram(&write("Docking clamps", "Clamp notes. See [[thrust-cut]]."))
        .await
        .unwrap();
    engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "docking-clamps".to_string(),
                domain: "open".to_string(),
                destination: "ops/docking-clamps".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let out = read(&engine, "thrust-cut").await;
    let text = out.to_string();
    assert!(
        text.contains("ops/docking-clamps"),
        "the reader sees the moved source at once: {text}"
    );
}
