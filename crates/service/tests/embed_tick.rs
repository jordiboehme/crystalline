//! The daemon's embed self-heal tick. The embed worker is event-driven: a
//! transient provider failure consumes its signal and strands the backlog until
//! the next write. This periodic tick re-fires the worker while a backlog
//! remains and stays silent when there is none, and exits on shutdown. The
//! engine's own pass is covered here too: it pages the backlog and skips a
//! batch the provider rejects instead of stranding the rest.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::{Store, TursoStore};
use crystalline_service::daemon::run_embed_tick;
use crystalline_service::engine::Engine;
use crystalline_service::params::*;
use support::CountingEmbedder;
use tokio::sync::Mutex;

/// An engine over a config with a single virtual domain named `notes`.
fn virtual_engine(store: Arc<Mutex<dyn Store>>) -> Engine {
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    Engine::new(store, cfg, None, None)
}

fn write_params(title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
    }
}

#[tokio::test]
async fn tick_refires_the_worker_while_a_backlog_remains() {
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let (embed_tx, mut embed_rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(virtual_engine(store).with_embed_channel(embed_tx));

    // A written engram is chunked but, with no provider, never embedded, so the
    // backlog is non-empty. The write itself schedules one pass on the wired
    // channel; drain that so only a tick-driven signal is left to observe.
    engine
        .write_engram(&write_params(
            "Note",
            "the body of a note that produces a chunk",
        ))
        .await
        .unwrap();
    while embed_rx.try_recv().is_ok() {}
    assert!(
        engine.embedding_backlog().await.unwrap() > 0,
        "the write left an unembedded backlog"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(run_embed_tick(
        engine.clone(),
        Duration::from_millis(25),
        shutdown_rx,
    ));

    // The tick must re-fire the worker within the window.
    let signal = tokio::time::timeout(Duration::from_secs(1), embed_rx.recv()).await;
    assert!(
        signal.is_ok_and(|v| v.is_some()),
        "a tick re-fires the worker while a backlog remains"
    );

    // Shutdown mirrors the other periodic tasks: the task exits promptly.
    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("the tick task exits when shutdown is signaled")
        .unwrap();
}

#[tokio::test]
async fn tick_stays_silent_with_no_backlog() {
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let (embed_tx, mut embed_rx) = tokio::sync::mpsc::unbounded_channel();
    // Nothing is written, so nothing is chunked and the backlog is empty.
    let engine = Arc::new(virtual_engine(store).with_embed_channel(embed_tx));
    assert_eq!(
        engine.embedding_backlog().await.unwrap(),
        0,
        "an empty index has an empty backlog"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(run_embed_tick(
        engine.clone(),
        Duration::from_millis(25),
        shutdown_rx,
    ));

    // Several tick periods pass with an empty backlog; the worker is never fired.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        embed_rx.try_recv().is_err(),
        "an empty backlog fires no tick signal"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

// --- WAL hygiene -------------------------------------------------------------

/// The WAL sidecar path turso writes next to a local db file. Mirrors the CLI
/// test helpers in crates/cli/tests/data.rs.
fn wal_path(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_os_string();
    s.push("-wal");
    PathBuf::from(s)
}

/// True when the WAL sidecar is either absent or truncated to 0 bytes - the
/// two shapes `PRAGMA wal_checkpoint(TRUNCATE)` can leave behind.
fn wal_is_truncated(db: &Path) -> bool {
    match std::fs::metadata(wal_path(db)) {
        Ok(meta) => meta.len() == 0,
        Err(_) => true,
    }
}

#[tokio::test]
async fn checkpoint_wal_truncates_the_sidecar() {
    // A real file-backed db: the sidecar this test asserts on does not exist
    // for an in-memory store.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("index.db");
    let store = TursoStore::open(&db).await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let engine = virtual_engine(store);

    engine
        .write_engram(&write_params(
            "Note",
            "a body long enough to leave a non-empty WAL sidecar before the checkpoint",
        ))
        .await
        .unwrap();
    assert!(
        !wal_is_truncated(&db),
        "the write left the WAL non-empty: {:?}",
        std::fs::metadata(wal_path(&db)).map(|m| m.len())
    );

    engine.checkpoint_wal().await;
    assert!(
        wal_is_truncated(&db),
        "checkpoint_wal truncates the sidecar: {:?}",
        std::fs::metadata(wal_path(&db)).map(|m| m.len())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embed_worker_checkpoints_the_wal_after_a_pass() {
    // A real file-backed db, driven through the same run_embed_worker loop the
    // daemon spawns, so this exercises the post-embed-pass checkpoint call
    // site, not just Engine::checkpoint_wal in isolation.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("index.db");
    let store = TursoStore::open(&db).await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let (embed_tx, embed_rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(virtual_engine(store).with_embed_channel(embed_tx));
    let embedder = Arc::new(CountingEmbedder::new());
    engine.set_provider(embedder.clone());

    tokio::spawn(crystalline_service::engine::run_embed_worker(
        engine.clone(),
        embed_rx,
    ));

    // write_engram indexes and chunks synchronously but, like the real MCP
    // and watcher paths, does not itself request a background pass; that is
    // the self-heal tick's job (daemon::run_embed_tick) or, here, an explicit
    // request mirroring it. The spawned worker consumes the signal, embeds
    // via the provider and, per the change under test, checkpoints the WAL
    // once the pass embeds a non-zero count.
    engine
        .write_engram(&write_params(
            "Note",
            "the body of a note that produces a chunk for the worker to embed",
        ))
        .await
        .unwrap();
    assert!(
        engine.request_embed(),
        "the wired channel accepts the request"
    );

    for _ in 0..200 {
        if embedder.calls.load(std::sync::atomic::Ordering::SeqCst) > 0 && wal_is_truncated(&db) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "the embed worker never checkpointed the WAL after its pass: embed calls={}, wal={:?}",
        embedder.calls.load(std::sync::atomic::Ordering::SeqCst),
        std::fs::metadata(wal_path(&db)).map(|m| m.len())
    );
}

#[tokio::test]
async fn embed_pending_pages_the_backlog_and_embeds_each_chunk_once() {
    // Ten one-chunk engrams driven with a page size of three, so the pass spans
    // several full pages and ends on a short one. The backlog is walked by
    // keyset cursor, so a paged pass must embed every chunk exactly once.
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let engine = Arc::new(virtual_engine(store));
    let embedder = Arc::new(CountingEmbedder::new());
    engine.set_provider(embedder.clone());

    for i in 0..10 {
        engine
            .write_engram(&write_params(
                &format!("Note {i:02}"),
                &format!("the body of note number {i:02}"),
            ))
            .await
            .unwrap();
    }
    let backlog = engine.embedding_backlog().await.unwrap();
    assert_eq!(backlog, 10, "one chunk per note is outstanding");

    let embedded = engine.embed_pending_with_page(3).await.unwrap();
    assert_eq!(embedded, backlog, "every chunk embedded exactly once");
    assert_eq!(
        engine.embedding_backlog().await.unwrap(),
        0,
        "the paged pass drains the backlog"
    );
    // One provider call per page (a page is well under the batch size), so the
    // pass really did span four pages rather than one snapshot.
    assert_eq!(
        embedder.calls.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "ten chunks at a page size of three is four pages"
    );
    // A second pass has nothing left to do.
    assert_eq!(engine.embed_pending_with_page(3).await.unwrap(), 0);
}

/// An embedder that rejects any batch holding a text with the poison marker,
/// the shape of a chunk the real provider chokes on.
struct PoisonEmbedder;

#[async_trait::async_trait]
impl crystalline_index::EmbeddingProvider for PoisonEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        if texts.iter().any(|t| t.contains("POISON")) {
            return Err(crystalline_index::IndexError::Embedding(
                "this batch is poisoned".into(),
            ));
        }
        Ok(vec![vec![0.1_f32; 4]; texts.len()])
    }
    fn model_id(&self) -> &str {
        "test-model"
    }
    fn dims(&self) -> usize {
        4
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
}

#[tokio::test]
async fn a_rejected_batch_never_starves_the_backlog() {
    // A chunk the provider chokes on must not strand everything queued behind
    // it: the field symptom this guards is a backlog stuck at a handful of
    // chunks for days. A page size of one makes every batch a single chunk, so
    // exactly the poisoned chunk survives the pass unembedded.
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let engine = Arc::new(virtual_engine(store));
    engine.set_provider(Arc::new(PoisonEmbedder));

    for i in 0..6 {
        let body = if i == 3 {
            "a POISON body the provider rejects".to_string()
        } else {
            format!("the body of note number {i:02}")
        };
        engine
            .write_engram(&write_params(&format!("Note {i:02}"), &body))
            .await
            .unwrap();
    }

    let embedded = engine.embed_pending_with_page(1).await.unwrap();
    assert_eq!(embedded, 5, "every healthy chunk embedded");
    assert_eq!(
        engine.embedding_backlog().await.unwrap(),
        1,
        "the poisoned chunk stays in the backlog, visible for a later pass"
    );
}
