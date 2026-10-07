//! End-to-end coverage for the salience ranking prior in hybrid search: a
//! numeric `salience` frontmatter field lifts an engram's score by a bounded,
//! additive amount and can never filter a result out. No network: uses the
//! deterministic `FakeProvider` pattern mirrored from `tests/embed.rs`.

use std::path::Path;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, EmbeddingProvider, Result, SearchMode, SearchQuery, Store, TursoStore,
    run_embedding_pass, sync_domain_with,
};

// --- fake provider (mirrored from tests/embed.rs) -----------------------------

/// A deterministic, network-free provider. It hashes each word into one of eight
/// buckets and L2-normalizes, so texts that share vocabulary get similar
/// vectors: enough structure to exercise ranking.
struct FakeProvider {
    model: String,
}

impl FakeProvider {
    fn new(model: &str) -> FakeProvider {
        FakeProvider {
            model: model.to_string(),
        }
    }
}

fn embed_one(text: &str) -> Vec<f32> {
    let mut v = [0f32; 8];
    for tok in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
    {
        let mut h: u64 = 0;
        for byte in tok.to_lowercase().bytes() {
            h = h.wrapping_mul(31).wrapping_add(byte as u64);
        }
        v[(h % 8) as usize] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        let mut z = [0f32; 8];
        z[0] = 1.0;
        return z.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

#[async_trait]
impl EmbeddingProvider for FakeProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| embed_one(t)).collect())
    }
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dims(&self) -> usize {
        8
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
}

// --- helpers (mirrored from tests/embed.rs) -----------------------------------

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, extra_fm: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n{extra_fm}---\n\n{body}\n"
    )
}

async fn open() -> TursoStore {
    TursoStore::open_in_memory().await.unwrap()
}

/// Sync the corpus fingerprinting for the fake model, then embed everything.
async fn sync_and_embed(store: &TursoStore, name: &str, root: &Path, provider: &FakeProvider) {
    let params = ChunkParams::for_model(provider.model_id());
    sync_domain_with(store, name, root, &params).await.unwrap();
    run_embedding_pass(store, provider, |_, _| {})
        .await
        .unwrap();
}

// --- tests ---------------------------------------------------------------------

#[tokio::test]
async fn salient_engram_outranks_equal_relevance_neighbor() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Two engrams with the same relevant body text, so lexical relevance ties
    // and semantic relevance is close. Only `b` is marked salient.
    write(
        root,
        "a.md",
        &engram("Plain", "a", "", "vector index tuning notes"),
    );
    write(
        root,
        "b.md",
        &engram("Salient", "b", "salience: 9\n", "vector index tuning notes"),
    );
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(&store, "d", root, &fake).await;

    let query = SearchQuery {
        text: Some("vector index tuning notes".to_string()),
        mode: SearchMode::Hybrid,
        query_embedding: Some(embed_one("vector index tuning notes")),
        active_model: Some(fake.model_id().to_string()),
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    };
    let page = store.search(&query).await.unwrap();

    // Never a filter: both engrams are still returned.
    assert_eq!(page.total, 2, "salience must not drop any result");
    // The salient engram ranks first.
    assert_eq!(page.items[0].permalink, "b");
    assert_eq!(page.items[1].permalink, "a");
    // The lift is bounded: the salient score exceeds the plain one but by no
    // more than the default weight.
    let lead = page.items[0].score - page.items[1].score;
    assert!(
        lead > 0.0 && lead <= 0.15 + 1e-9,
        "bounded lift, got {lead}"
    );
}

#[tokio::test]
async fn salience_never_overrides_a_clearly_more_relevant_hit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // `a` is far more relevant to the query; `b` is only weakly relevant but
    // maximally salient. Relevance must still win.
    write(
        root,
        "a.md",
        &engram(
            "OnTopic",
            "a",
            "",
            "vector index tuning vector index tuning",
        ),
    );
    write(
        root,
        "b.md",
        &engram(
            "OffTopic",
            "b",
            "salience: 10\n",
            "unrelated kitchen recipe",
        ),
    );
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(&store, "d", root, &fake).await;

    let query = SearchQuery {
        text: Some("vector index tuning".to_string()),
        mode: SearchMode::Hybrid,
        query_embedding: Some(embed_one("vector index tuning")),
        active_model: Some(fake.model_id().to_string()),
        limit: 10,
        page: 1,
        // `b`'s raw cosine similarity to the query sits below the default
        // floor. Drop the floor to zero so `b` clears the semantic gate and
        // actually competes in the candidate set: without this, the test
        // would pass trivially because `b` never entered the ranking.
        min_similarity: Some(0.0),
        ..SearchQuery::default()
    };
    let page = store.search(&query).await.unwrap();
    // Both engrams must be candidates, or the assertion below proves nothing.
    assert_eq!(
        page.total, 2,
        "b must be in the candidate set so its salience actually competes"
    );
    assert_eq!(
        page.items[0].permalink, "a",
        "relevance dominates a small prior"
    );
}
