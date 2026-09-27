//! The contradiction check end to end on the engine: the profile setting,
//! the model cache keep list, the daemon pass with the stub scorer, pending,
//! the sweep's V302 read and the pair-of-lines acknowledgment.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::nli::{
    ContradictionScorer, NLI_MODELS, NliModel, NliProfile, StubScorer, nli_model,
};
use crystalline_index::{ContradictionRow, EmbeddingProvider, IndexError, TursoStore};
use crystalline_service::Engine;
use crystalline_service::engine::{
    ConfigureAction, ContradictionOutcome, NLI_IDLE_DROP, ScorerLoader,
};
use crystalline_service::params::{EditParams, WriteParams};
use tokio::sync::Mutex;

/// One virtual domain `notes` on an engine with the topic provider, so two
/// engrams about the retry queue share a lead vector and a docking note is
/// orthogonal to both.
async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
    engine_on(Arc::new(crate::support::TopicEmbedder)).await
}

/// [`engine`] with another embedding provider.
async fn engine_on(provider: Arc<dyn EmbeddingProvider>) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        Some(provider),
        Some(config_path),
    );
    (tmp, Arc::new(engine))
}

async fn set(engine: &Engine, key: &str, value: &str) {
    engine
        .configure(&ConfigureAction::Set {
            key: key.to_string(),
            value: value.to_string(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn the_profile_names_one_model_and_off_names_none() {
    let (_tmp, engine) = engine().await;
    assert!(engine.contradiction_model().is_none(), "off by default");
    set(&engine, "evolve.contradictions", "light").await;
    assert_eq!(
        engine.contradiction_model().map(|m| m.repo),
        Some(nli_model(NliProfile::Light).repo)
    );
    set(&engine, "evolve.contradictions", "full").await;
    assert_eq!(
        engine.contradiction_model().map(|m| m.profile),
        Some(NliProfile::Full)
    );
    set(&engine, "evolve.contradictions", "off").await;
    assert!(engine.contradiction_model().is_none());
}

/// A hub-shaped cache directory with one weight file.
fn hub_dir(root: &std::path::Path, repo: &str) {
    let dir = root
        .join(crystalline_index::hub_dir_name(repo))
        .join("snapshots/abc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.safetensors"), b"weights").unwrap();
}

#[tokio::test]
async fn the_model_cache_keeps_the_configured_profile_and_drops_the_others() {
    let (_tmp, engine) = engine().await;
    let cache = tempfile::tempdir().unwrap();
    let granite = "ibm-granite/granite-embedding-97m-multilingual-r2";
    hub_dir(cache.path(), granite);
    for m in &NLI_MODELS {
        hub_dir(cache.path(), m.repo);
    }
    set(&engine, "evolve.contradictions", "light").await;
    engine.prune_model_cache(cache.path().to_path_buf()).await;
    let left: Vec<String> = crystalline_index::cached_model_dirs(cache.path())
        .into_iter()
        .map(|(repo, _)| repo)
        .collect();
    assert_eq!(
        left,
        vec![
            nli_model(NliProfile::Light).repo.to_string(),
            granite.to_string()
        ],
        "the embedding model and the configured profile stay (sorted by repo id)"
    );

    // Off keeps no NLI checkpoint at all.
    set(&engine, "evolve.contradictions", "off").await;
    engine.prune_model_cache(cache.path().to_path_buf()).await;
    let left: Vec<String> = crystalline_index::cached_model_dirs(cache.path())
        .into_iter()
        .map(|(repo, _)| repo)
        .collect();
    assert_eq!(left, vec![granite.to_string()]);
}

// --- the daemon pass -------------------------------------------------------

fn write(title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: vec!["t".to_string()],
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
    }
}

const NODE_18: &str = "The retry queue build runs on a pinned runtime.\n\n- [fact] The build uses Node 18\n- [fact] Retries back off on the queue";
const NODE_20: &str = "The retry queue build runs on a pinned runtime.\n\n- [fact] The build uses Node 20\n- [fact] Retries back off on the queue";
const NODE_19: &str = "The retry queue build runs on a pinned runtime.\n\n- [fact] The build breaks on Node 19\n- [fact] Retries back off on the queue";
const FRIDAYS: &str = "Docking clamps seat in bay three under thrust.\n\n- [fact] Deployments run on Fridays\n- [fact] The clamps misread below eight degrees";

fn full() -> &'static NliModel {
    nli_model(NliProfile::Full)
}

type Loaded = crystalline_index::Result<Arc<dyn ContradictionScorer>>;

/// A loader's answer, boxed the way [`ScorerLoader`] wants it.
fn ready(result: Loaded) -> futures::future::BoxFuture<'static, Loaded> {
    Box::pin(async move { result })
}

