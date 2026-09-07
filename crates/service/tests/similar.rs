//! The neighbours advisory at the engine: what a probe finds, what it never
//! finds, and how it fails.

mod support;

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::engine::ConfigureAction;
use crystalline_service::params::WriteParams;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, SIMILAR_GUIDANCE, Scope, SimilarProbe};
use serde_json::json;
use tokio::sync::Mutex;

const RETRY: &str = "The retry queue doubles its backoff on every failure.\nA dead-letter ttl bounds how long a retry waits.\nRaising the ttl fixed the stuck retries last time.";
const RETRY_AGAIN: &str = "Retries wait on a backoff that doubles each time.\nThe dead-letter ttl is the bound on a stuck retry.\nWe raised the ttl and the queue drained.";
const DOCKING: &str = "Clamp three reads locked before it seats in the aft bay.\nWait for the green tone before cutting thrust on docking.\nThe clamps misread below eight degrees.";

/// Two virtual domains, `open` and `lab`, on one engine with the topic
/// provider installed. Virtual so nothing touches disk and every write goes
/// straight to the index.
async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
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
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        Some(Arc::new(support::TopicEmbedder)),
        Some(config_path),
    ));
    (tmp, engine)
}

fn write(domain: &str, title: &str, content: &str, status: Option<&str>) -> WriteParams {
    WriteParams {
        domain: domain.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: vec!["t".to_string()],
        status: status.map(str::to_string),
        metadata: None,
        overwrite: false,
    }
}

fn permalinks(similar: &[crystalline_service::SimilarEngram]) -> Vec<String> {
    similar
        .iter()
        .map(|s| format!("{}/{}", s.domain, s.permalink))
        .collect()
}

fn user(account: &str) -> Scope {
    Scope::User {
        account: account.into(),
        admin: false,
    }
}

