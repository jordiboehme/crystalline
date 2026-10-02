//! A GPU that fails after a good model load moves that model to the CPU, end
//! to end on the engine: a semantic search still answers, the contradiction
//! pass still completes, and status names the CPU with the runtime reason.
//!
//! The models here are stubs held in the real [`ModelSlot`]: one instance
//! stands for the model on Metal and fails every call once the "GPU" is
//! broken, the CPU instance the slot rebuilds works. No GPU is needed, so the
//! tests run on any machine.

#![cfg(feature = "local-embeddings")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::device::ModelSlot;
use crystalline_index::nli::{ContradictionScorer, NliModel, NliProfile, StubScorer, nli_model};
use crystalline_index::{DeviceReport, EmbeddingProvider, IndexError, TursoStore};
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::engine::{ContradictionOutcome, ScorerLoader};
use crystalline_service::params::{SearchParams, WriteParams};
use tokio::sync::Mutex;

use crate::support::TopicEmbedder;

/// What a stub model's two instances count.
#[derive(Default)]
struct Counts {
    gpu_calls: AtomicUsize,
    cpu_calls: AtomicUsize,
    rebuilds: AtomicUsize,
    /// Once set, every call on the GPU instance fails.
    gpu_broken: AtomicBool,
}

/// One instance of a stub model: the GPU one or the CPU one.
struct Instance {
    gpu: bool,
    counts: Arc<Counts>,
}

