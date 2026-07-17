//! The daemon's embed self-heal tick. The embed worker is event-driven: a
//! transient provider failure consumes its signal and strands the backlog until
//! the next write. This periodic tick re-fires the worker while a backlog
//! remains and stays silent when there is none, and exits on shutdown.

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
