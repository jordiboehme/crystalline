//! The neighbours advisory at the engine: what a probe finds, what it never
//! finds, and how it fails.

mod support;

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{
    DomainEntry, EmbeddingsConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::{EmbeddingProvider, TursoStore};
use crystalline_service::engine::ConfigureAction;
use crystalline_service::params::WriteParams;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::similar::{SIMILAR_BACKLOG_WAIT, SIMILAR_TIMEOUT};
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
    let (tmp, store) = fresh_store().await;
    let engine = build(
        &tmp,
        store,
        Some(Arc::new(support::TopicEmbedder)),
        None,
        None,
    );
    (tmp, engine)
}

/// [`engine`] with a live embed worker behind it: the channel is wired into
/// [`build`] and `run_embed_worker` is listening on the other end by the time
/// this returns. The pair a write's nudge needs to be observable at all - with
/// no channel `request_embed` is a no-op and there is nothing to watch drain.
async fn engine_with_worker() -> (tempfile::TempDir, Arc<Engine>) {
    let (tmp, store) = fresh_store().await;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = build(
        &tmp,
        store,
        Some(Arc::new(support::TopicEmbedder)),
        Some(tx),
        None,
    );
    tokio::spawn(crystalline_service::engine::run_embed_worker(
        engine.clone(),
        rx,
    ));
    (tmp, engine)
}

/// A fresh in-memory store and the temp directory its config lives in, kept
/// apart from [`build`] so two engines can be raised over the same store.
async fn fresh_store() -> (tempfile::TempDir, Arc<Mutex<TursoStore>>) {
    let tmp = tempfile::tempdir().unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    (tmp, Arc::new(Mutex::new(store)))
}

/// An engine over `store`, with or without a provider and with or without an
/// embed worker wired. `model` names the engine's active embedding model,
/// which is what an embedding is stored against and searched by; two engines
/// over one store with different models are how a stale index is staged.
fn build(
    tmp: &tempfile::TempDir,
    store: Arc<Mutex<TursoStore>>,
    provider: Option<Arc<dyn EmbeddingProvider>>,
    embed_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    model: Option<&str>,
) -> Arc<Engine> {
    let mut cfg = GlobalConfig::default();
    if let Some(model) = model {
        cfg.embeddings = Some(EmbeddingsConfig {
            provider: "local".to_string(),
            model: model.to_string(),
            endpoint: None,
            api_key_env: None,
        });
    }
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
    let engine = Engine::new(store, cfg, provider, Some(config_path));
    Arc::new(match embed_tx {
        Some(tx) => engine.with_embed_channel(tx),
        None => engine,
    })
}

/// The two retry engrams every test starts from: the one a receipt would be
/// about, and the one that is its neighbour.
async fn two_retry_engrams(engine: &Engine) {
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    engine
        .write_engram(&write("open", "Retry backoff lesson", RETRY_AGAIN, None))
        .await
        .unwrap();
}

/// The probe a receipt for `Retry queue gotcha` would carry.
fn retry_probe() -> SimilarProbe<'static> {
    SimilarProbe::Write {
        title: "Retry queue gotcha",
        description: None,
        body: RETRY,
    }
}

