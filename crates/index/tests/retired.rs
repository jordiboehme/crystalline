//! End-to-end coverage for the retired-status fade across all three scored
//! search modes (hybrid, lexical, semantic), on both backends. The fade is a
//! soft multiplicative reorder, never a filter: a retired engram's score drops
//! but it is always still returned. Mirrors the copy-helper convention from
//! `tests/salience.rs` (the FakeProvider embedding harness) and the parity
//! runner from `tests/store.rs` (Turso always, Postgres when
//! `CRYSTALLINE_TEST_POSTGRES_URL` is set).

use std::path::Path;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, DEFAULT_RETIRED_WEIGHT, EmbeddingProvider, Result, SearchMode, SearchQuery, Store,
    TursoStore, retired_factor, run_embedding_pass, sync_domain, sync_domain_with,
};

// --- fake provider (mirrored from tests/salience.rs) --------------------------

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

// --- helpers (mirrored from tests/salience.rs; `engram` extended with a
// `status` parameter, since salience.rs's version hardcodes `status: current`) -

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, status: &str, extra_fm: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: {status}\nrecorded_at: 2026-01-01\n{extra_fm}---\n\n{body}\n"
    )
}

/// Sync the corpus fingerprinting for the fake model, then embed everything.
async fn sync_and_embed(store: &dyn Store, name: &str, root: &Path, provider: &FakeProvider) {
    let params = ChunkParams::for_model(provider.model_id());
    sync_domain_with(store, name, root, &params).await.unwrap();
    run_embedding_pass(store, provider, |_, _| {})
        .await
        .unwrap();
}

// --- backend runner (mirrored from tests/store.rs) -----------------------------

#[cfg(feature = "postgres")]
fn pg_url() -> Option<String> {
    use std::sync::Once;
    static NOTE: Once = Once::new();
    match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            NOTE.call_once(|| {
                eprintln!(
                    "note: skipping the postgres parity leg (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
                )
            });
            None
        }
    }
}

/// A distinct schema name per test invocation. The pid keeps runs apart, the
/// counter keeps tests within a run apart; both stay well under Postgres's
/// 63-byte identifier limit.
#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ct_{}_{}", std::process::id(), n)
}

/// Run a parity body against Turso (always) and Postgres (when configured),
/// giving each backend a fresh, isolated store.
macro_rules! parity {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                $body(&store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(url) = pg_url() {
                    let schema = unique_schema();
                    let store = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .expect("open the postgres test schema");
                    $body(&store).await;
                    store
                        .drop_schema()
                        .await
                        .expect("drop the postgres test schema");
                }
            }
        }
    };
}

// --- tests ---------------------------------------------------------------------

/// Two engrams with identical body text, so lexical and semantic relevance tie
/// exactly and only the fade can separate their scores. Titles are picked so
/// the retired engram would win the ascending title tiebreak absent the fade:
/// the ranking assertion below only holds because of the fade, not the
/// tiebreak.
async fn hybrid_fade(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "current.md",
        &engram(
            "Zeta",
            "current",
            "current",
            "",
            "vector index tuning notes",
        ),
    );
    write(
        root,
        "superseded.md",
        &engram(
            "Alpha",
            "superseded",
            "superseded",
            "",
            "vector index tuning notes",
        ),
    );
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(store, "d", root, &fake).await;

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

    assert_eq!(page.total, 2, "the retired engram must never be filtered");
    assert_eq!(
        page.items[0].permalink, "current",
        "the current engram outranks the faded retired one"
    );
    assert_eq!(page.items[1].permalink, "superseded");
    assert!(page.items[0].score > 0.0);
    assert!(page.items[1].score > 0.0);
}
parity!(
    retired_engram_fades_in_hybrid_but_is_never_filtered,
    hybrid_fade
);