impl Instance {
    /// Count the call and fail it when this is the GPU instance and the GPU
    /// is broken.
    fn enter(&self, error: fn(String) -> IndexError) -> crystalline_index::Result<()> {
        if self.gpu {
            self.counts.gpu_calls.fetch_add(1, Ordering::SeqCst);
            if self.counts.gpu_broken.load(Ordering::SeqCst) {
                return Err(error("inference: command buffer error".to_string()));
            }
        } else {
            self.counts.cpu_calls.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// A slot that loaded the stub "on Metal" and rebuilds it on the CPU.
fn slot(
    what: &'static str,
    counts: &Arc<Counts>,
    error: fn(String) -> IndexError,
) -> ModelSlot<Instance> {
    let rebuild = counts.clone();
    ModelSlot::new(
        what,
        Instance {
            gpu: true,
            counts: counts.clone(),
        },
        DeviceReport::metal(),
        error,
        move || {
            rebuild.rebuilds.fetch_add(1, Ordering::SeqCst);
            Ok(Instance {
                gpu: false,
                counts: rebuild.clone(),
            })
        },
    )
}

/// The topic embedder, run through a slot.
struct SlotEmbedder {
    slot: ModelSlot<Instance>,
}

#[async_trait::async_trait]
impl EmbeddingProvider for SlotEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        self.slot.run(|instance| {
            instance.enter(IndexError::Embedding)?;
            Ok(texts.iter().map(|t| TopicEmbedder::embed_one(t)).collect())
        })
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

    fn device(&self) -> Option<DeviceReport> {
        Some(self.slot.report())
    }
}

/// The stub scorer, run through a slot.
struct SlotScorer {
    slot: ModelSlot<Instance>,
    stub: StubScorer,
}

impl ContradictionScorer for SlotScorer {
    fn score(&self, pairs: &[(String, String)]) -> crystalline_index::Result<Vec<f32>> {
        self.slot.run(|instance| {
            instance.enter(IndexError::Nli)?;
            self.stub.score(pairs)
        })
    }

    fn model_repo(&self) -> &str {
        self.stub.model_repo()
    }

    fn device(&self) -> Option<DeviceReport> {
        Some(self.slot.report())
    }
}

async fn engine_on(provider: Arc<dyn EmbeddingProvider>) -> (tempfile::TempDir, Engine) {
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
    (tmp, engine)
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
        share_link: None,
        model: None,
    }
}

const NODE_18: &str = "The retry queue build runs on a pinned runtime.\n\n- [fact] The build uses Node 18\n- [fact] Retries back off on the queue";
const NODE_20: &str = "The retry queue build runs on a pinned runtime.\n\n- [fact] The build uses Node 20\n- [fact] Retries back off on the queue";
const FRIDAYS: &str = "Docking clamps seat in bay three under thrust.\n\n- [fact] Deployments run on Fridays\n- [fact] The clamps misread below eight degrees";

async fn three(engine: &Engine) {
    for (title, content) in [
        ("Eighteen", NODE_18),
        ("Twenty", NODE_20),
        ("Fridays", FRIDAYS),
    ] {
        engine.write_engram(&write(title, content)).await.unwrap();
    }
    engine.embed_pending().await.unwrap();
}

async fn semantic(engine: &Engine, query: &str) -> serde_json::Value {
    engine
        .search_engrams(
            &SearchParams {
                query: Some(query.to_string()),
                search_type: Some("semantic".to_string()),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .expect("a GPU failure never fails the search")
}

/// The embedding model fails on the GPU while a search embeds its query: the
/// same query is embedded again on the CPU and the search answers; status
/// names the CPU and why; the next query never touches the GPU.
#[tokio::test]
async fn a_search_survives_a_gpu_failure_and_the_model_stays_on_the_cpu() {
    let counts = Arc::new(Counts::default());
    let provider = Arc::new(SlotEmbedder {
        slot: slot("embedding model", &counts, IndexError::Embedding),
    });
    let (_tmp, engine) = engine_on(provider).await;
    three(&engine).await;
    let status = engine.status_report().await.unwrap();
    assert_eq!(status["embeddings"]["device"], "metal", "{status}");
    let before = counts.gpu_calls.load(Ordering::SeqCst);
    assert!(before > 0, "the documents were embedded on the GPU");

    counts.gpu_broken.store(true, Ordering::SeqCst);
    let hits = semantic(&engine, "retry backoff").await;
    assert_eq!(hits["mode"], "semantic", "{hits}");
    let found: Vec<&str> = hits["hits"]
        .as_array()
        .expect("a hit list")
        .iter()
        .filter_map(|h| h["permalink"].as_str())
        .collect();
    assert!(
        found.contains(&"eighteen") && found.contains(&"twenty"),
        "{hits}"
    );

    let status = engine.status_report().await.unwrap();
    assert_eq!(
        status["embeddings"]["device"],
        "cpu (metal failed at runtime: embedding error: inference: command buffer error)",
        "{status}"
    );
    assert_eq!(counts.rebuilds.load(Ordering::SeqCst), 1);
    let gpu_after_failure = counts.gpu_calls.load(Ordering::SeqCst);
    assert_eq!(gpu_after_failure, before + 1, "one failed call on the GPU");

    semantic(&engine, "docking clamps").await;
    assert_eq!(
        counts.gpu_calls.load(Ordering::SeqCst),
        gpu_after_failure,
        "the second query never touches the GPU"
    );
    assert_eq!(counts.rebuilds.load(Ordering::SeqCst), 1);
}

fn full() -> &'static NliModel {
    nli_model(NliProfile::Full)
}

/// The contradiction model fails on the GPU in the middle of a pass: the
/// batch is scored again on the CPU, the pass completes with nothing parked,
/// and status names the CPU and why.
#[tokio::test]
async fn the_scoring_pass_completes_on_the_cpu_after_a_gpu_failure() {
    let counts = Arc::new(Counts::default());
    counts.gpu_broken.store(true, Ordering::SeqCst);
    let scorer: Arc<dyn ContradictionScorer> = Arc::new(SlotScorer {
        slot: slot("contradiction model", &counts, IndexError::Nli),
        stub: StubScorer::new(full().repo, 0.05).with(
            "The build uses Node 18",
            "The build uses Node 20",
            0.93,
        ),
    });
    let loader: ScorerLoader = Arc::new(move |_model: &'static NliModel| {
        let scorer = scorer.clone();
        Box::pin(async move { Ok(scorer) })
    });
    let (_tmp, engine) = engine_on(Arc::new(TopicEmbedder)).await;
    let engine = engine.with_scorer_loader(loader);
    engine
        .configure(&crystalline_service::engine::ConfigureAction::Set {
            key: "evolve.contradictions".to_string(),
            value: "full".to_string(),
        })
        .await
        .unwrap();
    three(&engine).await;

    assert_eq!(
        engine.score_contradictions().await.unwrap(),
        ContradictionOutcome::Scored {
            pairs: 1,
            line_pairs: 1,
            remaining: 0,
        },
        "the same outcome as a pass that never saw the GPU fail"
    );
    let status = engine.contradictions_status().await.unwrap();
    assert_eq!(status["failing_pairs"], 0, "{status}");
    assert!(status["last_error"].is_null(), "{status}");
    assert_eq!(
        status["device"],
        "cpu (metal failed at runtime: contradiction model error: inference: command buffer error)",
        "{status}"
    );
    assert_eq!(
        counts.gpu_calls.load(Ordering::SeqCst),
        1,
        "one failed batch"
    );
    assert_eq!(counts.rebuilds.load(Ordering::SeqCst), 1);
    assert!(counts.cpu_calls.load(Ordering::SeqCst) >= 1);
}
