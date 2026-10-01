//! The contradiction check end to end on the engine: the profile setting,
//! the model cache keep list, the daemon pass with the stub scorer, pending,
//! the sweep's V302 read and the pair-of-lines acknowledgment.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crystalline_core::config::{
    DomainEntry, EmbeddingsConfig, EvolveConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::embed::{DEFAULT_MODEL_ID, line_similarity_floor};
use crystalline_index::nli::{
    ContradictionScorer, LineRules, NliModel, NliProfile, RETIRED_NLI_REPOS, StubScorer, nli_model,
    observation_hash, scoring_checksum,
};
use crystalline_index::sweep::{FactObservation, MAX_LINE_PAIRS_PER_ENGRAM_PAIR};
use crystalline_index::{
    ContradictionRow, EmbeddingProvider, IndexError, ObservationVector, TursoStore,
};
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::engine::{
    ConfigureAction, ContradictionOutcome, NLI_FETCH_RETRY_FIRST, NLI_FETCH_RETRY_MAX,
    NLI_IDLE_DROP, ScorerLoader,
};
use crystalline_service::params::{
    DeleteParams, EditParams, EvolveParams, MoveParams, WriteParams,
};
use tokio::sync::Mutex;

/// One virtual domain `notes` on an engine with the topic provider, so two
/// engrams about the retry queue share a lead vector and a docking note is
/// orthogonal to both.
async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
    engine_on(Arc::new(crate::support::TopicEmbedder)).await
}

/// [`engine`] with another embedding provider.
async fn engine_on(provider: Arc<dyn EmbeddingProvider>) -> (tempfile::TempDir, Arc<Engine>) {
    engine_with_domains(provider, &["notes"]).await
}

/// [`engine_on`] with these virtual domains.
async fn engine_with_domains(
    provider: Arc<dyn EmbeddingProvider>,
    domains: &[&str],
) -> (tempfile::TempDir, Arc<Engine>) {
    engine_with_config(provider, domains, |_| {}).await
}

/// [`engine_with_domains`] with `edit` applied to the config before the engine
/// is built, which is how a hand-edited value reaches it.
async fn engine_with_config(
    provider: Arc<dyn EmbeddingProvider>,
    domains: &[&str],
    edit: impl FnOnce(&mut GlobalConfig),
) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = GlobalConfig::default();
    edit(&mut cfg);
    for name in domains {
        cfg.domains
            .insert(name.to_string(), DomainEntry::virtual_domain());
    }
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
    set(&engine, "evolve.contradictions", "full").await;
    assert_eq!(
        engine.contradiction_model().map(|m| m.profile),
        Some(NliProfile::Full)
    );
    set(&engine, "evolve.contradictions", "off").await;
    assert!(engine.contradiction_model().is_none());
}

/// Only `off` and `full` ship: `light` and `english-only` (a development
/// build's profiles) are refused by `config set` with the accepted values
/// named, and nothing is written.
#[tokio::test]
async fn a_removed_profile_is_refused_with_the_accepted_values() {
    let (_tmp, engine) = engine().await;
    for removed in ["light", "english-only"] {
        let err = engine
            .configure(&ConfigureAction::Set {
                key: "evolve.contradictions".to_string(),
                value: removed.to_string(),
            })
            .await
            .expect_err("a removed profile is refused");
        let text = err.to_string();
        assert!(text.contains("off or full"), "{text}");
        assert!(text.contains(&format!("'{removed}'")), "{text}");
        assert!(engine.contradiction_model().is_none(), "still off");
    }
}

/// A config file written by a development build that still says `light` must
/// not break the daemon: the engine reads it as off (no model, nothing
/// scored, no walk wanted), the status block still carries the raw value so
/// status and doctor can say it is not known, and a pass is an honest no-op.
#[tokio::test]
async fn a_config_that_still_says_light_reads_as_off_and_reports_the_raw_value() {
    let (_tmp, engine) =
        engine_with_config(Arc::new(crate::support::TopicEmbedder), &["notes"], |cfg| {
            cfg.evolve = Some(EvolveConfig {
                contradictions: Some("light".to_string()),
            });
        })
        .await;
    assert!(engine.contradiction_model().is_none());
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["profile"], "light");
    assert!(status["model"].is_null(), "{status}");
    assert_eq!(status["scored_pairs"], 0);
    assert!(!engine.contradictions_wanted());
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    // Fixing the value is one `config set` away.
    set(&engine, "evolve.contradictions", "off").await;
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["profile"], "off");
}

/// A hub-shaped cache directory with one weight file.
fn hub_dir(root: &std::path::Path, repo: &str) {
    let dir = root
        .join(crystalline_index::hub_dir_name(repo))
        .join("snapshots/abc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.safetensors"), b"weights").unwrap();
}

fn cached_repos(root: &std::path::Path) -> Vec<String> {
    crystalline_index::cached_model_dirs(root)
        .into_iter()
        .map(|(repo, _)| repo)
        .collect()
}

#[tokio::test]
async fn the_model_cache_keeps_the_configured_profile_and_drops_the_others() {
    let (_tmp, engine) = engine().await;
    let cache = tempfile::tempdir().unwrap();
    let granite = "ibm-granite/granite-embedding-97m-multilingual-r2";
    hub_dir(cache.path(), granite);
    hub_dir(cache.path(), nli_model(NliProfile::Full).repo);
    // The checkpoints of the two profiles a development build had.
    for retired in RETIRED_NLI_REPOS {
        hub_dir(cache.path(), retired);
    }
    set(&engine, "evolve.contradictions", "full").await;
    engine.prune_model_cache(cache.path().to_path_buf()).await;
    assert_eq!(
        cached_repos(cache.path()),
        vec![
            nli_model(NliProfile::Full).repo.to_string(),
            granite.to_string()
        ],
        "the embedding model and the configured profile stay, the retired checkpoints go (sorted by repo id)"
    );

    // Off keeps no NLI checkpoint at all.
    set(&engine, "evolve.contradictions", "off").await;
    engine.prune_model_cache(cache.path().to_path_buf()).await;
    assert_eq!(cached_repos(cache.path()), vec![granite.to_string()]);
}

/// An install whose config still says `light` has the retired checkpoint on
/// disk: the unknown value keeps nothing, so the prune takes it.
#[tokio::test]
async fn a_retired_checkpoint_is_pruned_when_the_config_still_names_its_profile() {
    let (_tmp, engine) =
        engine_with_config(Arc::new(crate::support::TopicEmbedder), &["notes"], |cfg| {
            cfg.evolve = Some(EvolveConfig {
                contradictions: Some("light".to_string()),
            });
        })
        .await;
    let cache = tempfile::tempdir().unwrap();
    let granite = "ibm-granite/granite-embedding-97m-multilingual-r2";
    hub_dir(cache.path(), granite);
    for retired in RETIRED_NLI_REPOS {
        hub_dir(cache.path(), retired);
    }
    engine.prune_model_cache(cache.path().to_path_buf()).await;
    assert_eq!(cached_repos(cache.path()), vec![granite.to_string()]);
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
const POISONED: &str = "The retry queue build runs on a pinned runtime.\n\n- [fact] The retry queue is poisoned\n- [fact] Retries back off on the queue";

/// Embeds like [`crate::support::TopicEmbedder`] but refuses a batch that
/// holds exactly the line "The retry queue is poisoned", which only a line
/// embedding ever is: a chunk carries the title and body around it.
struct PoisonLine;

#[async_trait::async_trait]
impl EmbeddingProvider for PoisonLine {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        if texts.iter().any(|t| t == "The retry queue is poisoned") {
            return Err(IndexError::Embedding(
                "the provider refused a line".to_string(),
            ));
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
    // One kept line pair: Node 18 against Node 20 (both unmarked, 1.0); the
    // retry bullet is the same text on both sides and on another axis than
    // the Node lines.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1, "loaded lazily, once");
    // One first order, and the 0.93 pair read back in the second.
    assert_eq!(s.forwards(), 2);
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
    assert_eq!(s.forwards(), 2, "an up-to-date pair is not asked again");
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
        scored(1, 2, 0),
        "two kept of two lines against three: Node 18 against Node 20, and the retry bullet against the new queue line (one axis, other texts)"
    );
}

/// Review focus 4, for the one model that ships. A failed load is not retried
/// by the next pass, only once the setting is set again. The rows of the
/// first walk stay.
#[tokio::test]
async fn a_failed_load_is_asked_for_again_only_when_the_setting_is_set_again() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let loader: ScorerLoader = {
        let (full_stub, attempts) = (stub(), attempts.clone());
        Arc::new(move |_model: &'static NliModel| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            let s: Arc<dyn ContradictionScorer> = full_stub.clone();
            if n == 0 {
                ready(Err(IndexError::Nli(
                    "the first download failed".to_string(),
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
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(!engine.contradiction_scorer_loaded());
    assert!(
        engine
            .contradiction_last_error()
            .is_some_and(|e| e.contains("the first download failed")),
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
        1,
        "nor by the next pass: the loader wipes and downloads again on a failure"
    );

    // Setting the profile again is what asks for another load.
    set(&engine, "evolve.contradictions", "full").await;
    assert!(engine.contradiction_last_error().is_none());
    assert!(engine.contradictions_wanted());
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        rows(&engine, full().repo).await.len(),
        1,
        "the one pair above the store floor"
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
        scored(1, 1, 0),
        "one of the two copies against Node 20; the retry bullet is on another axis"
    );
    assert_eq!(
        s.forwards(),
        2,
        "one line pair, both orders (0.93), nothing twice"
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    // Off and on again forgets the settled digest, so the next walk parses
    // the domain and really checks the stored checksums.
    reparse_next(&engine).await;
    let walks = engine.contradiction_fact_walks();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks + 1,
        "the domain was parsed again"
    );
    assert_eq!(s.forwards(), 2);
    assert!(!engine.contradictions_wanted());
}

/// Set the profile again: the pass forgets its settled digests and its
/// pending count, so the next walk parses every domain. Not by way of off,
/// which now deletes the stored scores and line vectors as well.
async fn reparse_next(engine: &Engine) {
    set(engine, "evolve.contradictions", "full").await;
    assert!(
        engine.contradictions_status().await.unwrap()["pending_pairs"].is_null(),
        "pending is unknown again"
    );
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
    // Eighteen against Twenty, one kept line pair; the prose has no lines.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert!(!engine.contradictions_wanted());
    let before = s.forwards();
    reparse_next(&engine).await;
    let walks = engine.contradiction_fact_walks();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks + 1,
        "the domain was parsed again"
    );
    assert_eq!(s.forwards(), before);
    assert!(!engine.contradictions_wanted());
}

#[tokio::test]
async fn a_pass_is_bounded_and_the_next_one_finishes() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    // Thirty-eight engrams on the retry axis, each with one line all of them
    // share and one of its own. The shared line is the same text on both
    // sides and never a pair, so each of the 703 pairs keeps three line
    // pairs, 2109 in all, over the 2000 a pass may score. Three does not
    // divide 2000, whatever order the pairs come in.
    for i in 0..38 {
        engine
            .write_engram(&write(
                &format!("Retry note {i}"),
                &format!(
                    "The retry queue note {i}.\n\n- [fact] Retries back off on the queue\n- [fact] Retry fact {i}"
                ),
            ))
            .await
            .unwrap();
    }
    engine.embed_pending().await.unwrap();
    // 666 whole pairs take 1998; the next one holds three line pairs, more
    // than the two left, so it is turned away whole, not split. 37 are left.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(666, 1998, 37)
    );
    assert!(engine.contradictions_wanted(), "pairs remain");
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(37, 111, 0)
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
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(first.await.unwrap(), scored(1, 1, 0));
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
            ready(Err(IndexError::Nli("offline".to_string())))
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

/// Final review I4: a download that failed (offline at the first pass, a DNS
/// blip, a proxy outage) is not a broken checkpoint. The tick tries it again
/// on its own after five minutes, then after ten, twenty and so on up to an
/// hour, never in between; a setting change starts the wait over.
#[tokio::test(start_paused = true)]
async fn a_failed_download_is_retried_on_the_tick_with_a_backoff() {
    let (s, attempts) = (stub(), Arc::new(AtomicUsize::new(0)));
    let offline = Arc::new(AtomicUsize::new(2));
    let loader: ScorerLoader = {
        let (s, attempts, offline) = (s.clone(), attempts.clone(), offline.clone());
        Arc::new(move |_model: &'static NliModel| {
            attempts.fetch_add(1, Ordering::SeqCst);
            if offline.load(Ordering::SeqCst) > 0 {
                offline.fetch_sub(1, Ordering::SeqCst);
                ready(Err(IndexError::NliFetch(
                    "downloading model.safetensors: offline".to_string(),
                )))
            } else {
                ready(Ok(s.clone() as Arc<dyn ContradictionScorer>))
            }
        })
    };
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let minute = std::time::Duration::from_secs(60);

    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["load_failed"], true, "{status}");
    assert_eq!(status["load_retry"], true, "a download retries: {status}");
    assert!(
        !engine.contradictions_wanted(),
        "not before its wait is over"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1, "a write asks no load");

    tokio::time::advance(NLI_FETCH_RETRY_FIRST - minute).await;
    assert!(!engine.contradictions_wanted());
    tokio::time::advance(minute).await;
    assert!(
        engine.contradictions_wanted(),
        "five minutes on, the tick asks"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2, "tried once more");

    // The second failure waits twice as long.
    tokio::time::advance(NLI_FETCH_RETRY_FIRST).await;
    assert!(!engine.contradictions_wanted(), "the wait doubled");
    tokio::time::advance(NLI_FETCH_RETRY_FIRST).await;
    assert!(engine.contradictions_wanted());
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0),
        "back online: loaded and scored (one kept line pair)"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["load_failed"], false, "{status}");
    assert!(status["last_error"].is_null(), "{status}");
}

