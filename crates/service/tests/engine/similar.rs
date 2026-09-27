//! The neighbours advisory at the engine: what a probe finds, what it never
//! finds, and how it fails.

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
        Some(Arc::new(crate::support::TopicEmbedder)),
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
        Some(Arc::new(crate::support::TopicEmbedder)),
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
        share_link: None,
        model: None,
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
    for name in ["keeper", "out"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("lab", true, "keeper")
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
        .similar_engrams(
            &probe,
            Some(("open", "retry-queue-gotcha")),
            &user("keeper"),
        )
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

/// The three ways the probe answers nothing: no provider, nothing embedded,
/// and embeddings under another model.
///
/// The first and the third have a neighbour sitting there to be found, so an
/// empty answer is the guard's doing and not the store's. The middle one
/// cannot be shown that way and does not pretend to be: with nothing embedded
/// at all there is no neighbour for a guard to be hiding, and the case is here
/// because a store in that state has to stay quiet rather than because it
/// discriminates a guard.
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
        Some(Arc::new(crate::support::TopicEmbedder)),
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
        Some(Arc::new(crate::support::TopicEmbedder)),
        None,
        Some("model-that-was"),
    );
    two_retry_engrams(&old_model).await;
    old_model.embed_pending().await.unwrap();
    let new_model = build(
        &tmp,
        store,
        Some(Arc::new(crate::support::TopicEmbedder)),
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
    engine.set_provider(Arc::new(crate::support::SleepyEmbedder {
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

/// A probe that finishes inside the budget logs its elapsed time at `debug`,
/// alongside attaching the advisory as usual.
#[tokio::test]
async fn a_completed_probe_logs_its_elapsed_time() {
    let (_tmp, engine) = engine().await;
    two_retry_engrams(&engine).await;
    engine.embed_pending().await.unwrap();
    let (logs, _guard) = crate::support::capture_logs();
    let mut receipt = retry_receipt();
    engine
        .attach_similar(&mut receipt, retry_probe(), &Scope::Unrestricted)
        .await;
    assert!(
        receipt.get("similar").is_some(),
        "a neighbour should have been found"
    );
    assert!(
        logs.any_contains("similar probe completed in"),
        "{:?}",
        logs.lines()
    );
}

/// The bug this covers: `tokio::time::timeout` only cuts a future at a point
/// where it yields, and the turso binding steps a store statement
/// synchronously with no such point inside it, so a slow query runs to
/// completion no matter what the timeout is set to and leaves no trace
/// unless the wall clock is checked afterward.
///
/// [`crate::support::BlockingEmbedder`] reproduces that shape at the provider
/// instead of the store, which is the same failure mode from the timeout's
/// point of view: unlike [`crate::support::SleepyEmbedder`]'s `tokio::time::sleep`
/// (an async, cancellable delay - see `a_slow_provider_is_cut_at_the_timeout`
/// above), its delay is a `std::thread::sleep` inside the poll, so nothing
/// yields and the timeout cannot cut it.
#[tokio::test]
async fn a_probe_that_blocks_the_thread_logs_an_overrun() {
    let (_tmp, engine) = engine().await;
    two_retry_engrams(&engine).await;
    engine.embed_pending().await.unwrap();
    let over_budget = SIMILAR_TIMEOUT + Duration::from_millis(1300);
    engine.set_provider(Arc::new(crate::support::BlockingEmbedder {
        delay: over_budget,
    }));
    let (logs, _guard) = crate::support::capture_logs();
    let mut receipt = retry_receipt();
    let started = std::time::Instant::now();
    engine
        .attach_similar(&mut receipt, retry_probe(), &Scope::Unrestricted)
        .await;
    assert!(
        started.elapsed() >= over_budget,
        "a synchronous block runs past the timeout uncut: {:?}",
        started.elapsed()
    );
    assert!(
        logs.any_contains("similar probe overran its budget"),
        "{:?}",
        logs.lines()
    );
    assert!(logs.any_contains("on domain 'open'"), "{:?}", logs.lines());
    assert!(logs.any_contains("(write)"), "{:?}", logs.lines());
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
        Some(Arc::new(crate::support::TopicEmbedder)),
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

// --- Task 6: the advisory is the writer's own view of the domain -------------

/// The frontmatter of a base engram the team has reviewed.
fn team_engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A file domain `team` in review mode holding exactly `files`, with the topic
/// provider installed and a state directory of its own so a draft can be
/// mirrored.
///
/// Review mode is written straight into the configuration, as Task 4's
/// fixtures do: turning it on through a verb is Task 7's, and a fixture that
/// had to satisfy the enabling gates would be testing those instead.
async fn review_engine(files: &[(&str, &str)]) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("team");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: team\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# team\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for team work\n",
    )
    .unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let mut cfg = GlobalConfig::default();
    let mut entry = DomainEntry::file(dir);
    entry.review = Some(crystalline_core::config::ReviewMode::Overlay);
    cfg.domains.insert("team".to_string(), entry);
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(store)),
            cfg,
            Some(Arc::new(crate::support::TopicEmbedder)),
            Some(config_path),
        )
        .with_state_dir(tmp.path().join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// One account, as an authenticated surface resolves it.
fn account(name: &str) -> Scope {
    Scope::User {
        account: name.to_string(),
        admin: false,
    }
}

/// The advisory on a draft's receipt never names the base row that draft
/// stands over, and never names the draft itself.
///
/// Two rules meet here and the test needs both. The candidate set is the
/// writer's shadowed view, so the base row at the path she is drafting is not
/// a candidate at all - without that she would be told her own engram is close
/// to what she just wrote, with the team's wording, which is exactly the merge
/// advice she must not act on. And the exclusion of the engram that was just
/// written is by address across every actor's rows, so her own draft drops out
/// of its own advisory however its row is keyed.
///
/// Her draft moves the permalink, which is what makes the first rule
/// observable: while the addresses agree, the exclusion alone would hide the
/// base row and the shadow would never be tested.
#[tokio::test]
async fn a_drafts_receipt_never_lists_its_own_base_row() {
    let (_tmp, engine) = review_engine(&[
        (
            "retry-queue-gotcha.md",
            &team_engram("Retry queue gotcha", "retry-queue-gotcha", RETRY),
        ),
        (
            "retry-backoff-lesson.md",
            &team_engram("Retry backoff lesson", "retry-backoff-lesson", RETRY_AGAIN),
        ),
    ])
    .await;
    let alice = account("alice");

    let read = engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: "retry-queue-gotcha".to_string(),
                domain: Some("team".to_string()),
                share_link: None,
            },
            &alice,
        )
        .await
        .unwrap();
    let saved_text = team_engram("Retry queue notes", "retry-queue-notes", RETRY);
    let mut receipt = engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "retry-queue-gotcha".to_string(),
                content: saved_text.clone(),
                expected_checksum: read["checksum"].as_str().unwrap().to_string(),
            },
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(receipt["draft"], serde_json::json!(true), "{receipt}");
    engine.embed_pending().await.unwrap();
    engine
        .attach_similar(
            &mut receipt,
            SimilarProbe::Markdown { text: &saved_text },
            &alice,
        )
        .await;

    let named: Vec<&str> = receipt["similar"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["permalink"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !named.contains(&"retry-queue-gotcha"),
        "the base row she is drafting over is not a neighbour of her draft: {receipt}"
    );
    assert!(
        !named.contains(&"retry-queue-notes"),
        "and neither is the draft itself, whatever address it answers to: {receipt}"
    );
    assert!(
        named.contains(&"retry-backoff-lesson"),
        "what the team has elsewhere on the topic is still the advice: {receipt}"
    );

    // And again through the other verb, which is where the two halves come
    // apart: an edit's receipt names the engram by the address the TEAM knows
    // it by, because a draft over a base row resolves to the base descriptor,
    // while the row a search answers with carries the address her document
    // gave it. Excluding only what the receipt says would hand her her own
    // draft as a neighbour, under guidance that tells her to merge into it.
    let appended =
        "and a retry that exhausts its backoff is parked in the dead-letter queue for the ttl";
    let mut edited = engine
        .edit_engram_as(
            &crystalline_service::params::EditParams {
                identifier: "retry-queue-gotcha".to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some(format!("- [decision] {appended} #team")),
                key: None,
                value: None,
                find_text: None,
                expected_replacements: None,
                section: None,
                include_subsections: false,
                expected_checksum: None,
                ack_scope: None,
                share_link: None,
                model: None,
            },
            None,
            &alice,
        )
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    engine
        .attach_similar(
            &mut edited,
            SimilarProbe::Edit { new_text: appended },
            &alice,
        )
        .await;
    let after: Vec<&str> = edited["similar"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["permalink"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !after.contains(&"retry-queue-notes") && !after.contains(&"retry-queue-gotcha"),
        "an edit of the same draft names neither the draft nor the row it stands \
         over, whichever address each of them answers to: {edited}"
    );
}

/// An author's own drafts can fill the advisory, and the cut stands.
///
/// The neighbours are rows competing on one ladder, not two lists merged: a
/// draft sits where its own text puts it and nothing reserves a slot for the
/// domain's files. So an author drafting several engrams on one topic is told
/// about their own drafts and not about the reviewed engram further away -
/// which is the ranking answering the question it was asked, "what is nearest
/// to what you just wrote", rather than the advisory failing.
///
/// The crowding is a page-boundary effect rather than a `SIMILAR_LIMIT` one:
/// the page is one wider than the receipt and the write itself spends a slot,
/// so it takes three nearer drafts to push the base row out of the page
/// entirely. The colleague's half of the test is what says the base row is
/// findable at all: bob, who drafts nothing, is told about it from the very
/// same probe text.
#[tokio::test]
async fn an_authors_own_drafts_can_fill_the_advisory_and_the_cut_stands() {
    // The one base engram on the topic is deliberately further away than any
    // draft: a few docking markers among its retry ones tilt its lead vector
    // off the axis every draft below is exactly on, while it still clears the
    // search floor (the split is 7:2, about cosine 0.96). A base row at
    // the same distance would make which four rows reach the page a tie rather
    // than a fact.
    let (_tmp, engine) = review_engine(&[(
        "retry-clamp-runbook.md",
        &team_engram(
            "Retry clamp runbook",
            "retry-clamp-runbook",
            "The retry queue doubles its backoff and the dead-letter ttl bounds a retry.\nThe hangar door swings shut before docking begins.",
        ),
    )])
    .await;
    let alice = account("alice");

    let probe = async |title: &str, body: &str, scope: &Scope| {
        let params = write("team", title, body, None);
        let mut receipt = engine.write_engram_as(&params, None, scope).await.unwrap();
        engine.embed_pending().await.unwrap();
        engine
            .attach_similar(&mut receipt, SimilarProbe::for_write(&params), scope)
            .await;
        receipt
    };
    let named = |receipt: &serde_json::Value| -> Vec<String> {
        receipt["similar"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| r["permalink"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };

    // Her first draft is told about the engram the team reviewed: one draft
    // does not crowd anything.
    let first = probe("Retry ttl note", RETRY, &alice).await;
    assert_eq!(
        named(&first),
        vec!["retry-clamp-runbook".to_string()],
        "with nothing of her own on the topic, the team's engram is the advice: {first}"
    );

    probe("Retry backoff note", RETRY_AGAIN, &alice).await;
    probe("Retry queue note", RETRY, &alice).await;
    let fourth = probe("Retry dead-letter note", RETRY_AGAIN, &alice).await;
    let mut hers = named(&fourth);
    hers.sort();
    assert_eq!(
        hers,
        vec![
            "retry-backoff-note".to_string(),
            "retry-queue-note".to_string(),
            "retry-ttl-note".to_string()
        ],
        "her three nearer drafts fill the page, and the cut stands: {fourth}"
    );

    // And the base row is findable, which is what makes the line above a
    // crowding rather than a distance: the same probe text answers bob, who
    // is drafting nothing, with the engram the team has.
    let bobs = probe("Retry ledger note", RETRY_AGAIN, &account("bob")).await;
    assert_eq!(
        named(&bobs),
        vec!["retry-clamp-runbook".to_string()],
        "his advisory is the team's engram, and names no draft of hers: {bobs}"
    );
}