fn stub() -> Arc<StubScorer> {
    Arc::new(StubScorer::new(full().repo, 0.05).with(
        "The build uses Node 18",
        "The build uses Node 20",
        0.93,
    ))
}

/// A loader that hands out `scorer` and counts the loads.
fn loader_of(scorer: Arc<dyn ContradictionScorer>, loads: Arc<AtomicUsize>) -> ScorerLoader {
    Arc::new(move |_model: &'static NliModel| {
        loads.fetch_add(1, Ordering::SeqCst);
        ready(Ok(scorer.clone()))
    })
}

/// A loader that hands out `stub` and counts the loads.
fn loader(stub: Arc<StubScorer>, loads: Arc<AtomicUsize>) -> ScorerLoader {
    loader_of(stub, loads)
}

fn with_loader(engine: Arc<Engine>, loader: ScorerLoader) -> Arc<Engine> {
    Arc::new(
        Arc::try_unwrap(engine)
            .ok()
            .expect("one owner")
            .with_scorer_loader(loader),
    )
}

async fn engine_with(loader: ScorerLoader) -> (tempfile::TempDir, Arc<Engine>) {
    let (tmp, engine) = engine().await;
    (tmp, with_loader(engine, loader))
}

async fn rows(engine: &Engine, repo: &str) -> Vec<ContradictionRow> {
    let store = engine.store();
    let store = store.lock().await;
    let domain = store.list_engrams("notes", None, None).await.unwrap()[0].domain_id;
    store.contradictions(domain, repo, 0.0).await.unwrap()
}

async fn three(engine: &Engine) {
    engine
        .write_engram(&write("Eighteen", NODE_18))
        .await
        .unwrap();
    engine
        .write_engram(&write("Twenty", NODE_20))
        .await
        .unwrap();
    engine
        .write_engram(&write("Fridays", FRIDAYS))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
}

fn scored(pairs: usize, line_pairs: usize, remaining: usize) -> ContradictionOutcome {
    ContradictionOutcome::Scored {
        pairs,
        line_pairs,
        remaining,
    }
}

fn append_to_twenty() -> EditParams {
    EditParams {
        identifier: "twenty".to_string(),
        domain: "notes".to_string(),
        operation: "append".to_string(),
        content: Some("\n- [fact] The queue drains hourly".to_string()),
        ..EditParams::default()
    }
}

#[tokio::test]
async fn the_pass_is_off_until_a_profile_is_set_then_scores_related_pairs() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s.clone(), loads.clone())).await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    assert_eq!(loads.load(Ordering::SeqCst), 0, "off never loads a model");

    set(&engine, "evolve.contradictions", "full").await;
    // Eighteen and Twenty share the retry axis; Fridays is the docking axis.
    // Two lines against two lines, each scored in both orders.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0)
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1, "loaded lazily, once");
    assert_eq!(s.forwards(), 8);
    let stored = rows(&engine, full().repo).await;
    assert_eq!(
        stored.len(),
        1,
        "only the 0.93 pair clears the floor: {stored:?}"
    );
    assert!((stored[0].score_ab - 0.93).abs() < 1e-6);

    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(s.forwards(), 8, "an up-to-date pair is not asked again");
    assert!(
        !engine.contradictions_wanted(),
        "nothing pending, nothing to ask for"
    );
}

#[tokio::test]
async fn an_edit_requeues_the_pair() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    engine.edit_engram(&append_to_twenty()).await.unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 6, 0),
        "two lines against three"
    );
}