/// A build error stays blocked however long the tick waits (the loader has
/// already wiped and fetched the checkpoint once); only a set setting or a
/// restart asks again.
#[tokio::test(start_paused = true)]
async fn a_failed_build_is_never_retried_by_the_tick() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let loader: ScorerLoader = {
        let attempts = attempts.clone();
        Arc::new(move |_model: &'static NliModel| {
            attempts.fetch_add(1, Ordering::SeqCst);
            ready(Err(IndexError::Nli(
                "building model: bad weights".to_string(),
            )))
        })
    };
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["load_retry"], false, "{status}");
    tokio::time::advance(NLI_FETCH_RETRY_MAX * 3).await;
    assert!(!engine.contradictions_wanted());
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

/// A scorer that says it runs on the GPU: the stub's scores, the device a
/// real checkpoint reports after a Metal load.
struct OnMetal(Arc<StubScorer>);

impl ContradictionScorer for OnMetal {
    fn score(&self, pairs: &[(String, String)]) -> crystalline_index::Result<Vec<f32>> {
        self.0.score(pairs)
    }

    fn model_repo(&self) -> &str {
        self.0.model_repo()
    }

    fn device(&self) -> Option<crystalline_index::DeviceReport> {
        Some(crystalline_index::DeviceReport::metal())
    }
}

/// The status block names the device of the model in memory, the way the
/// embeddings line does, and nothing while no model is loaded: before the
/// first pass, after the idle drop, and for a scorer that runs no model.
#[tokio::test]
async fn the_status_names_the_device_only_while_the_model_is_loaded() {
    let loads = Arc::new(AtomicUsize::new(0));
    let (_tmp, engine) = engine_with(loader_of(Arc::new(OnMetal(stub())), loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let status = engine.contradictions_status().await.unwrap();
    assert!(status["device"].is_null(), "nothing loaded yet: {status}");
    engine.score_contradictions().await.unwrap();
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["device"], "metal", "{status}");
    let now = tokio::time::Instant::now();
    engine.drop_idle_scorer_at(now + NLI_IDLE_DROP + std::time::Duration::from_secs(1));
    let status = engine.contradictions_status().await.unwrap();
    assert!(status["device"].is_null(), "dropped, no guess: {status}");

    let (_tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    assert!(engine.contradiction_scorer_loaded());
    let status = engine.contradictions_status().await.unwrap();
    assert!(
        status["device"].is_null(),
        "the stub runs no model: {status}"
    );
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
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
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
    // Two kept: Node 18 against Node 20, the retry bullet against the new
    // queue line.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 2, 0)
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
    // Eighteen against Twenty, one kept line pair; the poisoned engram has
    // no lead vector, so it is no candidate.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
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
    // L7/lesson 62: 0 pending must not read as "fully checked" while the
    // poisoned chunk still has no lead vector.
    assert_eq!(
        engine.contradictions_status().await.unwrap()["embedding_pending"],
        true
    );
}

/// Embeds like the topic provider, and rejects every chunk batch while
/// `closed`. A chunk carries the title above its body, so it holds a line
/// break; an observation line is folded and never does, so the walk's line
/// embeddings go through and only the lead vectors wait.
struct GatedTopicEmbedder {
    closed: AtomicBool,
}

#[async_trait::async_trait]
impl EmbeddingProvider for GatedTopicEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        if self.closed.load(Ordering::SeqCst) && texts.iter().any(|t| t.contains('\n')) {
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
    assert_eq!(
        engine.contradictions_status().await.unwrap()["embedding_pending"],
        true,
        "0 pending while a candidate still lacks a lead vector"
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
        scored(1, 1, 0),
        "the vector arrived without a stamp change and the pair is scored (one kept line pair)"
    );
    assert_eq!(engine.contradiction_fact_walks(), walks + 1);
    assert_eq!(
        engine.contradictions_status().await.unwrap()["embedding_pending"],
        false,
        "every possible candidate has its vector now"
    );
}

/// Fails every batch that names `marker`, and answers the rest like `inner`.
struct FailsOn {
    inner: Arc<StubScorer>,
    marker: &'static str,
    /// How many failing batches it was asked to score.
    failed: AtomicUsize,
}

impl FailsOn {
    fn new(marker: &'static str) -> Arc<FailsOn> {
        Arc::new(FailsOn {
            inner: stub(),
            marker,
            failed: AtomicUsize::new(0),
        })
    }
}

impl ContradictionScorer for FailsOn {
    fn score(&self, pairs: &[(String, String)]) -> crystalline_index::Result<Vec<f32>> {
        if pairs
            .iter()
            .any(|(p, h)| p.contains(self.marker) || h.contains(self.marker))
        {
            self.failed.fetch_add(1, Ordering::SeqCst);
            return Err(IndexError::Nli("the batch failed".to_string()));
        }
        self.inner.score(pairs)
    }

    fn model_repo(&self) -> &str {
        self.inner.model_repo()
    }
}

/// L3: one failing batch does not abort the pass. The pair it belonged to
/// stays pending and unstored, `last_error` names the failure, and the other
/// pairs are scored. A known failure is left alone by every later walk; the
/// tick's mark retries it once.
#[tokio::test]
async fn a_failing_batch_sets_last_error_and_the_pass_scores_the_rest() {
    let failing = FailsOn::new("Node 19");
    let (_tmp, engine) =
        engine_with(loader_of(failing.clone(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Nineteen", NODE_19))
        .await
        .unwrap();
    three(&engine).await;
    // Three retry engrams make three pairs of one kept line pair each (the
    // Node lines); the two with Nineteen fail.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 2)
    );
    // The three pairs share one group, whose batch fails once; each pair is
    // then scored alone, and the two with Nineteen fail again.
    assert_eq!(failing.failed.load(Ordering::SeqCst), 3);
    assert!(
        engine
            .contradiction_last_error()
            .is_some_and(|e| e.contains("the batch failed")),
        "{:?}",
        engine.contradiction_last_error()
    );
    // L1: a parked batch failure is "N pairs failing", never "the model
    // could not be loaded" - the two share `last_error` but must render
    // differently, so the block says which one this is.
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["pending_pairs"], 2);
    assert_eq!(status["failing_pairs"], 2);
    assert_eq!(status["load_failed"], false, "{status}");
    assert!(
        status["last_error"]
            .as_str()
            .is_some_and(|e| e.contains("the batch failed")),
        "{status}"
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    assert!(
        engine.contradictions_wanted(),
        "while the model is loaded, the tick retries the failed pairs"
    );
    let walks = engine.contradiction_fact_walks();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 2),
        "the failed pairs still count as pending"
    );
    assert_eq!(
        failing.failed.load(Ordering::SeqCst),
        3,
        "a walk nobody marked leaves known failures alone"
    );
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks,
        "and does not parse their domain"
    );

    assert!(engine.mark_contradiction_retry());
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 2)
    );
    // The two as one group (one failed batch), then each alone (two).
    assert_eq!(
        failing.failed.load(Ordering::SeqCst),
        6,
        "the marked walk retried both once"
    );
    assert!(engine.contradiction_last_error().is_some());
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 2)
    );
    assert_eq!(
        failing.failed.load(Ordering::SeqCst),
        6,
        "the mark is spent"
    );
}

