//! The first start after the default embedding model changes. One store,
//! embedded under the model an install used to have, then served by an engine
//! whose config names no model at all, so its active model is the new default.
//! Nothing here is new behaviour: each of the three is something the code
//! already does per model, and these are the pins that say so.

use std::sync::Arc;

use crystalline_core::config::{
    DomainEntry, EmbeddingsConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::{Store, TursoStore};
use crystalline_service::params::{EvolveParams, SearchParams, WriteParams};
use crystalline_service::{Engine, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

const OLD_MODEL: &str = "bge-small-en-v1.5";

const A: &str = "The retry queue doubles its backoff on every failure and a dead-letter ttl bounds how long a retry waits.\nRaising the ttl fixed the stuck retries last time.\nA retry storm needs a wider backoff on the queue.";
const PARAPHRASE: &str = "Each failed attempt makes the queue back off twice as long, while the dead-letter ttl is the ceiling on any single retry.\nWhen retries stalled before, a bigger ttl cleared them.\nBursty retry traffic wants more generous backoff across the queue.";

fn base_config() -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    cfg
}

/// An engine pinned to the old model, over `store`.
fn old_engine(store: Arc<Mutex<dyn Store>>) -> Arc<Engine> {
    let mut cfg = base_config();
    cfg.embeddings = Some(EmbeddingsConfig {
        provider: "local".to_string(),
        model: OLD_MODEL.to_string(),
        endpoint: None,
        api_key_env: None,
    });
    Arc::new(Engine::new(
        store,
        cfg,
        Some(Arc::new(crate::support::TopicEmbedder)),
        None,
    ))
}

/// An engine with no model configured, over `store`: the upgraded install.
fn upgraded_engine(store: Arc<Mutex<dyn Store>>) -> Arc<Engine> {
    let mut cfg = base_config();
    cfg.embeddings = None;
    Arc::new(Engine::new(
        store,
        cfg,
        Some(Arc::new(crate::support::TopicEmbedder)),
        None,
    ))
}

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
        // Both added since this plan was written (params.rs:75-89); the struct
        // has no Default, so a literal must name them.
        model: None,
        share_link: None,
    }
}

fn search(query: &str, mode: &str) -> SearchParams {
    SearchParams {
        query: Some(query.to_string()),
        domains: Vec::new(),
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata_filters: None,
        after: None,
        search_type: Some(mode.to_string()),
        min_similarity: None,
        limit: Some(10),
        page: Some(1),
    }
}

/// A store with two engrams embedded under the old model, and the engine that
/// did it (kept so a test can compare the before and the after).
async fn embedded_under_the_old_model() -> (Arc<Mutex<dyn Store>>, Arc<Engine>) {
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let old = old_engine(store.clone());
    old.write_engram(&write("Retry queue gotcha", A))
        .await
        .unwrap();
    old.write_engram(&write("Backoff lesson", PARAPHRASE))
        .await
        .unwrap();
    old.embed_pending().await.unwrap();
    {
        let s = store.lock().await;
        let coverage = s.embedding_coverage().await.unwrap();
        assert_eq!(coverage.embedded_for(OLD_MODEL), coverage.total_chunks);
        assert!(coverage.total_chunks > 0);
    }
    (store, old)
}