/// Review focus 4. Two models never sit in memory together: the old one is
/// dropped before the new one loads, which a failed first load makes visible.
/// A failed load is not retried by the next pass, only once the setting is
/// set again. Rows land under the model that scored them, and the old model's
/// rows stay.
#[tokio::test]
async fn a_profile_switch_drops_the_old_scorer_before_loading_the_new_one() {
    let light = nli_model(NliProfile::Light);
    let light_stub = Arc::new(StubScorer::new(light.repo, 0.7));
    let attempts = Arc::new(AtomicUsize::new(0));
    let loader: ScorerLoader = {
        let (full_stub, light_stub, attempts) = (stub(), light_stub.clone(), attempts.clone());
        Arc::new(move |model: &'static NliModel| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            let s: Arc<dyn ContradictionScorer> = if model.profile == NliProfile::Full {
                full_stub.clone()
            } else {
                light_stub.clone()
            };
            if model.profile == NliProfile::Light && n == 1 {
                ready(Err(IndexError::Embedding(
                    "the first light download failed".to_string(),
                )))
            } else {
                ready(Ok(s))
            }
        })
    };
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0)
    );
    assert!(engine.contradiction_scorer_loaded());

    set(&engine, "evolve.contradictions", "light").await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(
        !engine.contradiction_scorer_loaded(),
        "the full model went before light was asked for"
    );
    assert!(
        engine
            .contradiction_last_error()
            .is_some_and(|e| e.contains("the first light download failed")),
        "{:?}",
        engine.contradiction_last_error()
    );
    assert!(
        !engine.contradictions_wanted(),
        "a failed load is not retried by the tick"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "nor by the next pass: the loader wipes and downloads again on a failure"
    );

    // Setting the profile again is what asks for another load.
    set(&engine, "evolve.contradictions", "light").await;
    assert!(engine.contradiction_last_error().is_none());
    assert!(engine.contradictions_wanted());
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0)
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(
        rows(&engine, light.repo).await.len(),
        4,
        "every line pair at 0.7, under light"
    );
    assert_eq!(
        rows(&engine, full().repo).await.len(),
        1,
        "full's rows stay where they were"
    );
}

/// Review focus 1: a copied bullet is scored once and the pass drains.
#[tokio::test]
async fn a_repeated_observation_line_is_scored_once_and_the_pass_drains() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s.clone(), loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write(
            "Eighteen",
            "The retry queue build runs on a pinned runtime.\n\n- [fact] The build uses Node 18\n- [fact] The build uses Node 18",
        ))
        .await
        .unwrap();
    engine
        .write_engram(&write("Twenty", NODE_20))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 2, 0),
        "one of the two copies against two lines"
    );
    assert_eq!(
        s.forwards(),
        4,
        "two line pairs, both orders, nothing twice"
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(s.forwards(), 4);
    assert!(!engine.contradictions_wanted());
}

/// Review focus 2: an engram with no observations never enters the pending
/// set, so a drained scope stops asking.
#[tokio::test]
async fn a_drained_scope_asks_for_nothing_more() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s.clone(), loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine
        .write_engram(&write(
            "Retry prose",
            "The retry queue has a history.\nThe queue retries.\nBackoff grows on the queue.",
        ))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0)
    );
    assert!(!engine.contradictions_wanted());
    let before = s.forwards();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(s.forwards(), before);
    assert!(!engine.contradictions_wanted());
}

#[tokio::test]
async fn a_pass_is_bounded_and_the_next_one_finishes() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    // Nine engrams on one axis with eight lines each: 36 pairs of 64 line
    // pairs is 2304, over the 2000 a pass may score.
    for i in 0..9 {
        let bullets: String = (0..8)
            .map(|j| format!("\n- [fact] Retry fact {i} {j}"))
            .collect();
        engine
            .write_engram(&write(
                &format!("Retry note {i}"),
                &format!("The retry queue note {i}.\n{bullets}"),
            ))
            .await
            .unwrap();
    }
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(31, 1984, 5)
    );
    assert!(engine.contradictions_wanted(), "pairs remain");
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(5, 320, 0)
    );
}