/// Review round 1, finding 2: a pair a batch always fails on neither makes
/// every write re-walk its domain nor keeps the model in memory.
#[tokio::test(start_paused = true)]
async fn a_failing_pair_is_not_rewalked_on_every_write_and_the_model_idles_out() {
    let failing = FailsOn::new("Node 19");
    let (_tmp, engine) =
        engine_with_domains(Arc::new(crate::support::TopicEmbedder), &["notes", "other"]).await;
    let engine = with_loader(
        engine,
        loader_of(failing.clone(), Arc::new(AtomicUsize::new(0))),
    );
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Nineteen", NODE_19))
        .await
        .unwrap();
    three(&engine).await;
    // Eighteen against Twenty, one kept line pair; the two with Nineteen fail.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 2)
    );
    let (walks, failed) = (
        engine.contradiction_fact_walks(),
        failing.failed.load(Ordering::SeqCst),
    );
    // Writes elsewhere, each followed by the embed worker's handover, four
    // minutes apart.
    for i in 0..3 {
        tokio::time::advance(std::time::Duration::from_secs(240)).await;
        engine
            .write_engram(&WriteParams {
                domain: "other".to_string(),
                // Prose with no observations: nothing to score there either.
                ..write(
                    &format!("Docking {i}"),
                    "Docking clamps seat in bay three under thrust.",
                )
            })
            .await
            .unwrap();
        engine.embed_pending().await.unwrap();
        engine.score_contradictions().await.unwrap();
    }
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks + 3,
        "only the domain that changed is parsed, never the failing one"
    );
    assert_eq!(
        failing.failed.load(Ordering::SeqCst),
        failed,
        "the failing pair is not scored again on a handover"
    );
    // Twelve minutes after the last pair was scored, the model goes.
    engine.drop_idle_scorer();
    assert!(
        !engine.contradiction_scorer_loaded(),
        "walks that scored nothing do not keep the model"
    );
    assert!(
        !engine.contradictions_wanted(),
        "a failing pair never loads a model by itself"
    );
    assert!(!engine.mark_contradiction_retry());
}

/// Final review M2: the tick marks a retry only while the model is loaded,
/// but the mark can outlive that (the tick marked while a pass or an embed
/// was in flight, and no walk came before the idle drop). A walk that finds
/// such a stale mark with the model gone retries nothing and loads nothing.
#[tokio::test]
async fn a_stale_retry_mark_after_an_idle_drop_loads_nothing() {
    let failing = FailsOn::new("Node 19");
    let loads = Arc::new(AtomicUsize::new(0));
    let (_tmp, engine) = engine_with(loader_of(failing.clone(), loads.clone())).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Nineteen", NODE_19))
        .await
        .unwrap();
    three(&engine).await;
    // Eighteen against Twenty, one kept line pair; the two with Nineteen fail.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 2)
    );
    assert!(engine.mark_contradiction_retry(), "marked while loaded");
    engine.drop_idle_scorer_at(
        tokio::time::Instant::now() + NLI_IDLE_DROP + std::time::Duration::from_secs(1),
    );
    assert!(!engine.contradiction_scorer_loaded());
    let failed = failing.failed.load(Ordering::SeqCst);
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 2)
    );
    assert_eq!(
        loads.load(Ordering::SeqCst),
        1,
        "the stale mark loads nothing"
    );
    assert_eq!(
        failing.failed.load(Ordering::SeqCst),
        failed,
        "and retries nothing"
    );
    assert!(!engine.contradiction_scorer_loaded());
}

/// Review round 1, finding 6: a store write that fails is a failure of that
/// pair, not of the walk.
#[tokio::test]
async fn a_failed_store_write_skips_the_pair_and_the_walk_goes_on() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    // The line vectors are in place first: the open transaction below would
    // refuse their write too, and the pair would then wait for its lines
    // instead of failing at the score write this test is about.
    {
        let lines: Vec<ObservationVector> = [
            "The build uses Node 18",
            "The build uses Node 20",
            "Retries back off on the queue",
            "Deployments run on Fridays",
            "The clamps misread below eight degrees",
        ]
        .iter()
        .map(|t| ObservationVector {
            hash: observation_hash(t),
            vector: crate::support::TopicEmbedder::embed_one(t),
        })
        .collect();
        let store = engine.store();
        let store = store.lock().await;
        store
            .store_observation_vectors(DEFAULT_MODEL_ID, &lines)
            .await
            .unwrap();
    }
    // An open transaction makes `replace_contradictions` refuse.
    let store = engine.store();
    store.lock().await.begin().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 1),
        "the walk returns, it does not abort"
    );
    assert!(engine.contradiction_last_error().is_some());
    store.lock().await.rollback().await.unwrap();
    assert!(engine.mark_contradiction_retry());
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert!(engine.contradiction_last_error().is_none());
}

