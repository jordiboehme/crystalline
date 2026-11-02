//! Coverage decides the search mode on the domains a search names: a named
//! domain whose engrams still wait for vectors answers as text while another
//! domain is embedded, and a search that names none keeps the global decision.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::params::{SearchParams, WriteParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

const RETRY: &str = "The retry queue doubles its backoff on every failure.\nA dead-letter ttl bounds how long a retry waits.";

/// Two virtual domains, `open` and `lab`, with the topic provider installed
/// and no embed worker, so a write stays unembedded until `embed_pending`.
async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("open".to_string(), DomainEntry::virtual_domain());
    cfg.domains
        .insert("lab".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let engine = Engine::new(
        store,
        cfg,
        Some(Arc::new(crate::support::TopicEmbedder)),
        Some(config_path),
    );
    (tmp, Arc::new(engine))
}

fn write(domain: &str, title: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: domain.to_string(),
        title: title.to_string(),
        content: Some(RETRY.to_string()),
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

/// `open` holds a retry engram with vectors; `lab` holds one written after the
/// embed pass, so it waits.
async fn embedded_open_waiting_lab() -> (tempfile::TempDir, Arc<Engine>) {
    let (tmp, engine) = engine().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha"))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    engine
        .write_engram(&write("lab", "Retry lab note"))
        .await
        .unwrap();
    (tmp, engine)
}

async fn search(engine: &Engine, domains: &[&str], mode: &str) -> Value {
    engine
        .search_engrams(
            &SearchParams {
                query: Some("retry".to_string()),
                domains: domains.iter().map(|d| d.to_string()).collect(),
                search_type: Some(mode.to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn a_named_domain_without_vectors_answers_as_text_while_another_is_embedded() {
    let (_tmp, engine) = embedded_open_waiting_lab().await;
    for mode in ["semantic", "hybrid"] {
        let waiting = search(&engine, &["lab"], mode).await;
        assert_eq!(waiting["mode"], "text", "{mode}: {waiting}");
        assert_eq!(
            waiting["total"], 1,
            "{mode} finds the hit as text: {waiting}"
        );
        assert_eq!(waiting["hits"][0]["permalink"], "retry-lab-note");

        let embedded = search(&engine, &["open"], mode).await;
        assert_eq!(embedded["mode"], mode, "{embedded}");
        assert_eq!(embedded["total"], 1, "{embedded}");

        let both = search(&engine, &["open", "lab"], mode).await;
        assert_eq!(both["mode"], mode, "one embedded domain is enough: {both}");
    }
}

#[tokio::test]
async fn a_sweep_keeps_the_global_mode() {
    let (_tmp, engine) = embedded_open_waiting_lab().await;
    for mode in ["semantic", "hybrid"] {
        let swept = search(&engine, &[], mode).await;
        assert_eq!(swept["mode"], mode, "{swept}");
    }
}

/// A caller who cannot see every domain decides a sweep's mode on the domains
/// they can see: the hidden domain's vectors do not make a semantic sweep run
/// over visible engrams that have none.
#[tokio::test]
async fn a_sweep_decides_its_mode_on_the_domains_the_caller_can_see() {
    let (tmp, engine) = engine().await;
    engine
        .write_engram(&write("lab", "Retry secrets"))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    engine
        .write_engram(&write("open", "Retry queue gotcha"))
        .await
        .unwrap();
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "out"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));
    let user = |account: &str| Scope::User {
        account: account.into(),
        admin: false,
    };
    let params = |mode: &str| SearchParams {
        query: Some("retry".to_string()),
        search_type: Some(mode.to_string()),
        ..SearchParams::default()
    };
    for mode in ["semantic", "hybrid"] {
        let stranger = engine
            .search_engrams(&params(mode), &user("out"))
            .await
            .unwrap();
        assert_eq!(stranger["mode"], "text", "{mode}: {stranger}");
        assert_eq!(stranger["total"], 1, "{mode}: {stranger}");
        assert_eq!(stranger["hits"][0]["permalink"], "retry-queue-gotcha");
        // The owner sees the embedded domain too, so the sweep stays semantic.
        let owner = engine
            .search_engrams(&params(mode), &user("keeper"))
            .await
            .unwrap();
        assert_eq!(owner["mode"], mode, "{owner}");
    }
}