fn retry_receipt() -> serde_json::Value {
    json!({ "domain": "open", "permalink": "retry-queue-gotcha" })
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
async fn neighbours_are_the_near_ones_and_exclude_self_and_retired() {
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

/// The two guards that make the probe answer nothing, each with a neighbour
/// sitting there to be found so an empty answer is the guard's doing and not
/// the store's.
#[tokio::test]
async fn no_provider_and_no_embeddings_both_mean_no_neighbours() {
    let probe = format!("Retry queue gotcha\n{RETRY}");

    // No provider, over a store that has everything: one engine writes and
    // embeds, a second engine on the same store is built without a provider.
    // The embeddings are there and the neighbour is findable - the first
    // engine finds it - so the empty answer is the provider guard alone.
    let (tmp, store) = fresh_store().await;
    let embedding = build(
        &tmp,
        Arc::clone(&store),
        Some(Arc::new(support::TopicEmbedder)),
        None,
        None,
    );
    two_retry_engrams(&embedding).await;
    embedding.embed_pending().await.unwrap();
    let found = embedding
        .similar_engrams(
            &probe,
            Some(("open", "retry-queue-gotcha")),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        permalinks(&found),
        vec!["open/retry-backoff-lesson"],
        "the store holds a neighbour to find"
    );
    let blind = build(&tmp, store, None, None, None);
    assert!(
        blind
            .similar_engrams(
                &probe,
                Some(("open", "retry-queue-gotcha")),
                &Scope::Unrestricted,
            )
            .await
            .unwrap()
            .is_empty(),
        "no provider, so no probe, however much is indexed"
    );

    // A provider but nothing embedded: chunked and never embedded, so the mode
    // degrades to text and text is never probed.
    let (_tmp, engine) = engine().await;
    two_retry_engrams(&engine).await;
    assert!(
        engine
            .similar_engrams(
                &probe,
                Some(("open", "retry-queue-gotcha")),
                &Scope::Unrestricted,
            )
            .await
            .unwrap()
            .is_empty()
    );
    let mut receipt = retry_receipt();
    engine
        .attach_similar(&mut receipt, retry_probe(), &Scope::Unrestricted)
        .await;
    assert!(receipt.get("similar").is_none());

    // Embeddings for a different model, which is the case the mode guard is
    // really for. An empty store answers a semantic query empty whether the
    // guard exists or not, so it cannot discriminate; a store embedded under
    // another model refuses one with `StaleEmbeddings`. The guard is what
    // turns that refusal into a quiet receipt instead of an error the write
    // would have to carry.
    let (tmp, store) = fresh_store().await;
    let old_model = build(
        &tmp,
        Arc::clone(&store),
        Some(Arc::new(support::TopicEmbedder)),
        None,
        Some("model-that-was"),
    );
    two_retry_engrams(&old_model).await;
    old_model.embed_pending().await.unwrap();
    let new_model = build(
        &tmp,
        store,
        Some(Arc::new(support::TopicEmbedder)),
        None,
        Some("model-that-is"),
    );
    assert!(
        new_model
            .similar_engrams(
                &probe,
                Some(("open", "retry-queue-gotcha")),
                &Scope::Unrestricted,
            )
            .await
            .unwrap()
            .is_empty(),
        "a stale index is quiet, not an error"
    );
    let mut receipt = retry_receipt();
    new_model
        .attach_similar(&mut receipt, retry_probe(), &Scope::Unrestricted)
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

/// The backlog wait: bounded when a worker is wired, absent when none is.
///
/// Both halves start from the same shape - two engrams written, nothing
/// embedded, so the backlog is non-empty and never drains - and differ only in
/// whether an embed channel is wired. What separates them is time, which is
/// the only observable `await_embed_backlog` has.
#[tokio::test]
async fn the_backlog_wait_is_bounded_and_only_happens_with_a_worker() {
    let (tmp, store) = fresh_store().await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let worker = build(
        &tmp,
        store,
        Some(Arc::new(support::TopicEmbedder)),
        Some(tx),
        None,
    );
    two_retry_engrams(&worker).await;
    assert!(
        worker.embedding_backlog().await.unwrap() > 0,
        "the wait needs something to wait for"
    );

    let mut receipt = retry_receipt();
    let started = std::time::Instant::now();
    worker
        .attach_similar(&mut receipt, retry_probe(), &Scope::Unrestricted)
        .await;
    let waited = started.elapsed();
    assert!(
        waited >= SIMILAR_BACKLOG_WAIT,
        "a backlog that never drains is waited out in full: {waited:?}"
    );
    assert!(
        waited < SIMILAR_TIMEOUT,
        "and the wait stays inside the overall bound: {waited:?}"
    );
    assert!(
        receipt.get("similar").is_none(),
        "nothing drained, so nothing is embedded and nothing is near"
    );

    let (_tmp, none) = engine().await;
    two_retry_engrams(&none).await;
    assert!(none.embedding_backlog().await.unwrap() > 0);
    let mut receipt = retry_receipt();
    let started = std::time::Instant::now();
    none.attach_similar(&mut receipt, retry_probe(), &Scope::Unrestricted)
        .await;
    let waited = started.elapsed();
    assert!(
        waited < SIMILAR_BACKLOG_WAIT / 2,
        "with no worker there is nothing to wait for: {waited:?}"
    );
}

/// A probe cancelled mid-flight leaves the engine answering.
///
/// [`SIMILAR_TIMEOUT`] can cut a store call - the coverage read, the title
/// lookup or the search itself - and the store is one shared connection behind
/// a process-wide lock, so what matters is that a dropped future costs its own
/// answer and nothing else. The budgets sweep a range so that some land inside
/// a store call; where each one lands is not asserted, only that the engine is
/// whole afterwards.
#[tokio::test]
async fn a_cancelled_probe_leaves_the_engine_answering() {
    let (_tmp, engine) = engine().await;
    two_retry_engrams(&engine).await;
    engine.embed_pending().await.unwrap();
    let probe = format!("Retry queue gotcha\n{RETRY}");

    for budget in [1u64, 2, 4, 8, 16] {
        let _ = tokio::time::timeout(
            Duration::from_micros(budget * 100),
            engine.similar_engrams(
                &probe,
                Some(("open", "retry-queue-gotcha")),
                &Scope::Unrestricted,
            ),
        )
        .await;
    }

    let after = engine
        .similar_engrams(
            &probe,
            Some(("open", "retry-queue-gotcha")),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(permalinks(&after), vec!["open/retry-backoff-lesson"]);
}

/// A poll with a ceiling and no fixed sleep: the property is "well before the
/// 300-second tick", and the loop returns the moment it holds.
async fn backlog_drains(engine: &Engine, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    loop {
        if engine.embedding_backlog().await.unwrap() == 0 {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_virtual_write_is_embedded_long_before_the_tick() {
    let (_tmp, engine) = engine_with_worker().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    assert!(
        backlog_drains(&engine, Duration::from_secs(5)).await,
        "the write nudged the worker; nothing waited for the tick"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn back_to_back_writes_see_each_other() {
    let (_tmp, engine) = engine_with_worker().await;
    engine
        .write_engram(&write("open", "Retry queue gotcha", RETRY, None))
        .await
        .unwrap();
    let second = write("open", "Retry backoff lesson", RETRY_AGAIN, None);
    let mut receipt = engine.write_engram(&second).await.unwrap();
    engine
        .attach_similar(
            &mut receipt,
            SimilarProbe::for_write(&second),
            &Scope::Unrestricted,
        )
        .await;
    assert_eq!(
        receipt["similar"][0]["permalink"], "retry-queue-gotcha",
        "the probe waited for the worker: {receipt}"
    );
}