/// Review round 1, finding 1: a setting change that lands while a walk runs
/// is not lost. The old walk finishes its pairs, which are valid scores of
/// the same model, but publishes nothing: pending stays unknown, so the next
/// pass recounts without any write.
#[tokio::test]
async fn a_setting_change_during_a_walk_is_not_lost() {
    let (gate, entered) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    let first_load = Arc::new(AtomicBool::new(true));
    let loader: ScorerLoader = {
        let (gate, entered, first_load) = (gate.clone(), entered.clone(), first_load.clone());
        let full_stub = stub();
        Arc::new(move |_model: &'static NliModel| {
            let (gate, entered) = (gate.clone(), entered.clone());
            let s: Arc<dyn ContradictionScorer> = full_stub.clone();
            let hold = first_load.swap(false, Ordering::SeqCst);
            let held: futures::future::BoxFuture<'static, Loaded> = Box::pin(async move {
                if hold {
                    entered.notify_one();
                    gate.notified().await;
                }
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
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("the walk reached the loader");
    set(&engine, "evolve.contradictions", "full").await;
    gate.notify_one();
    first.await.unwrap();
    assert_eq!(
        rows(&engine, full().repo).await.len(),
        1,
        "the old walk's scores of the same model stand"
    );
    assert!(
        engine.contradictions_wanted(),
        "the old walk did not overwrite the unknown pending"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0),
        "the recount finds nothing left to score"
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    assert!(
        !engine.contradictions_wanted(),
        "and the pending count is known again"
    );
}

/// Finding 1 at its narrowest: the setting moves while the walk's last pair
/// is scoring, so the walk finishes that pair and then has nothing left. What
/// it found is under the old generation and is not published, so pending
/// stays unknown and the next pass recounts.
#[tokio::test]
async fn a_setting_change_during_the_last_pair_is_not_lost() {
    let entered = Arc::new(AtomicBool::new(false));
    let (release, rx) = std::sync::mpsc::channel();
    let blocking: Arc<dyn ContradictionScorer> = Arc::new(Blocking {
        entered: entered.clone(),
        release: std::sync::Mutex::new(rx),
    });
    let loader: ScorerLoader =
        Arc::new(move |_model: &'static NliModel| ready(Ok(blocking.clone())));
    let (_tmp, engine) = engine_with(loader).await;
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
    set(&engine, "evolve.contradictions", "full").await;
    release.send(()).unwrap();
    pass.await.unwrap();
    assert!(
        engine.contradictions_wanted(),
        "the old walk did not publish its empty pending over the change"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0),
        "recounted without any write"
    );
    assert!(
        !engine.contradictions_wanted(),
        "and the pending count is known again"
    );
}

/// Review round 1, finding 7: turning the check off ends the walk in flight
/// after the pair it is on, which is not stored, and lets go of the model.
#[tokio::test]
async fn turning_the_check_off_stops_the_walk_in_flight() {
    let entered = Arc::new(AtomicBool::new(false));
    let (release, rx) = std::sync::mpsc::channel();
    let scorer: Arc<dyn ContradictionScorer> = Arc::new(Blocking {
        entered: entered.clone(),
        release: std::sync::Mutex::new(rx),
    });
    let (_tmp, engine) = engine_with(loader_of(scorer, Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Nineteen", NODE_19))
        .await
        .unwrap();
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
    set(&engine, "evolve.contradictions", "off").await;
    release.send(()).unwrap();
    // One release only: a walk that went on to another pair would park in
    // the scorer for its ten seconds, so a quick end is the walk stopping.
    tokio::time::timeout(std::time::Duration::from_secs(5), pass)
        .await
        .expect("the walk stopped after the pair in flight")
        .unwrap();
    let pairs = {
        let store = engine.store();
        let store = store.lock().await;
        store.scored_pair_count(full().repo).await.unwrap()
    };
    // The pair in flight finishes scoring, but it is no longer stored: the
    // profile is read again under the store lock, and off deletes the scores
    // anyway, so nothing may land after that clear.
    assert_eq!(
        pairs, 0,
        "the pair in flight is not stored, no other is scored"
    );
    assert!(
        !engine.contradiction_scorer_loaded(),
        "the model went with the walk"
    );
    assert!(!engine.contradictions_wanted());
}

/// Review round 1, finding 4: after one request, the worker makes a bounded
/// number of walks and does not ask itself again in a loop, even with a pair
/// that fails every time.
#[tokio::test(start_paused = true)]
async fn the_worker_settles_after_a_request_with_a_failing_pair() {
    let failing = FailsOn::new("Node 19");
    let (_tmp, engine) =
        engine_with(loader_of(failing.clone(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Nineteen", NODE_19))
        .await
        .unwrap();
    three(&engine).await;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(
        Arc::try_unwrap(engine)
            .ok()
            .expect("one owner")
            .with_contradiction_channel(tx),
    );
    tokio::spawn(crystalline_service::engine::run_contradiction_worker(
        engine.clone(),
        rx,
    ));
    assert!(engine.request_contradictions());
    // Paused: each sleep lets the worker run until it is idle.
    let mut last = u64::MAX;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let now = engine.contradiction_walks();
        if now == last && !engine.contradictions_in_flight() {
            break;
        }
        last = now;
    }
    let settled = engine.contradiction_walks();
    assert!(
        (1..=3).contains(&settled),
        "a bounded number of walks: {settled}"
    );
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert_eq!(
        engine.contradiction_walks(),
        settled,
        "the worker does not ask itself again"
    );
    // One failed group batch, then the two pairs with Nineteen alone.
    assert_eq!(failing.failed.load(Ordering::SeqCst), 3);
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
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
        scored(1, 1, 0),
        "Eighteen and Twenty only, one kept line pair"
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
            .recv_timeout(std::time::Duration::from_secs(10));
        Ok(vec![0.0; pairs.len()])
    }

    fn model_repo(&self) -> &str {
        full().repo
    }
}

/// Task 7: the `contradictions` block `ctl status` and the standalone
/// fallback both carry. Off names no model and no pending; a fresh profile
/// knows nothing until a walk runs (`pending_pairs` stays null, never 0, per
/// L7); once a walk scores the pair, pending, failing and scored all read
/// back correctly.
#[tokio::test]
async fn the_status_block_reports_profile_model_pending_and_scored_pairs() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    let off = engine.status_report().await.unwrap();
    assert_eq!(off["contradictions"]["profile"], "off");
    assert_eq!(off["contradictions"]["model"], serde_json::Value::Null);
    assert_eq!(
        off["contradictions"]["pending_pairs"],
        serde_json::Value::Null
    );
    assert_eq!(
        off["contradictions"]["failing_pairs"],
        serde_json::Value::Null
    );
    assert_eq!(off["contradictions"]["load_failed"], false);
    assert_eq!(off["contradictions"]["embedding_pending"], false);
    assert_eq!(off["contradictions"]["scored_pairs"], 0);

    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let before = engine.contradictions_status().await.unwrap();
    assert_eq!(before["model"], full().id);
    assert_eq!(
        before["pending_pairs"],
        serde_json::Value::Null,
        "unknown until a pass ran"
    );
    engine.score_contradictions().await.unwrap();
    let after = engine.status_report().await.unwrap();
    assert_eq!(after["contradictions"]["pending_pairs"], 0);
    assert_eq!(after["contradictions"]["failing_pairs"], 0);
    assert_eq!(after["contradictions"]["scored_pairs"], 1);
    assert_eq!(
        after["contradictions"]["last_error"],
        serde_json::Value::Null
    );
    assert_eq!(after["contradictions"]["load_failed"], false);
}

/// The error-label minor: a load failure's `last_error` is the pending count
/// plus a message that never says "embedding error" (it is not an embedding
/// model), and `load_failed` says which of the two failure shapes this is.
#[tokio::test]
async fn a_failed_load_is_reported_with_the_pending_count_and_never_as_an_embedding_error() {
    let loader: ScorerLoader = Arc::new(|_model: &'static NliModel| {
        ready(Err(IndexError::Nli(
            "downloading model.safetensors: offline".to_string(),
        )))
    });
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["pending_pairs"], 1);
    assert_eq!(status["load_failed"], true, "{status}");
    let err = status["last_error"].as_str().unwrap();
    assert!(err.contains("offline"), "{err}");
    assert!(!err.contains("embedding error"), "{err}");
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
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(pass.await.unwrap(), scored(1, 1, 0));
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

// --- the sweep's V302 read -------------------------------------------------

/// The detection half of the sweep, which records no maintenance run, so a
/// test never writes the developer's state file.
async fn sweep(engine: &Engine) -> serde_json::Value {
    engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["notes".to_string()],
                families: vec!["meaning".to_string()],
                limit: Some(50),
                today: Some("2026-09-27".to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// Final review M5: a truncation line belongs to its rule, so it follows the
/// `--family` and `--rule` filters the findings follow. A temporal sweep
/// never says V302 pairs are not counted; a V302 sweep does.
#[tokio::test]
async fn truncation_lines_follow_the_family_and_rule_filters() {
    let (_tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let lines = |families: &[&str], rules: &[&str]| {
        let p = EvolveParams {
            domains: vec!["notes".to_string()],
            families: families.iter().map(|f| f.to_string()).collect(),
            rules: rules.iter().map(|r| r.to_string()).collect(),
            limit: Some(50),
            today: Some("2026-09-27".to_string()),
            ..EvolveParams::default()
        };
        let engine = engine.clone();
        async move {
            let value = engine
                .evolve_detect(&p, &Scope::Unrestricted)
                .await
                .unwrap();
            value["truncations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap().to_string())
                .collect::<Vec<String>>()
        }
    };
    assert_eq!(lines(&[], &[]).await, vec![NOT_COUNTED.to_string()]);
    assert_eq!(
        lines(&["meaning"], &[]).await,
        vec![NOT_COUNTED.to_string()]
    );
    assert_eq!(lines(&[], &["V302"]).await, vec![NOT_COUNTED.to_string()]);
    assert!(lines(&["temporal"], &[]).await.is_empty());
    assert!(lines(&[], &["V301"]).await.is_empty());
    assert!(lines(&["meaning"], &["V301"]).await.is_empty());
}

fn v302(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["rule"] == "V302")
        .collect()
}

fn v302_truncations(value: &serde_json::Value) -> Vec<String> {
    value["truncations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t.as_str())
        .filter(|t| t.contains("V302"))
        .map(str::to_string)
        .collect()
}

fn ack(permalink: &str, value: &str, scope: Option<&str>) -> EditParams {
    EditParams {
        identifier: permalink.to_string(),
        domain: "notes".to_string(),
        operation: "set_frontmatter".to_string(),
        key: Some("evolve_ack".to_string()),
        value: Some(value.to_string()),
        ack_scope: scope.map(str::to_string),
        ..EditParams::default()
    }
}

/// The stored markdown of `permalink` in `notes`.
async fn content_of(engine: &Engine, permalink: &str) -> String {
    let store = engine.store();
    let store = store.lock().await;
    let d = store
        .list_engrams("notes", None, None)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.permalink == permalink)
        .unwrap();
    store
        .engram_content(d.domain_id, &d.path)
        .await
        .unwrap()
        .unwrap()
}

/// A loader whose first load fails and every later one hands out `stub`: a
/// walk that counts and cannot score, then one that scores.
fn offline_once(stub: Arc<StubScorer>) -> ScorerLoader {
    let first = Arc::new(AtomicBool::new(true));
    Arc::new(move |_model: &'static NliModel| {
        if first.swap(false, Ordering::SeqCst) {
            ready(Err(IndexError::Nli("offline".to_string())))
        } else {
            ready(Ok(stub.clone() as Arc<dyn ContradictionScorer>))
        }
    })
}

const NOT_COUNTED: &str =
    "notes - V302: related pairs not counted yet (the daemon counts them after embedding)";

/// What the V302 lines say instead of promising a pass while the model's
/// failed load blocks the daemon.
const UNAVAILABLE: &str = "the contradiction model could not be loaded (crystalline status and crystalline doctor say why; setting evolve.contradictions again or restarting the daemon retries)";

/// While a failed load blocks the pass, an edit leaves the domain uncounted
/// for good, so the line names the failure and where to read it rather than
/// a pass that will not come.
#[tokio::test]
async fn a_blocked_model_never_promises_a_pass() {
    let loader: ScorerLoader =
        Arc::new(|_model: &'static NliModel| ready(Err(IndexError::Nli("offline".to_string()))));
    let (_tmp, engine) = engine_with(loader).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    engine.edit_engram(&append_to_twenty()).await.unwrap();
    engine.embed_pending().await.unwrap();
    engine.score_contradictions().await.unwrap();
    assert_eq!(
        v302_truncations(&sweep(&engine).await),
        vec![format!(
            "notes - V302: related pairs not counted: {UNAVAILABLE}"
        )]
    );
}

/// A walk that skips a settled domain keeps the domain's record, so a sweep
/// after two walks with nothing changed still reads as counted.
#[tokio::test]
async fn a_settled_domain_stays_counted_across_a_skipping_walk() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    engine.score_contradictions().await.unwrap();
    let (parsed, walked) = (
        engine.contradiction_fact_walks(),
        engine.contradiction_walks(),
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Scored {
            pairs: 0,
            line_pairs: 0,
            remaining: 0
        }
    );
    assert_eq!(engine.contradiction_walks(), walked + 1, "a walk ran");
    assert_eq!(
        engine.contradiction_fact_walks(),
        parsed,
        "and skipped the settled domain without parsing it"
    );
    let value = sweep(&engine).await;
    assert!(v302_truncations(&value).is_empty(), "{value}");
    assert_eq!(v302(&value).len(), 1, "{value}");
}

#[tokio::test]
async fn evolve_raises_v302_from_stored_rows_and_never_scores_inline() {
    let s = stub();
    let (_tmp, engine) = engine_with(offline_once(s.clone())).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;

    // No walk yet: the count is unknown, and says so.
    let unknown = sweep(&engine).await;
    assert!(v302(&unknown).is_empty());
    assert_eq!(
        v302_truncations(&unknown),
        vec![NOT_COUNTED.to_string()],
        "{unknown}"
    );

    // A walk that counts and cannot load the model: the count is known.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::ModelUnavailable
    );
    let before = sweep(&engine).await;
    assert!(v302(&before).is_empty());
    assert_eq!(
        v302_truncations(&before),
        vec![format!(
            "notes - V302: 1 related pairs not scored: {UNAVAILABLE}"
        )],
        "a blocked model promises no pass: {before}"
    );
    assert_eq!(s.forwards(), 0, "a sweep never scores");

    set(&engine, "evolve.contradictions", "full").await;
    engine.score_contradictions().await.unwrap();
    let forwards = s.forwards();
    let after = sweep(&engine).await;
    assert_eq!(s.forwards(), forwards, "a sweep never scores");
    let rows = v302(&after);
    assert_eq!(rows.len(), 1, "{after}");
    let row = rows[0];
    assert_eq!(row["class"], "judgment");
    assert!(
        row["finding"]
            .as_str()
            .unwrap()
            .contains("read as a contradiction at probability 0.93"),
        "{row}"
    );
    assert_eq!(row["probability"], 0.93);
    assert!(
        matches!(row["counterpart"].as_str(), Some("eighteen" | "twenty")),
        "{row}"
    );
    assert_ne!(row["counterpart"], row["permalink"]);
    assert!(row["counterpart_title"].as_str().is_some());
    assert!(row["counterpart_line"].as_u64().is_some(), "{row}");
    assert!(
        row["scope"].as_str().is_some(),
        "pair-scoped rows carry their scope"
    );
    assert!(
        after["families"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["family"] == "meaning")
    );
    assert!(v302_truncations(&after).is_empty(), "{after}");
    let legend = after["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["rule"] == "V302")
        .expect("the legend names the rule on the page");
    assert_eq!(legend["summary"], "possible contradiction");

    // Acknowledge the pair of lines through the row's scope: silent.
    let permalink = row["permalink"].as_str().unwrap().to_string();
    let scope = row["scope"].as_str().unwrap().to_string();
    engine
        .edit_engram(&ack(&permalink, "V302 different builds", Some(&scope)))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    let silenced = sweep(&engine).await;
    assert!(v302(&silenced).is_empty(), "{silenced}");
    assert_eq!(silenced["acknowledged"]["by_family"]["meaning"], 1);

    // Take it back: the finding resurfaces.
    engine
        .edit_engram(&ack(&permalink, "remove V302", None))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(v302(&sweep(&engine).await).len(), 1);
    assert_eq!(
        s.forwards(),
        forwards,
        "no sweep and no acknowledgment ever scored"
    );
}

#[tokio::test]
async fn an_off_check_is_silent_in_the_sweep_even_with_rows_stored() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    set(&engine, "evolve.contradictions", "off").await;
    let walks = engine.contradiction_fact_walks();
    let value = sweep(&engine).await;
    assert!(v302(&value).is_empty());
    assert!(v302_truncations(&value).is_empty(), "{value}");
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks,
        "an off check costs the sweep no second parse"
    );
}

/// The performance ruling: a sweep with the check on reads what the walk
/// counted and never parses the domain or walks the candidates a second time,
/// and neither does the sweep the acknowledgment path runs to resolve a scope.
#[tokio::test]
async fn a_sweep_and_an_acknowledgment_never_walk_the_candidates() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    let walks = engine.contradiction_fact_walks();
    let row = v302(&sweep(&engine).await)[0].clone();
    engine
        .edit_engram(&ack(row["permalink"].as_str().unwrap(), "V302 fine", None))
        .await
        .unwrap();
    assert_eq!(engine.contradiction_fact_walks(), walks);
}

/// The stale zero the digest exists for: the walk counted nothing pending,
/// then an edit made a new question. Until the next walk the sweep says the
/// count is unknown, never that nothing is pending.
#[tokio::test]
async fn an_edit_since_the_last_walk_reads_as_not_counted_never_as_quiet() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    assert!(v302_truncations(&sweep(&engine).await).is_empty());

    engine.edit_engram(&append_to_twenty()).await.unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        v302_truncations(&sweep(&engine).await),
        vec![NOT_COUNTED.to_string()]
    );

    engine.score_contradictions().await.unwrap();
    assert!(v302_truncations(&sweep(&engine).await).is_empty());
}

