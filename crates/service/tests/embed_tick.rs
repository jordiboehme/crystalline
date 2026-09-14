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

    // write_engram indexes and chunks synchronously and nudges the wired
    // channel once it has; the explicit request below is kept anyway, so this
    // test observes the checkpoint whether the pass it watches is the write's
    // or its own. The spawned worker consumes the signal, embeds via the
    // provider and, per the change under test, checkpoints the WAL once the
    // pass embeds a non-zero count.
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

// --- one pass at a time ------------------------------------------------------

/// An embedder that holds every batch until the test opens the gate and records
/// each text it was handed. Blocking the provider is what makes a pass
/// observably in flight, and the recorded texts are what prove no chunk was
/// embedded twice.
struct GatedEmbedder {
    open: std::sync::atomic::AtomicBool,
    seen: std::sync::Mutex<Vec<String>>,
}

impl GatedEmbedder {
    fn closed() -> Self {
        Self {
            open: std::sync::atomic::AtomicBool::new(false),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn open(&self) {
        self.open.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crystalline_index::EmbeddingProvider for GatedEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        while !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.seen.lock().unwrap().extend(texts.iter().cloned());
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

/// The live `embed` entries in a status report.
fn embed_activities(report: &serde_json::Value) -> usize {
    report["activity"]["now"]
        .as_array()
        .map(|now| now.iter().filter(|e| e["kind"] == "embed").count())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_embed_requests_during_a_pass_run_one_activity_and_then_none() {
    // The shape of the scale-run finding: the daemon's startup task ran a pass
    // inline while the worker ran another, so two passes walked one backlog
    // with independent cursors, each re-embedding what the other had in flight.
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let (embed_tx, embed_rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(virtual_engine(store).with_embed_channel(embed_tx));
    let embedder = Arc::new(GatedEmbedder::closed());
    engine.set_provider(embedder.clone());

    // Distinct bodies so a text identifies its chunk.
    for i in 0..6 {
        engine
            .write_engram(&write_params(
                &format!("Note {i:02}"),
                &format!("the body of note number {i:02}"),
            ))
            .await
            .unwrap();
    }
    let backlog = engine.embedding_backlog().await.unwrap();
    assert_eq!(backlog, 6, "one chunk per note is outstanding");

    // The worker, and beside it an inline caller: the two pass sources the
    // daemon had.
    tokio::spawn(crystalline_service::engine::run_embed_worker(
        engine.clone(),
        embed_rx,
    ));
    let inline = tokio::spawn({
        let e = engine.clone();
        async move { e.embed_pending().await }
    });
    assert!(engine.request_embed(), "the wired channel takes a request");
    assert!(engine.request_embed(), "and a second one");

    // A pass is in flight (the provider is holding its first batch).
    let mut live = 0;
    for _ in 0..400 {
        live = embed_activities(&engine.status_report().await.unwrap());
        if live > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(live > 0, "a pass opened an embed activity");
    // Hold there a while: a second pass would have opened its own record by now.
    for _ in 0..20 {
        assert_eq!(
            embed_activities(&engine.status_report().await.unwrap()),
            1,
            "exactly one embed activity runs, whatever asks"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Drain.
    embedder.open();
    let mut drained = false;
    for _ in 0..500 {
        if engine.embedding_backlog().await.unwrap() == 0
            && embed_activities(&engine.status_report().await.unwrap()) == 0
        {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(drained, "the backlog drains and the activity closes");
    let report = engine.status_report().await.unwrap();
    assert_eq!(
        report["activity"]["last"]["kind"], "embed",
        "the finished pass is the last recorded operation"
    );
    let _ = tokio::time::timeout(Duration::from_secs(5), inline).await;

    // Every chunk was handed to the provider exactly once.
    let mut seen = embedder.seen();
    let handed = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        backlog,
        "the provider saw every chunk in the backlog"
    );
    assert_eq!(handed, seen.len(), "and saw no chunk twice");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tick_stays_silent_while_a_pass_is_in_flight() {
    // "backlog non-empty" is true for the whole life of a long pass, so that
    // predicate alone fires the worker into a second one every cadence.
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let (embed_tx, mut embed_rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(virtual_engine(store).with_embed_channel(embed_tx));
    let embedder = Arc::new(GatedEmbedder::closed());
    engine.set_provider(embedder.clone());

    engine
        .write_engram(&write_params(
            "Note",
            "the body of a note that produces a chunk",
        ))
        .await
        .unwrap();
    while embed_rx.try_recv().is_ok() {}

    // No worker: the pass is this task, so nothing but the tick can signal.
    let pass = tokio::spawn({
        let e = engine.clone();
        async move { e.embed_pending().await }
    });
    for _ in 0..400 {
        if engine.embed_in_flight() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(engine.embed_in_flight(), "a pass is in flight");
    assert!(
        engine.embedding_backlog().await.unwrap() > 0,
        "and the backlog it is working on is non-empty"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(run_embed_tick(
        engine.clone(),
        Duration::from_millis(25),
        shutdown_rx,
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        embed_rx.try_recv().is_err(),
        "the tick never fires a second pass into a running one"
    );

    embedder.open();
    let _ = tokio::time::timeout(Duration::from_secs(5), pass).await;
    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_made_during_a_pass_is_served_by_a_follow_up_walk() {
    // The other half of one-pass-at-a-time: a caller that loses the gate must
    // not lose its work with it. No worker and no tick are wired here, so the
    // only thing that can embed the note written mid-pass is the running pass
    // walking the backlog once more.
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let engine = Arc::new(virtual_engine(store));
    let embedder = Arc::new(GatedEmbedder::closed());
    engine.set_provider(embedder.clone());

    for i in 0..6 {
        engine
            .write_engram(&write_params(
                &format!("Note {i:02}"),
                &format!("the body of note number {i:02}"),
            ))
            .await
            .unwrap();
    }
    let pass = tokio::spawn({
        let e = engine.clone();
        async move { e.embed_pending().await }
    });
    for _ in 0..400 {
        if engine.embed_in_flight() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(engine.embed_in_flight(), "a pass is in flight");

    // A write lands mid-pass, below or above the running walk's cursor, and
    // asks for an embed. The gate is held, so this caller is turned away.
    engine
        .write_engram(&write_params(
            "Note 06",
            "the body of note number 06, written while a pass was running",
        ))
        .await
        .unwrap();
    assert_eq!(
        engine.embed_pending().await.unwrap(),
        0,
        "a second caller is turned away rather than walking the backlog too"
    );

    embedder.open();
    let embedded = tokio::time::timeout(Duration::from_secs(10), pass)
        .await
        .expect("the pass finishes")
        .unwrap()
        .unwrap();
    assert_eq!(embedded, 7, "the pass embedded the note written during it");
    assert_eq!(
        engine.embedding_backlog().await.unwrap(),
        0,
        "nothing is left behind for a tick to find"
    );
    let mut seen = embedder.seen();
    let handed = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 7, "every chunk reached the provider");
    assert_eq!(handed, seen.len(), "and none of them twice");
}