#[tokio::test]
async fn the_pass_is_single_flight() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let loader: ScorerLoader = {
        let (gate, s) = (gate.clone(), stub());
        Arc::new(move |_model: &'static NliModel| {
            let (gate, s) = (gate.clone(), s.clone() as Arc<dyn ContradictionScorer>);
            let held: futures::future::BoxFuture<'static, Loaded> = Box::pin(async move {
                gate.notified().await;
                Ok(s)
            });
            held
        })
    };
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let first = tokio::spawn({
        let engine = engine.clone();
        async move { engine.score_contradictions().await.unwrap() }
    });
    while !engine.contradictions_in_flight() {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::AlreadyRunning
    );
    gate.notify_one();
    // The running pass walks once more for the turned-away request, and that
    // walk finds nothing left.
    assert_eq!(first.await.unwrap(), scored(1, 4, 0));
}

/// The T4 ruling: the loader wipes and downloads about 500 MB again on any
/// build error, so a failed load waits for the setting to be set again (or a
/// daemon start) and `last_error` says why until then.
#[tokio::test]
async fn a_failed_load_waits_for_a_setting_change_and_never_loops() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let loader: ScorerLoader = {
        let attempts = attempts.clone();
        Arc::new(move |_model: &'static NliModel| {
            attempts.fetch_add(1, Ordering::SeqCst);
            ready(Err(IndexError::Embedding("offline".to_string())))
        })
    };
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(
        engine
            .contradiction_last_error()
            .is_some_and(|e| e.contains("offline"))
    );
    assert!(
        !engine.contradictions_wanted(),
        "the tick does not retry it"
    );
    for _ in 0..3 {
        assert_eq!(
            engine.score_contradictions().await.unwrap(),
            ContradictionOutcome::ModelUnavailable
        );
    }
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "a failed load does not ask again by itself"
    );
    assert!(
        engine.contradiction_last_error().is_some(),
        "the reason stays"
    );

    set(&engine, "evolve.contradictions", "full").await;
    assert!(engine.contradictions_wanted(), "a set setting asks again");
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    // Off forgets the failure; on again loads again.
    set(&engine, "evolve.contradictions", "off").await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    assert!(engine.contradiction_last_error().is_none());
    set(&engine, "evolve.contradictions", "full").await;
    engine.score_contradictions().await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn the_scorer_drops_after_ten_idle_minutes_and_reloads_on_demand() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads.clone())).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    let now = tokio::time::Instant::now();
    engine.drop_idle_scorer_at(now + NLI_IDLE_DROP - std::time::Duration::from_secs(5));
    assert!(engine.contradiction_scorer_loaded(), "not idle long enough");
    engine.drop_idle_scorer_at(now + NLI_IDLE_DROP + std::time::Duration::from_secs(1));
    assert!(
        !engine.contradiction_scorer_loaded(),
        "dropped after ten idle minutes"
    );
    engine.edit_engram(&append_to_twenty()).await.unwrap();
    engine.embed_pending().await.unwrap();
    engine.score_contradictions().await.unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 2, "reloaded for the new work");

    // Turning the check off drops the model at once.
    set(&engine, "evolve.contradictions", "off").await;
    engine.drop_idle_scorer();
    assert!(!engine.contradiction_scorer_loaded());
}

/// PF9: a domain whose stamps and coverage are unchanged since a walk that
/// left it at zero is not parsed again; a changed stamp walks it.
#[tokio::test]
async fn a_settled_domain_is_skipped_until_a_stamp_changes() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let start = engine.contradiction_fact_walks();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0)
    );
    assert_eq!(engine.contradiction_fact_walks(), start + 1);
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(
        engine.contradiction_fact_walks(),
        start + 1,
        "nothing changed, so the domain is not parsed again"
    );
    engine.edit_engram(&append_to_twenty()).await.unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 6, 0)
    );
    assert_eq!(
        engine.contradiction_fact_walks(),
        start + 2,
        "a changed stamp walks the domain"
    );
}

/// Embeds like the topic provider, and rejects any batch that carries the
/// word "poison", every time.
struct PoisonTopicEmbedder;

