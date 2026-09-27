//! The contradiction check end to end on the engine: the profile setting,
//! the model cache keep list, the daemon pass with the stub scorer, pending,
//! the sweep's V302 read and the pair-of-lines acknowledgment.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_index::nli::{NLI_MODELS, NliProfile, nli_model};
use crystalline_service::Engine;
use crystalline_service::engine::ConfigureAction;
use tokio::sync::Mutex;

/// One virtual domain `notes` on an engine with the topic provider, so two
/// engrams about the retry queue share a lead vector and a docking note is
/// orthogonal to both.
async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
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
    let provider: Arc<dyn crystalline_index::EmbeddingProvider> =
        Arc::new(crate::support::TopicEmbedder);
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
