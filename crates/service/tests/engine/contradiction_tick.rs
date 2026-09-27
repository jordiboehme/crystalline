//! The contradiction pass's two triggers: the embed worker asks for one after
//! every completed embed pass, and the embed tick asks while pending is
//! unknown or non-zero and nothing is embedding. Off is silent.

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, EvolveConfig, GlobalConfig};
use crystalline_index::{EmbeddingProvider, IndexError, Store, TursoStore};
use crystalline_service::daemon::run_embed_tick;
use crystalline_service::engine::{Engine, run_embed_worker};
use crystalline_service::params::WriteParams;
use tokio::sync::Mutex;

async fn engine_on(
    profile: &str,
    provider: Option<Arc<dyn EmbeddingProvider>>,
) -> (Engine, tokio::sync::mpsc::UnboundedReceiver<()>) {
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    cfg.evolve = Some(EvolveConfig {
        contradictions: Some(profile.to_string()),
    });
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (
        Engine::new(store, cfg, provider, None).with_contradiction_channel(tx),
        rx,
    )
}

async fn engine(profile: &str) -> (Engine, tokio::sync::mpsc::UnboundedReceiver<()>) {
    engine_on(profile, None).await
}

#[tokio::test]
async fn the_tick_asks_for_a_pass_while_pending_is_unknown() {
    let (engine, mut rx) = engine("full").await;
    let engine = Arc::new(engine);
    assert!(
        engine.contradictions_wanted(),
        "a fresh start knows nothing yet"
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(run_embed_tick(
        engine.clone(),
        Duration::from_millis(25),
        shutdown_rx,
    ));
    let signal = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
    assert!(
        signal.is_ok_and(|v| v.is_some()),
        "the tick asked for a contradiction pass"
    );
    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn the_tick_is_silent_when_the_check_is_off() {
    let (engine, mut rx) = engine("off").await;
    let engine = Arc::new(engine);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(run_embed_tick(
        engine.clone(),
        Duration::from_millis(25),
        shutdown_rx,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err(),
        "off asks for nothing"
    );
    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_completed_embed_pass_asks_for_a_contradiction_pass() {
    let (engine, mut rx) = engine("full").await;
    let (embed_tx, embed_rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(engine.with_embed_channel(embed_tx));
    tokio::spawn(run_embed_worker(engine.clone(), embed_rx));
    assert!(engine.request_embed());
    let signal = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
    assert!(
        signal.is_ok_and(|v| v.is_some()),
        "the embed worker hands over to the contradiction pass"
    );
}

/// Rejects every batch, as the work mac's provider rejects its 13 chunks on
/// every pass.
struct RejectingEmbedder;

#[async_trait::async_trait]
impl EmbeddingProvider for RejectingEmbedder {
    async fn embed(&self, _texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        Err(IndexError::Embedding("rejected".to_string()))
    }

    fn model_id(&self) -> &str {
        "rejecting-model"
    }

    fn dims(&self) -> usize {
        4
    }

    fn max_input_tokens(&self) -> usize {
        512
    }
}

/// L1: on an install whose backlog never drains, the tick's "nothing left to
/// embed" branch is never reached, so the handover after an embed pass is the
/// only way in. A pass whose batches were all rejected still completes and
/// still hands over.
#[tokio::test]
async fn an_embed_pass_with_a_rejected_batch_still_hands_over() {
    let (engine, mut rx) = engine_on("full", Some(Arc::new(RejectingEmbedder))).await;
    let (embed_tx, embed_rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(engine.with_embed_channel(embed_tx));
    engine
        .write_engram(&WriteParams {
            domain: "notes".to_string(),
            title: "Note".to_string(),
            content: "The retry queue.\n\n- [fact] The build uses Node 18".to_string(),
            folder: None,
            engram_type: None,
            tags: Vec::new(),
            status: None,
            metadata: None,
            overwrite: false,
            share_link: None,
            model: None,
        })
        .await
        .unwrap();
    tokio::spawn(run_embed_worker(engine.clone(), embed_rx));
    assert!(engine.request_embed());
    let signal = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
    assert!(
        signal.is_ok_and(|v| v.is_some()),
        "a pass that embedded nothing still hands over"
    );
    assert!(
        engine.embedding_backlog().await.unwrap() > 0,
        "the backlog never drains"
    );
}