#[async_trait::async_trait]
impl EmbeddingProvider for PoisonTopicEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        if texts.iter().any(|t| t.to_lowercase().contains("poison")) {
            return Err(IndexError::Embedding("this chunk never embeds".to_string()));
        }
        Ok(texts
            .iter()
            .map(|t| crate::support::TopicEmbedder::embed_one(t))
            .collect())
    }

    fn model_id(&self) -> &str {
        "topic-model"
    }

    fn dims(&self) -> usize {
        4
    }

    fn max_input_tokens(&self) -> usize {
        512
    }
}

/// L1: coverage below 100 percent never holds the pass. The work mac skips
/// 13 chunks on every embed pass; an engram with a lead vector is a
/// candidate whatever the rest of the domain's chunks do, and the domain
/// still settles.
#[tokio::test]
async fn a_chunk_the_provider_always_rejects_does_not_hold_the_pass() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_on(Arc::new(PoisonTopicEmbedder)).await;
    let engine = with_loader(engine, loader(s, loads));
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Eighteen", NODE_18))
        .await
        .unwrap();
    engine
        .write_engram(&write("Twenty", NODE_20))
        .await
        .unwrap();
    engine
        .write_engram(&write("Fridays", FRIDAYS))
        .await
        .unwrap();
    engine
        .write_engram(&write(
            "Poisoned",
            "The retry queue poison note.\n\n- [fact] The build uses Node 22",
        ))
        .await
        .unwrap();
    // One chunk per batch, so the poisoned chunk fails alone.
    engine.embed_pending_with_page(1).await.unwrap();
    engine.embed_pending_with_page(1).await.unwrap();
    assert!(
        engine.embedding_backlog().await.unwrap() > 0,
        "the poisoned chunk stays unembedded"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0)
    );
    let walks = engine.contradiction_fact_walks();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks,
        "a domain that never reaches full coverage still settles"
    );
    assert!(!engine.contradictions_wanted());
}

/// Embeds like the topic provider, and rejects every batch while `closed`.
struct GatedTopicEmbedder {
    closed: AtomicBool,
}

#[async_trait::async_trait]
impl EmbeddingProvider for GatedTopicEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(IndexError::Embedding("not yet".to_string()));
        }
        Ok(texts
            .iter()
            .map(|t| crate::support::TopicEmbedder::embed_one(t))
            .collect())
    }

    fn model_id(&self) -> &str {
        "topic-model"
    }

    fn dims(&self) -> usize {
        4
    }

    fn max_input_tokens(&self) -> usize {
        512
    }
}

/// The other half of PF9: a domain that settled while a possible candidate
/// still lacked its lead vector is walked again once the embedded count
/// moves, though no stamp changed.
#[tokio::test]
async fn a_lead_vector_that_arrives_later_walks_a_settled_domain_again() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let gated = Arc::new(GatedTopicEmbedder {
        closed: AtomicBool::new(false),
    });
    let (_tmp, engine) = engine_on(gated.clone()).await;
    let engine = with_loader(engine, loader(s, loads));
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Eighteen", NODE_18))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    gated.closed.store(true, Ordering::SeqCst);
    engine
        .write_engram(&write("Twenty", NODE_20))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0),
        "Twenty has no lead vector yet"
    );
    let walks = engine.contradiction_fact_walks();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(engine.contradiction_fact_walks(), walks, "settled");

    gated.closed.store(false, Ordering::SeqCst);
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0),
        "the vector arrived without a stamp change and the pair is scored"
    );
    assert_eq!(engine.contradiction_fact_walks(), walks + 1);
}

/// Fails every batch that names `marker`, and answers the rest like `inner`.
struct FailsOn {
    inner: Arc<StubScorer>,
    marker: &'static str,
}

impl ContradictionScorer for FailsOn {
    fn score(&self, pairs: &[(String, String)]) -> crystalline_index::Result<Vec<f32>> {
        if pairs
            .iter()
            .any(|(p, h)| p.contains(self.marker) || h.contains(self.marker))
        {
            return Err(IndexError::Embedding("the batch failed".to_string()));
        }
        self.inner.score(pairs)
    }

    fn model_repo(&self) -> &str {
        self.inner.model_repo()
    }
}