/// Lesson 40: the acknowledgment is a frontmatter write and leaves the body
/// and every other key as they were, apart from the edit stamp every write
/// makes (see the note below). Lesson 41: it holds for the two lines wherever they
/// move, and a move of the engram resurfaces the finding as a plain one rather
/// than silencing it under an address that no longer exists.
#[tokio::test]
async fn a_line_pair_acknowledgment_survives_a_renumbering_and_resurfaces_after_a_move() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    let row = v302(&sweep(&engine).await)[0].clone();
    let permalink = row["permalink"].as_str().unwrap().to_string();
    let other = row["counterpart"].as_str().unwrap().to_string();

    let before = content_of(&engine, &permalink).await;
    engine
        .edit_engram(&ack(
            &permalink,
            "V302 different builds",
            row["scope"].as_str(),
        ))
        .await
        .unwrap();
    let after = content_of(&engine, &permalink).await;
    let strip = |text: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut in_ack = false;
        for line in text.lines() {
            if line.starts_with("evolve_ack:") {
                in_ack = true;
                continue;
            }
            if in_ack && line.starts_with(' ') {
                continue;
            }
            in_ack = false;
            // Known gap against lesson 40, left for a ruling: every edit
            // restamps the `generated` block, an acknowledgment of any rule
            // included, and evolve.rs's
            // `an_edit_and_an_ack_both_survive_a_block_form_generated_mapping`
            // pins that. Here only its `at` moves; everything else holds.
            match line.split_once(", at: ") {
                Some((by, _)) if line.starts_with("generated:") => out.push(by.to_string()),
                _ => out.push(line.to_string()),
            }
        }
        out
    };
    assert!(after.contains("evolve_ack:"), "{after}");
    assert_eq!(
        strip(&after),
        strip(&before),
        "only the evolve_ack key and the edit stamp changed:\n{before}\n---\n{after}"
    );

    // A line inserted above the pair on the other engram: the lines move, the
    // hashes do not, and the acknowledgment still holds.
    engine
        .edit_engram(&EditParams {
            identifier: other.clone(),
            domain: "notes".to_string(),
            operation: "prepend".to_string(),
            content: Some("A line above everything.\n".to_string()),
            ..EditParams::default()
        })
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    let moved = sweep(&engine).await;
    assert!(v302(&moved).is_empty(), "{moved}");
    assert_eq!(moved["acknowledged"]["by_family"]["meaning"], 1, "{moved}");

    // A move changes the address the scope names: the pair is asked again,
    // plain, never marked stale.
    engine
        .move_engram(
            &MoveParams {
                identifier: other.clone(),
                domain: "notes".to_string(),
                destination: "archive/renamed".to_string(),
                destination_domain: None,
                permalink: Some("path".to_string()),
                update_links: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    // The daemon's pass after embedding: the move rewrote the file, so the
    // pair is scored again under its new checksum.
    engine.score_contradictions().await.unwrap();
    let resurfaced = sweep(&engine).await;
    let rows = v302(&resurfaced);
    assert_eq!(rows.len(), 1, "{resurfaced}");
    assert!(rows[0]["ack_stale"].is_null(), "{}", rows[0]);
}

/// Lesson 66: the queue is TOON for an agent, and a V302 row's four extra
/// columns sit beside a V301 row that has none. The encoder fills the missing
/// cells with null, so the queue stays one table; pinned byte for byte.
#[tokio::test]
async fn a_v302_row_renders_in_the_toon_queue_beside_a_v301_row() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    let value = sweep(&engine).await;
    let rules: Vec<&str> = value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, vec!["V302", "V301"], "{value}");
    let text = crystalline_engine::toon::render(&value);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("queue["))
        .unwrap_or_else(|| panic!("no tabular queue: {text}"));
    let v302_scope = value["queue"][0]["scope"].as_str().unwrap();
    let v301_scope = value["queue"][1]["scope"].as_str().unwrap();
    assert_eq!(
        lines[at..at + 3].to_vec(),
        vec![
            "queue[2]{class,counterpart,counterpart_line,counterpart_title,domain,evidence,finding,fix,line,n,permalink,priority,probability,rule,scope,title}:".to_string(),
            format!(
                "  judgment,twenty,14,Twenty,notes,notes/eighteen line 14; notes/twenty line 14; probability 0.93; model mdeberta-v3-base-xnli-2mil7,\"\\\"The build uses Node 18\\\" (Eighteen) against \\\"The build uses Node 20\\\" (Twenty) read as a contradiction at probability 0.93\",read both then supersede or close a window or acknowledge V302,14,1,eighteen,85,0.93,V302,\"{v302_scope}\",Eighteen"
            ),
            format!(
                "  judgment,null,null,null,notes,\"lead-vector cosine 1.00 at or above 0.94; twin: notes/twenty\",semantic twin of notes/twenty,read both then merge and supersede or link and acknowledge,null,2,eighteen,75,null,V301,\"{v301_scope}\",Eighteen"
            ),
        ],
        "{text}"
    );
}

/// A file domain `notes` holding the Node 18 and Node 20 engrams, on an
/// engine with the topic provider and `loader`, synced and embedded.
async fn file_engine(loader: ScorerLoader) -> (tempfile::TempDir, std::path::PathBuf, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("notes");
    std::fs::create_dir_all(&dir).unwrap();
    let engram = |title: &str, body: &str| {
        format!(
            "---\ntype: engram\ntitle: {title}\npermalink: {}\ntags:\n  - t\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n{body}\n",
            title.to_lowercase()
        )
    };
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: notes\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# notes\n\n## Scope\n\n- Build notes\n",
    )
    .unwrap();
    std::fs::write(dir.join("eighteen.md"), engram("Eighteen", NODE_18)).unwrap();
    std::fs::write(dir.join("twenty.md"), engram("Twenty", NODE_20)).unwrap();
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert("notes".to_string(), DomainEntry::file(dir.clone()));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        Some(Arc::new(crate::support::TopicEmbedder)),
        Some(config_path),
    )
    .with_state_dir(root.join("state"))
    .with_scorer_loader(loader);
    let engine = Arc::new(engine);
    engine.sync(None).await.unwrap();
    engine.embed_pending().await.unwrap();
    (tmp, dir, engine)
}

/// Final review I1: removing a domain clears its rows, so a domain added
/// back under the same name from the same folder in one daemon lifetime is
/// scored again, never skipped as settled over an empty table (unscored
/// must not read as clean, lesson 37).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_removed_and_added_back_is_scored_again() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, dir, engine) = file_engine(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );

    engine.domain_remove("notes").await.unwrap();
    engine
        .domain_add_local(Some("notes"), Some(dir.to_str().unwrap()))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert!(
        engine.contradictions_wanted(),
        "pending is unknown again, so the tick asks"
    );
    let unknown = sweep(&engine).await;
    assert_eq!(
        v302_truncations(&unknown),
        vec![NOT_COUNTED.to_string()],
        "not counted, never quiet: {unknown}"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0),
        "the re-added domain is scored again"
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
}