#[tokio::test]
async fn the_first_start_after_the_flip_answers_in_text_mode() {
    let (store, old) = embedded_under_the_old_model().await;
    let before = old
        .search_engrams(&search("retry backoff", "hybrid"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        before["mode"], "hybrid",
        "the old install searched with its vectors"
    );

    let upgraded = upgraded_engine(store);
    assert_eq!(
        upgraded.model_id(),
        crystalline_index::embed::DEFAULT_MODEL_ID
    );
    let after = upgraded
        .search_engrams(&search("retry backoff", "hybrid"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        after["mode"], "text",
        "with no vectors for the active model, search falls back to text rather than failing: {after}"
    );
    assert!(
        // The search envelope's results key is `hits`
        // (`crates/engine/src/engine/search.rs`), not `results`.
        after["hits"].as_array().is_some_and(|r| !r.is_empty()),
        "and still answers: {after}"
    );
}

#[tokio::test]
async fn the_first_start_after_the_flip_has_every_chunk_in_its_backlog() {
    let (store, _old) = embedded_under_the_old_model().await;
    let total = {
        let s = store.lock().await;
        s.embedding_coverage().await.unwrap().total_chunks
    };
    let upgraded = upgraded_engine(store.clone());
    assert_eq!(
        upgraded.embedding_backlog().await.unwrap(),
        total,
        "every chunk is outstanding for the new model"
    );

    // And one pass covers them, after which nothing of the old model is left.
    let embedded = upgraded.embed_pending().await.unwrap();
    assert_eq!(embedded, total);
    assert_eq!(upgraded.embedding_backlog().await.unwrap(), 0);
    let s = store.lock().await;
    let coverage = s.embedding_coverage().await.unwrap();
    assert_eq!(coverage.embedded_for(OLD_MODEL), 0, "{coverage:?}");
    assert_eq!(coverage.models.len(), 1, "{coverage:?}");
}

#[tokio::test]
async fn neighbours_and_v301_stay_silent_until_the_new_vectors_exist() {
    let (store, old) = embedded_under_the_old_model().await;
    // Before: the probe finds its neighbour and the sweep sees the twin.
    assert!(
        !old.similar_engrams(A, None, &Scope::Unrestricted)
            .await
            .unwrap()
            .is_empty(),
        "the old install had neighbours"
    );
    assert!(twin_rules(&sweep(&old).await).contains(&"V301".to_string()));

    let upgraded = upgraded_engine(store);
    assert!(
        upgraded
            .similar_engrams(A, None, &Scope::Unrestricted)
            .await
            .unwrap()
            .is_empty(),
        "a receipt names no neighbours while the active model has no vectors"
    );
    assert!(
        !twin_rules(&sweep(&upgraded).await).contains(&"V301".to_string()),
        "and the sweep emits no semantic twin"
    );
}

async fn sweep(engine: &Engine) -> Value {
    engine
        .evolve_engrams(
            &EvolveParams {
                domains: vec!["notes".to_string()],
                families: vec!["redundancy".to_string()],
                rules: Vec::new(),
                min_priority: None,
                limit: Some(50),
                page: None,
                today: Some("2026-09-14".to_string()),
                include_acknowledged: false,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

fn twin_rules(value: &Value) -> Vec<String> {
    value["queue"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|f| f["rule"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A model cache holding three models' weights, one of them the active one.
/// The third is not in `LOCAL_MODELS`: another tool's weights sharing the
/// same `CRYSTALLINE_MODELS_DIR`, which the prune must never touch.
fn seeded_cache(tmp: &std::path::Path) -> std::path::PathBuf {
    let cache = tmp.join("models");
    for repo in [
        "ibm-granite/granite-embedding-97m-multilingual-r2",
        "BAAI/bge-small-en-v1.5",
        "sentence-transformers/all-MiniLM-L6-v2",
    ] {
        let dir = cache.join(crystalline_index::hub_dir_name(repo));
        std::fs::create_dir_all(dir.join("blobs")).unwrap();
        std::fs::write(dir.join("blobs/weights"), [0u8; 128]).unwrap();
    }
    cache
}

fn cached_repos(cache: &std::path::Path) -> Vec<String> {
    crystalline_index::cached_model_dirs(cache)
        .into_iter()
        .map(|(repo, _)| repo)
        .collect()
}

/// An engine over a fresh store, its config as given.
async fn engine_over_a_fresh_store(cfg: GlobalConfig, read_only: bool) -> Arc<Engine> {
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    Arc::new(Engine::new(store, cfg, None, None).with_read_only(read_only))
}

/// The weights half of the upgrade: once the active model has loaded, the
/// cache keeps that model and nothing else, and the start says what it freed.
#[tokio::test]
async fn a_successful_load_prunes_the_models_the_config_no_longer_names() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = seeded_cache(tmp.path());

    // No embeddings block at all, which is the local provider on the default
    // model: the shape of the install this whole wave is about.
    let engine = engine_over_a_fresh_store(base_config(), false).await;
    engine.prune_model_cache(cache.clone()).await;
    assert_eq!(
        cached_repos(&cache),
        vec![
            "ibm-granite/granite-embedding-97m-multilingual-r2".to_string(),
            "sentence-transformers/all-MiniLM-L6-v2".to_string(),
        ],
        "the active model's weights survive, and so does a directory the \
         table does not know - only a table-known, not-kept model is pruned"
    );

    let report = engine.status_report().await.unwrap();
    let pruned = &report["embeddings"]["pruned_model_cache"];
    assert_eq!(
        pruned.as_array().map(Vec::len),
        Some(1),
        "only bge is both table-known and not kept: {report}"
    );
    assert_eq!(pruned[0]["repo"], "BAAI/bge-small-en-v1.5");
    assert!(pruned[0]["bytes"].as_u64().unwrap() >= 128);

    // An install that pruned nothing reports nothing, so its status output is
    // byte-identical to what it printed before this shipped.
    let quiet = engine_over_a_fresh_store(base_config(), false).await;
    let report = quiet.status_report().await.unwrap();
    assert!(
        report["embeddings"].get("pruned_model_cache").is_none(),
        "{report}"
    );
}

/// A read-only instance owns nothing on that disk it should be deleting: it
/// serves an index and a model cache somebody else's install may be the one
/// maintaining, so it leaves both alone.
#[tokio::test]
async fn a_read_only_instance_never_prunes_the_model_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = seeded_cache(tmp.path());
    let before = cached_repos(&cache);

    let engine = engine_over_a_fresh_store(base_config(), true).await;
    engine.prune_model_cache(cache.clone()).await;

    assert_eq!(
        cached_repos(&cache),
        before,
        "every model's weights are kept"
    );
    let report = engine.status_report().await.unwrap();
    assert!(
        report["embeddings"].get("pruned_model_cache").is_none(),
        "and nothing is reported: {report}"
    );
}

/// An endpoint may serve one of the table's models under its repository id.
/// That string says nothing about the weights on this disk, so a remote
/// provider never deletes any of them.
#[tokio::test]
async fn a_remote_provider_never_prunes_the_model_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = seeded_cache(tmp.path());
    let before = cached_repos(&cache);

    let mut cfg = base_config();
    cfg.embeddings = Some(EmbeddingsConfig {
        provider: "openai-compatible".to_string(),
        model: "BAAI/bge-small-en-v1.5".to_string(),
        endpoint: Some("https://example.invalid/v1".to_string()),
        api_key_env: None,
    });
    let engine = engine_over_a_fresh_store(cfg, false).await;
    engine.prune_model_cache(cache.clone()).await;

    assert_eq!(
        cached_repos(&cache),
        before,
        "every model's weights are kept"
    );
    let report = engine.status_report().await.unwrap();
    assert!(
        report["embeddings"].get("pruned_model_cache").is_none(),
        "and nothing is reported: {report}"
    );
}