/// L3: one failing batch does not abort the pass. The pair it belonged to
/// stays pending and unstored, `last_error` names the failure, and the other
/// pairs are scored.
#[tokio::test]
async fn a_failing_batch_sets_last_error_and_the_pass_scores_the_rest() {
    let failing: Arc<dyn ContradictionScorer> = Arc::new(FailsOn {
        inner: stub(),
        marker: "Node 19",
    });
    let (_tmp, engine) = engine_with(loader_of(failing, Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Nineteen", NODE_19))
        .await
        .unwrap();
    three(&engine).await;
    // Three retry engrams make three pairs; the two with Nineteen fail.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 2)
    );
    assert!(
        engine
            .contradiction_last_error()
            .is_some_and(|e| e.contains("the batch failed")),
        "{:?}",
        engine.contradiction_last_error()
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    assert!(
        engine.contradictions_wanted(),
        "the tick retries the failed pairs"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 2)
    );
}

/// The candidate rule reads statuses in lowercase: a hand-written `Stable`
/// is current, a `Draft` is speculative.
#[tokio::test]
async fn a_capitalised_status_is_read_in_lowercase() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    for (title, content, status) in [
        ("Eighteen", NODE_18, "Stable"),
        ("Twenty", NODE_20, "Stable"),
        ("Nineteen", NODE_19, "Draft"),
    ] {
        engine
            .write_engram(&WriteParams {
                status: Some(status.to_string()),
                ..write(title, content)
            })
            .await
            .unwrap();
    }
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 4, 0),
        "Eighteen and Twenty only"
    );
}

/// L2 and Task 4's hand-off: the daemon's shutdown steps (release the host
/// locks, checkpoint, take the store lock) never wait on a model that is
/// loading or on a batch that is scoring, so `Departure` finishes inside its
/// watchdog whatever the pass is doing.
#[tokio::test]
async fn shutdown_steps_never_wait_on_a_loading_model() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let loader: ScorerLoader = {
        let entered = entered.clone();
        Arc::new(move |_model: &'static NliModel| {
            let entered = entered.clone();
            let held: futures::future::BoxFuture<'static, Loaded> = Box::pin(async move {
                entered.notify_one();
                std::future::pending::<Loaded>().await
            });
            held
        })
    };
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let pass = tokio::spawn({
        let engine = engine.clone();
        async move { engine.score_contradictions().await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("the pass reached the loader");
    shutdown_steps(&engine).await;
    pass.abort();
}

/// A scorer that blocks inside `score` until it is released.
struct Blocking {
    entered: Arc<AtomicBool>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl ContradictionScorer for Blocking {
    fn score(&self, pairs: &[(String, String)]) -> crystalline_index::Result<Vec<f32>> {
        self.entered.store(true, Ordering::SeqCst);
        let _ = self
            .release
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(30));
        Ok(vec![0.0; pairs.len()])
    }

    fn model_repo(&self) -> &str {
        full().repo
    }
}

#[tokio::test]
async fn shutdown_steps_never_wait_on_a_scoring_batch() {
    let entered = Arc::new(AtomicBool::new(false));
    let (release, rx) = std::sync::mpsc::channel();
    let scorer: Arc<dyn ContradictionScorer> = Arc::new(Blocking {
        entered: entered.clone(),
        release: std::sync::Mutex::new(rx),
    });
    let (_tmp, engine) = engine_with(loader_of(scorer, Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let pass = tokio::spawn({
        let engine = engine.clone();
        async move { engine.score_contradictions().await.unwrap() }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the pass reached the scorer");
    shutdown_steps(&engine).await;
    release.send(()).unwrap();
    assert_eq!(pass.await.unwrap(), scored(1, 4, 0));
}

/// The engine half of the daemon's `Departure` steps, each bounded well
/// inside the ten-second watchdog.
async fn shutdown_steps(engine: &Engine) {
    let bound = std::time::Duration::from_secs(5);
    tokio::time::timeout(bound, engine.release_hosts())
        .await
        .expect("releasing host locks");
    tokio::time::timeout(bound, engine.checkpoint_wal())
        .await
        .expect("checkpointing the WAL");
    let store = engine.store();
    let _held = tokio::time::timeout(bound, store.lock())
        .await
        .expect("waiting for the store");
}