/// Final review I3: a pair is fresh while both engrams' observation lines
/// are what the model read, not while their files are byte for byte the
/// same. Acknowledging a finding (the V302 remedy itself) or raising an
/// engram's salience rewrites the file and leaves every line as it was, so
/// the next walk loads no model and scores nothing. A changed line is a new
/// question and is scored again.
#[tokio::test]
async fn a_frontmatter_edit_rescores_nothing_and_a_changed_line_rescores_the_pair() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s.clone(), loads.clone())).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    let idle = || {
        engine.drop_idle_scorer_at(
            tokio::time::Instant::now() + NLI_IDLE_DROP + std::time::Duration::from_secs(1),
        );
        assert!(!engine.contradiction_scorer_loaded());
    };
    idle();

    let row = v302(&sweep(&engine).await)[0].clone();
    let permalink = row["permalink"].as_str().unwrap().to_string();
    let scope = row["scope"].as_str().unwrap().to_string();
    let before = content_of(&engine, &permalink).await;
    engine
        .edit_engram(&ack(&permalink, "V302 different builds", Some(&scope)))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_ne!(
        content_of(&engine, &permalink).await,
        before,
        "the file moved"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0),
        "an acknowledgment is no new question"
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1, "and loads no model");

    let salience = EditParams {
        identifier: "twenty".to_string(),
        domain: "notes".to_string(),
        operation: "set_frontmatter".to_string(),
        key: Some("salience".to_string()),
        value: Some("7".to_string()),
        ..EditParams::default()
    };
    let before = content_of(&engine, "twenty").await;
    engine.edit_engram(&salience).await.unwrap();
    engine.embed_pending().await.unwrap();
    assert_ne!(
        content_of(&engine, "twenty").await,
        before,
        "the file moved"
    );
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0),
        "a salience change is no new question"
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1, "and loads no model");

    // A changed observation line is: the pair is scored again.
    let forwards = s.forwards();
    engine.edit_engram(&append_to_twenty()).await.unwrap();
    engine.embed_pending().await.unwrap();
    // Two kept: Node 18 against Node 20, the retry bullet against the new
    // queue line.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 2, 0)
    );
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    assert!(s.forwards() > forwards);
}

// --- line vectors (V302 line filter) ------------------------------------------

/// The line vectors the engine's own model keeps.
async fn line_hashes(engine: &Engine) -> Vec<String> {
    hashes_of(engine, DEFAULT_MODEL_ID).await
}

/// The line vectors `model` keeps.
async fn hashes_of(engine: &Engine, model: &str) -> Vec<String> {
    let store = engine.store();
    let store = store.lock().await;
    let mut all = store.observation_vector_hashes(model).await.unwrap();
    all.sort();
    all
}

/// An engine whose embedding model has no measured line-similarity floor (a
/// remote model), with the stub loader counting its loads in `loads`.
async fn engine_without_floor(loads: Arc<AtomicUsize>) -> (tempfile::TempDir, Arc<Engine>) {
    let (tmp, engine) =
        engine_with_config(Arc::new(crate::support::TopicEmbedder), &["notes"], |cfg| {
            cfg.embeddings = Some(EmbeddingsConfig {
                provider: "remote".to_string(),
                model: "text-embedding-3-small".to_string(),
                endpoint: Some("http://127.0.0.1:9".to_string()),
                api_key_env: None,
            });
        })
        .await;
    (tmp, with_loader(engine, loader(stub(), loads)))
}

/// Embeds like [`crate::support::TopicEmbedder`] and records every text it
/// was handed, so a test can tell line embeddings from chunk embeddings.
struct Recording(std::sync::Mutex<Vec<String>>);