/// Text-mode lexical search, no embeddings. Identical bodies tie the raw
/// term-frequency score exactly; the superseded engram gets the
/// alphabetically-first title, so absent the fade the title tiebreak would
/// rank it first. Ranking it second instead proves the reorder is the fade,
/// not the tiebreak.
async fn lexical_fade(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "current.md",
        &engram(
            "Zulu",
            "current",
            "current",
            "",
            "vector index tuning notes",
        ),
    );
    write(
        root,
        "superseded.md",
        &engram(
            "Alpha",
            "superseded",
            "superseded",
            "",
            "vector index tuning notes",
        ),
    );
    sync_domain(store, "d", root).await.unwrap();

    let query = SearchQuery {
        text: Some("vector index tuning notes".to_string()),
        mode: SearchMode::Text,
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    };
    let page = store.search(&query).await.unwrap();

    assert_eq!(page.total, 2, "the retired engram must never be filtered");
    assert_eq!(
        page.items[0].permalink, "current",
        "absent the fade, 'Alpha' would win the title tiebreak and rank first"
    );
    assert_eq!(page.items[1].permalink, "superseded");
}
parity!(retired_engram_fades_in_lexical_mode, lexical_fade);

/// Identical bodies, and a shared title reusing a body word (so the title
/// prepended into the chunk text by the chunker lands in an already-occupied
/// hash bucket rather than diluting the vector into a fresh one): both
/// engrams land at the same raw cosine similarity to the query, comfortably
/// above the 0.9 cutoff. The superseded engram's faded score (`~0.6x` that,
/// well below the cutoff) must still be returned, because the cutoff gates on
/// the raw similarity, before the fade is applied. If the fade ever migrated
/// ahead of the `retain`, this test goes red.
async fn semantic_gate_on_raw_similarity(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "current.md",
        &engram(
            "Notes",
            "current",
            "current",
            "",
            "vector index tuning notes",
        ),
    );
    write(
        root,
        "superseded.md",
        &engram(
            "Notes",
            "superseded",
            "superseded",
            "",
            "vector index tuning notes",
        ),
    );
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(store, "d", root, &fake).await;

    let query = SearchQuery {
        text: Some("vector index tuning notes".to_string()),
        mode: SearchMode::Semantic,
        query_embedding: Some(embed_one("vector index tuning notes")),
        active_model: Some(fake.model_id().to_string()),
        min_similarity: Some(0.9),
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    };
    let page = store.search(&query).await.unwrap();

    assert_eq!(
        page.total, 2,
        "the retired hit clears the raw-similarity gate and must survive the fade"
    );
    assert_eq!(page.items[0].permalink, "current");
    assert_eq!(page.items[1].permalink, "superseded");
    assert!(
        page.items[0].score >= 0.9,
        "the current hit keeps its full (unfaded) similarity, at or above the cutoff, got {}",
        page.items[0].score
    );
    assert!(
        page.items[1].score < 0.9,
        "the faded score sits below the min_similarity cutoff, proving the \
         gate ran on the raw similarity rather than the faded score, got {}",
        page.items[1].score
    );
    let ratio = page.items[1].score / page.items[0].score;
    let expected_factor = retired_factor("superseded", DEFAULT_RETIRED_WEIGHT);
    assert!(
        (ratio - expected_factor).abs() < 0.02,
        "the reported score is the raw similarity times the fade factor, got ratio {ratio}"
    );
}
parity!(
    semantic_min_similarity_gates_on_raw_similarity_not_the_faded_score,
    semantic_gate_on_raw_similarity
);

/// The same tied-relevance corpus as `hybrid_fade`, but `retired_weight` is
/// overridden to 1.0, which disables the fade entirely (full rank for every
/// status). Scores tie exactly again, so ordering falls back to the ascending
/// title tiebreak: the superseded engram's title sorts first, so it must rank
/// first.
async fn hybrid_weight_one_disables_fade(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "current.md",
        &engram(
            "Zeta",
            "current",
            "current",
            "",
            "vector index tuning notes",
        ),
    );
    write(
        root,
        "superseded.md",
        &engram(
            "Alpha",
            "superseded",
            "superseded",
            "",
            "vector index tuning notes",
        ),
    );
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(store, "d", root, &fake).await;

    let query = SearchQuery {
        text: Some("vector index tuning notes".to_string()),
        mode: SearchMode::Hybrid,
        query_embedding: Some(embed_one("vector index tuning notes")),
        active_model: Some(fake.model_id().to_string()),
        retired_weight: Some(1.0),
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    };
    let page = store.search(&query).await.unwrap();

    assert_eq!(page.total, 2);
    assert_eq!(
        page.items[0].permalink, "superseded",
        "weight 1.0 disables the fade, so the title tiebreak decides"
    );
    assert_eq!(page.items[1].permalink, "current");
}
parity!(
    retired_weight_one_disables_the_fade,
    hybrid_weight_one_disables_fade
);