#[tokio::test]
async fn neighbours_rank_by_meaning_and_exclude_self_and_retired() {
    let (_tmp, engine) = engine().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    engine
        .write_engram(&write("open", "Retry backoff lesson", RETRY_AGAIN, None))
        .await
        .unwrap();
    engine
        .write_engram(&write(
            "open",
            "Old retry note",
            RETRY_AGAIN,
            Some("superseded"),
        ))
        .await
        .unwrap();
    engine
        .write_engram(&write("open", "Docking clamps", DOCKING, None))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();

    let probe = format!("Retry queue gotcha\n{RETRY}");
    let similar = engine
        .similar_engrams(
            &probe,
            Some(("open", "retry-queue-gotcha")),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(permalinks(&similar), vec!["open/retry-backoff-lesson"]);
    assert_eq!(similar[0].status, "stable");
    assert_eq!(similar[0].engram_type, "engram");
}

#[tokio::test]
async fn a_hidden_domains_engram_never_reaches_a_stranger() {
    let (tmp, engine) = engine().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    engine
        .write_engram(&write("lab", "Retry secrets", RETRY_AGAIN, None))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["owner", "out"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("lab", true, "owner")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));

    let probe = format!("Retry queue gotcha\n{RETRY}");
    let stranger = engine
        .similar_engrams(&probe, Some(("open", "retry-queue-gotcha")), &user("out"))
        .await
        .unwrap();
    assert!(permalinks(&stranger).is_empty(), "{stranger:?}");
    let owner = engine
        .similar_engrams(&probe, Some(("open", "retry-queue-gotcha")), &user("owner"))
        .await
        .unwrap();
    assert_eq!(permalinks(&owner), vec!["lab/retry-secrets"]);
    let machine = engine
        .similar_engrams(
            &probe,
            Some(("open", "retry-queue-gotcha")),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(permalinks(&machine), vec!["lab/retry-secrets"]);
}

#[tokio::test]
async fn no_provider_and_no_embeddings_both_mean_no_neighbours() {
    let (_tmp, engine) = engine().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    // Chunked but never embedded: the mode degrades to text, and text is
    // never probed.
    let probe = format!("Retry queue gotcha\n{RETRY}");
    assert!(
        engine
            .similar_engrams(&probe, None, &Scope::Unrestricted)
            .await
            .unwrap()
            .is_empty()
    );
    let mut receipt = json!({ "domain": "open", "permalink": "retry-queue-gotcha" });
    engine
        .attach_similar(
            &mut receipt,
            SimilarProbe::Write {
                title: "Retry queue gotcha",
                description: None,
                body: RETRY,
            },
            &Scope::Unrestricted,
        )
        .await;
    assert!(receipt.get("similar").is_none());
}

#[tokio::test]
async fn attach_similar_honours_the_setting_and_the_floor() {
    let (_tmp, engine) = engine().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    engine
        .write_engram(&write("open", "Retry backoff lesson", RETRY_AGAIN, None))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    let probe = SimilarProbe::Write {
        title: "Retry queue gotcha",
        description: None,
        body: RETRY,
    };

    let mut receipt = json!({ "domain": "open", "permalink": "retry-queue-gotcha" });
    engine
        .attach_similar(
            &mut receipt,
            SimilarProbe::Write {
                title: "Retry queue gotcha",
                description: None,
                body: RETRY,
            },
            &Scope::Unrestricted,
        )
        .await;
    assert_eq!(receipt["similar"][0]["permalink"], "retry-backoff-lesson");
    assert_eq!(receipt["guidance"], SIMILAR_GUIDANCE);

    let mut short = json!({ "domain": "open", "permalink": "retry-queue-gotcha" });
    engine
        .attach_similar(
            &mut short,
            SimilarProbe::Edit {
                new_text: "- [fact] retry",
            },
            &Scope::Unrestricted,
        )
        .await;
    assert!(
        short.get("similar").is_none(),
        "under the floor nothing is probed"
    );

    engine
        .configure(&ConfigureAction::Set {
            key: "capture.similar".into(),
            value: "false".into(),
        })
        .await
        .unwrap();
    let mut off = json!({ "domain": "open", "permalink": "retry-queue-gotcha" });
    engine
        .attach_similar(&mut off, probe, &Scope::Unrestricted)
        .await;
    assert!(
        off.get("similar").is_none(),
        "the setting switches the advisory off"
    );
}

/// A content edit probes with the stored title plus the new text, and the
/// engram it edited is never its own neighbour.
#[tokio::test]
async fn an_edit_probes_with_the_stored_title_and_excludes_itself() {
    let (_tmp, engine) = engine().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    engine
        .write_engram(&write("open", "Retry backoff lesson", RETRY_AGAIN, None))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();

    let mut receipt = json!({ "domain": "open", "permalink": "retry-queue-gotcha" });
    engine
        .attach_similar(
            &mut receipt,
            SimilarProbe::Edit { new_text: RETRY },
            &Scope::Unrestricted,
        )
        .await;
    assert_eq!(receipt["similar"][0]["permalink"], "retry-backoff-lesson");
    assert_eq!(
        receipt["similar"].as_array().unwrap().len(),
        1,
        "the edited engram is not its own neighbour"
    );
}

#[tokio::test]
async fn a_slow_provider_is_cut_at_the_timeout() {
    let (_tmp, engine) = engine().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    engine
        .write_engram(&write("open", "Retry backoff lesson", RETRY_AGAIN, None))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    engine.set_provider(Arc::new(support::SleepyEmbedder {
        delay: Duration::from_secs(10),
    }));
    let mut receipt = json!({ "domain": "open", "permalink": "retry-queue-gotcha" });
    let started = std::time::Instant::now();
    engine
        .attach_similar(
            &mut receipt,
            SimilarProbe::Write {
                title: "Retry queue gotcha",
                description: None,
                body: RETRY,
            },
            &Scope::Unrestricted,
        )
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(receipt.get("similar").is_none());
    assert_eq!(
        receipt["permalink"], "retry-queue-gotcha",
        "the receipt itself is untouched"
    );
}