#[async_trait::async_trait]
impl EmbeddingProvider for Recording {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        self.0.lock().unwrap().extend(texts.iter().cloned());
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

fn delete_fridays() -> DeleteParams {
    DeleteParams {
        identifier: "fridays".to_string(),
        domain: "notes".to_string(),
        expected_checksum: None,
    }
}

#[tokio::test]
async fn line_vectors_stay_empty_while_off() {
    let (_tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    assert!(
        line_hashes(&engine).await.is_empty(),
        "off embeds and stores nothing"
    );
}

#[tokio::test]
async fn line_vectors_are_embedded_once_per_text_and_shared() {
    let recording = Arc::new(Recording(std::sync::Mutex::new(Vec::new())));
    let (_tmp, engine) = engine_on(recording.clone()).await;
    let engine = with_loader(engine, loader(stub(), Arc::new(AtomicUsize::new(0))));
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    recording.0.lock().unwrap().clear();
    engine.score_contradictions().await.unwrap();
    // Eighteen, Twenty and Fridays hold five distinct lines; "Retries back
    // off on the queue" is in two engrams and is embedded once.
    assert_eq!(line_hashes(&engine).await.len(), 5);
    let lines: Vec<String> = recording.0.lock().unwrap().clone();
    assert_eq!(
        lines
            .iter()
            .filter(|t| *t == "Retries back off on the queue")
            .count(),
        1,
        "{lines:?}"
    );
    // An edit embeds only the line whose text is new.
    engine.edit_engram(&append_to_twenty()).await.unwrap();
    engine.embed_pending().await.unwrap();
    recording.0.lock().unwrap().clear();
    engine.score_contradictions().await.unwrap();
    let lines: Vec<String> = recording.0.lock().unwrap().clone();
    assert_eq!(lines, vec!["The queue drains hourly".to_string()]);
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["lines_eligible"], 6);
    assert_eq!(status["lines_embedded"], 6);
    assert_eq!(status["line_floor"], 0.86);
    assert_eq!(status["line_floor_missing"], false);
    assert_eq!(status["embedding_model"], DEFAULT_MODEL_ID);
}

#[tokio::test]
async fn turning_off_clears_line_vectors_and_scores() {
    let (_tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    assert!(!line_hashes(&engine).await.is_empty());
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    set(&engine, "evolve.contradictions", "off").await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    assert!(line_hashes(&engine).await.is_empty());
    assert!(rows(&engine, full().repo).await.is_empty());
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["scored_pairs"], 0);
    assert!(
        status["lines_embedded"].is_null(),
        "off counts nothing: {status}"
    );
    // On again: embedded and scored anew.
    set(&engine, "evolve.contradictions", "full").await;
    engine.score_contradictions().await.unwrap();
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    assert_eq!(line_hashes(&engine).await.len(), 5);
}

/// Preflight H1: a daemon always carries an instance id, so the off switch
/// and the prune are gated on a shared database, never on the id. On turso
/// with an id, off still deletes the line vectors and a full walk still
/// prunes.
#[tokio::test]
async fn an_instance_id_on_an_unshared_store_still_clears_and_prunes() {
    let (_tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    let engine = Arc::new(
        Arc::try_unwrap(engine)
            .ok()
            .expect("one owner")
            .with_instance_id("x".to_string()),
    );
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    assert_eq!(line_hashes(&engine).await.len(), 5);
    engine.delete_engram(&delete_fridays()).await.unwrap();
    engine.embed_pending().await.unwrap();
    engine.score_contradictions().await.unwrap();
    assert_eq!(line_hashes(&engine).await.len(), 3, "pruned with an id set");
    set(&engine, "evolve.contradictions", "off").await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    assert!(
        line_hashes(&engine).await.is_empty(),
        "cleared with an id set"
    );
    assert!(rows(&engine, full().repo).await.is_empty());
}

/// Review focus 5.
#[tokio::test]
async fn a_setting_turned_off_while_the_daemon_was_down_clears_on_the_first_pass() {
    let (tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    // A second engine on the same store, as a restart with the setting off.
    let mut cfg = engine.config();
    cfg.evolve = Some(EvolveConfig {
        contradictions: Some("off".to_string()),
    });
    let restarted = Engine::new(
        engine.store(),
        cfg,
        Some(Arc::new(crate::support::TopicEmbedder)),
        Some(tmp.path().join("config.yaml")),
    );
    assert_eq!(
        restarted.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    assert!(line_hashes(&restarted).await.is_empty());
    assert!(rows(&restarted, full().repo).await.is_empty());
}

/// The same for a file domain on a restarted daemon: its first off pass runs
/// before the startup sync claims any file domain, so the instance's scope
/// holds none yet. On an unshared store the clear is the whole table's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_off_restart_clears_a_file_domain_before_any_claim() {
    let (tmp, _dir, engine) = file_engine(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine.score_contradictions().await.unwrap();
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
    assert!(!line_hashes(&engine).await.is_empty());
    let mut cfg = engine.config();
    cfg.evolve = Some(EvolveConfig {
        contradictions: Some("off".to_string()),
    });
    let restarted = Engine::new(
        engine.store(),
        cfg,
        Some(Arc::new(crate::support::TopicEmbedder)),
        Some(tmp.path().join("config.yaml")),
    )
    .with_instance_id("x".to_string());
    assert_eq!(
        restarted.score_contradictions().await.unwrap(),
        ContradictionOutcome::Off
    );
    assert!(line_hashes(&restarted).await.is_empty());
    assert!(rows(&restarted, full().repo).await.is_empty());
}

/// Review focus 4, the engine half.
#[tokio::test]
async fn an_embedding_model_without_a_floor_runs_no_v302_and_says_why() {
    let loads = Arc::new(AtomicUsize::new(0));
    let (_tmp, engine) = engine_without_floor(loads.clone()).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::NoLineFloor
    );
    assert_eq!(
        loads.load(Ordering::SeqCst),
        0,
        "no model is loaded for nothing"
    );
    assert!(!engine.contradictions_wanted(), "the tick never asks");
    assert!(
        hashes_of(&engine, "text-embedding-3-small")
            .await
            .is_empty(),
        "nothing is embedded for a model without a floor"
    );
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["line_floor_missing"], true);
    assert!(status["line_floor"].is_null());
    assert_eq!(status["embedding_model"], "text-embedding-3-small");
}

#[tokio::test]
async fn vectors_of_lines_nobody_uses_are_pruned_after_a_full_walk() {
    let (_tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    assert_eq!(line_hashes(&engine).await.len(), 5);
    engine.delete_engram(&delete_fridays()).await.unwrap();
    engine.embed_pending().await.unwrap();
    engine.score_contradictions().await.unwrap();
    assert_eq!(
        line_hashes(&engine).await.len(),
        3,
        "Fridays' two lines are used by no current engram"
    );
}

/// Review fix 1: pruning in steady state. A walk that skips a settled domain
/// still knows that domain's lines in use from its settled record, so a
/// delete in another domain prunes with no setting change and no restart,
/// and the skipped domain keeps its vectors.
#[tokio::test]
async fn a_walk_that_skips_a_settled_domain_still_prunes_another() {
    let (_tmp, engine) = engine_with_domains(
        Arc::new(crate::support::TopicEmbedder),
        &["notes", "harbour"],
    )
    .await;
    let engine = with_loader(engine, loader(stub(), Arc::new(AtomicUsize::new(0))));
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine
        .write_engram(&WriteParams {
            domain: "harbour".to_string(),
            ..write(
                "Ferry",
                "The harbour ferry timetable.\n\n- [fact] The ferry leaves at noon\n- [fact] The harbour closes at dusk",
            )
        })
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    engine.score_contradictions().await.unwrap();
    let all = line_hashes(&engine).await;
    assert_eq!(all.len(), 7, "five lines in notes, two in harbour");
    // Both domains settled; the next walk parses nothing.
    let walks = engine.contradiction_fact_walks();
    engine.score_contradictions().await.unwrap();
    assert_eq!(engine.contradiction_fact_walks(), walks);

    engine.delete_engram(&delete_fridays()).await.unwrap();
    engine.embed_pending().await.unwrap();
    engine.score_contradictions().await.unwrap();
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks + 1,
        "notes is parsed again, harbour is skipped as settled"
    );
    let left = line_hashes(&engine).await;
    assert_eq!(left.len(), 5, "Fridays' two lines are pruned");
    let harbour = [
        crystalline_index::nli::observation_hash("The ferry leaves at noon"),
        crystalline_index::nli::observation_hash("The harbour closes at dusk"),
    ];
    for h in &harbour {
        assert!(left.contains(h), "the skipped domain keeps its vectors");
    }
}

/// Embeds like the topic provider, and refuses every batch while `down`,
/// chunks and observation lines alike: a provider that is fully down.
struct DownTopicEmbedder {
    down: AtomicBool,
}

#[async_trait::async_trait]
impl EmbeddingProvider for DownTopicEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        if self.down.load(Ordering::SeqCst) {
            return Err(IndexError::Embedding("the provider is down".to_string()));
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

/// Review fix 2, lessons 37 and 62: with the provider fully down a domain
/// can settle neither on its lead vectors nor on its lines, and status still
/// says a candidate waits for its vector instead of reading as checked.
#[tokio::test]
async fn a_provider_that_is_fully_down_keeps_embedding_pending() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let provider = Arc::new(DownTopicEmbedder {
        down: AtomicBool::new(false),
    });
    let (_tmp, engine) = engine_on(provider.clone()).await;
    let engine = with_loader(engine, loader(s, loads));
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Eighteen", NODE_18))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    provider.down.store(true, Ordering::SeqCst);
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
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["pending_pairs"], 0);
    assert_eq!(
        status["embedding_pending"], true,
        "0 pending while a candidate still lacks a lead vector: {status}"
    );
    assert!(
        status["lines_embedded"].as_u64() < status["lines_eligible"].as_u64(),
        "{status}"
    );
    assert!(
        status["last_error"]
            .as_str()
            .is_some_and(|e| e.contains("observation lines could not be embedded")),
        "{status}"
    );
    // Up again: the lines and the lead vector arrive and the pair is scored.
    provider.down.store(false, Ordering::SeqCst);
    engine.embed_pending().await.unwrap();
    // One kept line pair, Node 18 against Node 20.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["embedding_pending"], false, "{status}");
    assert!(status["last_error"].is_null(), "{status}");
}

/// Preflight M12: a changed embedding model means a restart, and the first
/// full walk under the new one drops every line vector of the old one.
#[tokio::test]
async fn a_model_switch_drops_the_old_models_line_vectors() {
    let (_tmp, engine) = engine_with(loader(stub(), Arc::new(AtomicUsize::new(0)))).await;
    {
        let store = engine.store();
        let store = store.lock().await;
        store
            .store_observation_vectors(
                "bge-small-en-v1.5",
                &[ObservationVector {
                    hash: "a".repeat(64),
                    vector: vec![1.0, 0.0, 0.0, 0.0],
                }],
            )
            .await
            .unwrap();
    }
    assert_eq!(hashes_of(&engine, "bge-small-en-v1.5").await.len(), 1);
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    assert!(hashes_of(&engine, "bge-small-en-v1.5").await.is_empty());
    assert_eq!(line_hashes(&engine).await.len(), 5);
}

#[tokio::test]
async fn a_domain_with_a_missing_line_vector_never_settles() {
    let (_tmp, engine) = engine_on(Arc::new(PoisonLine)).await;
    let engine = with_loader(engine, loader(stub(), Arc::new(AtomicUsize::new(0))));
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine
        .write_engram(&write("Poisoned", POISONED))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    engine.score_contradictions().await.unwrap();
    let walks = engine.contradiction_fact_walks();
    engine.score_contradictions().await.unwrap();
    assert_eq!(
        engine.contradiction_fact_walks(),
        walks + 1,
        "a missing line vector keeps the domain from settling, so it is parsed again"
    );
    let status = engine.contradictions_status().await.unwrap();
    assert!(
        status["lines_embedded"].as_u64() < status["lines_eligible"].as_u64(),
        "{status}"
    );
    assert!(
        status["last_error"]
            .as_str()
            .is_some_and(|e| e.contains("observation lines could not be embedded")),
        "{status}"
    );
}

// --- scoring through the line filter ---------------------------------------

// The bodies carry six retry-axis words, so both lead vectors sit on the
// retry axis (lead cosine about 0.93 under the topic provider) whatever the
// bullets add.
const STAGING_MANUAL: &str = "The retry queue pipeline retries with backoff and a dead-letter queue.\n\n- [fact] Staging needs a manual trigger on the queue\n- [fact] Docking clamps seat in bay three\n- [fact] The login session cookie lasts a day";
const STAGING_AUTO: &str = "The retry queue pipeline retries with backoff and a dead-letter queue.\n\n- [fact] Staging retries the queue without a manual trigger\n- [fact] Token auth uses a csrf cookie";

/// Only line pairs about the same subject reach the model: under the topic
/// provider, a retry-queue line against a retry-queue line, a login line
/// against a login line, never the docking line against either.
#[tokio::test]
async fn only_lines_about_the_same_subject_reach_the_model() {
    let s = Arc::new(StubScorer::new(full().repo, 0.05).with(
        "Staging needs a manual trigger on the queue",
        "Staging retries the queue without a manual trigger",
        0.92,
    ));
    let (_tmp, engine) = engine_with(loader(s.clone(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Manual", STAGING_MANUAL))
        .await
        .unwrap();
    engine
        .write_engram(&write("Auto", STAGING_AUTO))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 2, 0)
    );
    // Two kept line pairs read once each, the staging pair (0.92) a second
    // time; the cookie pair (0.05) never in the other order.
    assert_eq!(s.forwards(), 3);
    let stored = rows(&engine, full().repo).await;
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert!((stored[0].similarity - 1.0).abs() < 1e-6, "same topic axis");
}

#[tokio::test]
async fn a_pair_with_no_line_above_the_floor_is_scored_empty_and_not_asked_again() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s.clone(), loads.clone())).await;
    set(&engine, "evolve.contradictions", "full").await;
    // Same lead topic (seven retry-axis words in each body, lead cosine about
    // 0.86), no line on a shared axis: the queue line against a docking line
    // and a login line.
    let body = "The retry queue backoff, retries and dead-letter queue ttl.";
    engine
        .write_engram(&write(
            "Queue",
            &format!("{body}\n\n- [fact] Retries back off on the queue"),
        ))
        .await
        .unwrap();
    engine
        .write_engram(&write(
            "Elsewhere",
            &format!(
                "{body}\n\n- [fact] Docking clamps seat in bay three\n- [fact] The login session cookie lasts a day"
            ),
        ))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 0, 0)
    );
    assert_eq!(s.forwards(), 0, "nothing reached the model");
    assert_eq!(
        loads.load(Ordering::SeqCst),
        0,
        "a pair with nothing to read loads no model"
    );
    assert!(rows(&engine, full().repo).await.is_empty());
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert_eq!(loads.load(Ordering::SeqCst), 0);
}

/// Review focus 3.
#[tokio::test]
async fn a_line_the_provider_rejects_keeps_its_pair_pending_and_says_why() {
    let (_tmp, engine) = engine_on(Arc::new(PoisonLine)).await;
    let engine = with_loader(engine, loader(stub(), Arc::new(AtomicUsize::new(0))));
    set(&engine, "evolve.contradictions", "full").await;
    engine
        .write_engram(&write("Eighteen", NODE_18))
        .await
        .unwrap();
    engine
        .write_engram(&write("Poisoned", POISONED))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    // The refused batch held every line of both engrams, so the pair waits.
    // pairs 0 with one remaining: the worker does not ask again at once
    // (run_contradiction_worker re-asks only after a pass that scored).
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 1),
        "the pair waits for its line vectors"
    );
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["pending_pairs"], 1);
    assert!(
        status["last_error"]
            .as_str()
            .unwrap()
            .contains("observation lines could not be embedded"),
        "{status}"
    );
    assert!(
        engine.contradictions_wanted(),
        "the tick retries it, every five minutes"
    );
}

#[tokio::test]
async fn rows_from_older_rules_are_rescored() {
    let s = stub();
    let (_tmp, engine) = engine_with(loader(s.clone(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    engine.score_contradictions().await.unwrap();
    // Stamp the pair as scored under other line rules, with no rows.
    {
        let store = engine.store();
        let store = store.lock().await;
        let domain = store.list_engrams("notes", None, None).await.unwrap()[0].domain_id;
        let scored = store
            .contradiction_pairs_scored(domain, full().repo)
            .await
            .unwrap();
        // The only pair is Eighteen (lower id, written first) against Twenty.
        // Its stored checksum is Eighteen's lines under today's rules; store
        // it again as a build with another floor would have.
        let lines = [
            FactObservation {
                line: 0,
                text: "The build uses Node 18".to_string(),
            },
            FactObservation {
                line: 0,
                text: "Retries back off on the queue".to_string(),
            },
        ];
        let rules = LineRules {
            embedding_model: DEFAULT_MODEL_ID,
            floor: line_similarity_floor(DEFAULT_MODEL_ID).unwrap(),
            max_line_pairs: MAX_LINE_PAIRS_PER_ENGRAM_PAIR,
        };
        assert_eq!(scored[0].checksum_a, scoring_checksum(&lines, &rules));
        let mut old = scored[0].clone();
        old.checksum_a = scoring_checksum(
            &lines,
            &LineRules {
                floor: rules.floor + 0.02,
                ..rules
            },
        );
        store
            .replace_contradictions(domain, &old, 0.9, full().repo, "t", &[])
            .await
            .unwrap();
    }
    set(&engine, "evolve.contradictions", "full").await;
    let before = s.forwards();
    // One kept line pair: Node 18 against Node 20 (both on the unmarked
    // axis); the shared retry bullet is the same text on both sides.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert!(s.forwards() > before, "the pair was read again");
    assert_eq!(rows(&engine, full().repo).await.len(), 1);
}

/// A walk that only stores pairs with no line pair to read never reads the
/// model, so it must not keep a loaded model in memory past its idle time.
#[tokio::test(start_paused = true)]
async fn pairs_stored_empty_do_not_keep_the_model_from_idling_out() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) =
        engine_with_domains(Arc::new(crate::support::TopicEmbedder), &["notes", "other"]).await;
    let engine = with_loader(engine, loader(s.clone(), loads.clone()));
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    // One kept line pair, Node 18 against Node 20: the model is read.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert!(engine.contradiction_scorer_loaded());
    let read = s.forwards();

    tokio::time::advance(NLI_IDLE_DROP / 2 + std::time::Duration::from_secs(60)).await;
    // In another domain, so no line meets a line of the first three: a
    // related pair with no line on a shared axis, stored empty.
    let body = "The retry queue backoff, retries and dead-letter queue ttl.";
    for (title, bullets) in [
        ("Queue", "\n\n- [fact] Retries back off on the queue"),
        (
            "Elsewhere",
            "\n\n- [fact] Docking clamps seat in bay three\n- [fact] The login session cookie lasts a day",
        ),
    ] {
        engine
            .write_engram(&WriteParams {
                domain: "other".to_string(),
                ..write(title, &format!("{body}{bullets}"))
            })
            .await
            .unwrap();
    }
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 0, 0)
    );
    assert_eq!(s.forwards(), read, "the empty pair never reached the model");

    // Ten minutes and a minute since the model was last read, five and a
    // bit since the walk that stored the empty pair.
    tokio::time::advance(NLI_IDLE_DROP / 2).await;
    engine.drop_idle_scorer();
    assert!(
        !engine.contradiction_scorer_loaded(),
        "a walk that read nothing does not keep the model"
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

/// Both reading orders land on the right line pair of the right engram pair
/// when two pairs share a group: four distinct directional scores, so a
/// swapped order or a second-order score put on the other pair fails.
#[tokio::test]
async fn both_reading_orders_land_on_their_own_pair_in_a_shared_group() {
    let (n18, n19, n20) = (
        "The build uses Node 18",
        "The build breaks on Node 19",
        "The build uses Node 20",
    );
    let s = Arc::new(
        StubScorer::new(full().repo, 0.05)
            .with_directional(n18, n20, 0.91)
            .with_directional(n20, n18, 0.81)
            .with_directional(n18, n19, 0.71)
            .with_directional(n19, n18, 0.61),
    );
    let (_tmp, engine) = engine_with(loader(s.clone(), Arc::new(AtomicUsize::new(0)))).await;
    set(&engine, "evolve.contradictions", "full").await;
    // Written in this order, so Eighteen has the lowest id and is the `a`
    // side (first-order premise) of both pairs it is in.
    for (title, content) in [
        ("Eighteen", NODE_18),
        ("Twenty", NODE_20),
        ("Nineteen", NODE_19),
    ] {
        engine.write_engram(&write(title, content)).await.unwrap();
    }
    engine.embed_pending().await.unwrap();
    // Three pairs of one kept line pair each (the Node lines), one group.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(3, 3, 0)
    );
    // Three first orders, then the two at or above 0.5 read back.
    assert_eq!(s.forwards(), 5);
    let mut got: Vec<(String, String, f32, f32, usize)> = rows(&engine, full().repo)
        .await
        .into_iter()
        .map(|r| {
            assert_eq!(r.line_a, r.line_b, "both Node lines are the first bullet");
            (r.hash_a, r.hash_b, r.score_ab, r.score_ba, r.line_a)
        })
        .collect();
    got.sort_by(|x, y| y.2.total_cmp(&x.2));
    let line = got[0].4;
    assert_eq!(
        got,
        vec![
            (
                observation_hash(n18),
                observation_hash(n20),
                0.91,
                0.81,
                line
            ),
            (
                observation_hash(n18),
                observation_hash(n19),
                0.71,
                0.61,
                line
            ),
        ]
    );
}

// --- an engram past its valid_to takes no part (Task 5b) --------------------

fn day(s: &str) -> chrono::NaiveDate {
    s.parse().unwrap()
}

/// Close `permalink`'s validity window at `date`.
async fn valid_to(engine: &Engine, permalink: &str, date: &str) {
    engine
        .edit_engram(&EditParams {
            identifier: permalink.to_string(),
            domain: "notes".to_string(),
            operation: "set_frontmatter".to_string(),
            key: Some("valid_to".to_string()),
            value: Some(date.to_string()),
            ..EditParams::default()
        })
        .await
        .unwrap();
}

/// The `V302` queue as of `today`, the same date the walk reads.
async fn sweep_on(engine: &Engine, today: &str) -> serde_json::Value {
    engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["notes".to_string()],
                families: vec!["meaning".to_string()],
                limit: Some(50),
                today: Some(today.to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// An engram whose `valid_to` is before today is never paired and its lines
/// are not embedded for `V302`; one whose `valid_to` is today still counts.
#[tokio::test]
async fn an_engram_past_its_valid_to_is_never_paired_and_one_ending_today_is() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    engine.set_contradiction_today(day("2026-10-01"));
    set(&engine, "evolve.contradictions", "full").await;
    for (title, content) in [
        ("Eighteen", NODE_18),
        ("Nineteen", NODE_19),
        ("Twenty", NODE_20),
    ] {
        engine.write_engram(&write(title, content)).await.unwrap();
    }
    valid_to(&engine, "eighteen", "2026-09-30").await;
    valid_to(&engine, "nineteen", "2026-10-01").await;
    engine.embed_pending().await.unwrap();
    // Nineteen against Twenty only, one kept line pair (the Node lines).
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    let held = line_hashes(&engine).await;
    assert!(
        !held.contains(&observation_hash("The build uses Node 18")),
        "an expired engram's lines are not embedded"
    );
    assert!(held.contains(&observation_hash("The build breaks on Node 19")));
    assert!(held.contains(&observation_hash("The build uses Node 20")));
}

/// Nothing is written when an engram expires: the next day's walk drops it,
/// its rows go, the queue stops showing it even before that walk, and its
/// line vectors are pruned.
#[tokio::test]
async fn an_engram_that_expires_between_walks_leaves_the_queue_without_an_edit() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    engine.set_contradiction_today(day("2026-10-01"));
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    valid_to(&engine, "eighteen", "2026-10-01").await;
    engine.embed_pending().await.unwrap();
    // Eighteen against Twenty, the 0.93 Node pair.
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    assert_eq!(v302(&sweep_on(&engine, "2026-10-01").await).len(), 1);
    assert!(!engine.contradictions_wanted(), "drained for the day");

    // A day on: Eighteen's window closed yesterday.
    engine.set_contradiction_today(day("2026-10-02"));
    assert!(
        engine.contradictions_wanted(),
        "a new day asks for a walk, with no write"
    );
    let before = sweep_on(&engine, "2026-10-02").await;
    assert!(
        v302(&before).is_empty(),
        "the sweep reads the date itself, before any walk: {before}"
    );
    assert_eq!(rows(&engine, full().repo).await.len(), 1, "not deleted yet");

    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(0, 0, 0)
    );
    assert!(rows(&engine, full().repo).await.is_empty(), "rows gone");
    {
        let store = engine.store();
        let store = store.lock().await;
        let domain = store.list_engrams("notes", None, None).await.unwrap()[0].domain_id;
        assert!(
            store
                .contradiction_pairs_scored(domain, full().repo)
                .await
                .unwrap()
                .is_empty(),
            "the pair row went too, so a reopened window scores it again"
        );
    }
    assert!(
        !line_hashes(&engine)
            .await
            .contains(&observation_hash("The build uses Node 18")),
        "its own line is no longer in use and is pruned"
    );
    assert!(!engine.contradictions_wanted());
    let after = sweep_on(&engine, "2026-10-02").await;
    assert!(v302(&after).is_empty(), "{after}");
    assert!(
        v302_truncations(&after).is_empty(),
        "counted for the new day: {after}"
    );
}

/// The walk reads the same date the sweep does by default (UTC): with no
/// date set, an engram whose window ends on the UTC date is scored by the
/// walk, shown by a sweep that names no date, and the domain reads as
/// counted, so the walk's digest and the sweep's agree.
#[tokio::test]
async fn the_walk_and_a_default_sweep_read_the_same_utc_date() {
    let (s, loads) = (stub(), Arc::new(AtomicUsize::new(0)));
    let (_tmp, engine) = engine_with(loader(s, loads)).await;
    set(&engine, "evolve.contradictions", "full").await;
    three(&engine).await;
    let today = chrono::Utc::now().date_naive().to_string();
    valid_to(&engine, "eighteen", &today).await;
    engine.embed_pending().await.unwrap();
    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        scored(1, 1, 0)
    );
    let value = engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["notes".to_string()],
                families: vec!["meaning".to_string()],
                limit: Some(50),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(v302(&value).len(), 1, "{value}");
    assert!(
        v302_truncations(&value).is_empty(),
        "counted under the same date: {value}"
    );
    assert!(!engine.contradictions_wanted());
}
