//! The semantic candidate scan runs as two queries: a narrow top-k that groups
//! and orders over ids and distances only, then a hydrate of the winners by id.
//! These tests pin what that split must not change.
//!
//! The shape that mattered in the field (2026-07-28, see
//! `research/2026-07-28-turso-sorter-spill.md`) is one multi-MB engram with many
//! chunks: the old single query fed that engram's whole body into a `GROUP BY`
//! sorter once per chunk, which is quadratic in engram size. The corpus here is
//! that shape in miniature. CI cannot assert temp-file bytes robustly, so what
//! is asserted is correctness: the closest chunk still decides its engram's
//! rank, every column the hydrate carries still arrives, and ties are broken the
//! same way on every run and on both backends.
//!
//! Mirrors the FakeProvider harness from `tests/retired.rs` and the parity
//! runner from `tests/store.rs` (Turso always, Postgres when
//! `CRYSTALLINE_TEST_POSTGRES_URL` is set).

use std::path::Path;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, EmbeddingProvider, Result, SearchMode, SearchQuery, Store, TursoStore,
    run_embedding_pass, sync_domain_with,
};

// --- fake provider (mirrored from tests/retired.rs) ---------------------------

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

// --- helpers ------------------------------------------------------------------

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
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

fn semantic_query(text: &str, provider: &FakeProvider) -> SearchQuery {
    SearchQuery {
        text: Some(text.to_string()),
        mode: SearchMode::Semantic,
        query_embedding: Some(embed_one(text)),
        active_model: Some(provider.model_id().to_string()),
        // Keep every candidate: this is a ranking and hydration test, not a
        // cutoff test, and the cutoff is covered in tests/embed.rs.
        min_similarity: Some(0.0),
        limit: 20,
        page: 1,
        ..SearchQuery::default()
    }
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

// --- corpus -------------------------------------------------------------------

/// The word the query is about. It appears exactly once in the whole corpus,
/// buried in the middle of the giant engram, so only one of that engram's many
/// chunks is close to the query vector.
const NEEDLE: &str = "quasar telemetry drift anomaly";

/// Filler vocabulary. Every word hashes into an embedding bucket the needle's
/// words do not touch, so a filler-only chunk is exactly orthogonal to the query
/// vector and cannot rank by accident.
const FILLER: &str = "lantern cobble ferry willow maple birch alder hazel elder gorse";

/// A multi-megabyte body whose middle holds the needle, as one paragraph long
/// enough to fill its own chunks. Everything around it is filler, so the engram
/// can only be found through that one buried region.
fn giant_body() -> String {
    // ~2.1MB: about 1200 chunks at the default 450-token budget, which is the
    // many-chunks-per-engram condition the split exists for.
    let half = format!("{FILLER}\n\n").repeat(15_000);
    let needle_block = format!("{NEEDLE} ").repeat(200);
    format!("{half}\n\n{needle_block}\n\n{half}")
}

// --- tests --------------------------------------------------------------------

/// The closest chunk decides its engram's rank, however many chunks that engram
/// has. Under the old single query this ranking came out of a sorter that had
/// been fed the giant body once per chunk; under the split it comes out of a
/// two-column top-k and a keyed hydrate. The answer must be the same.
async fn the_closest_chunk_still_decides_the_rank(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "giant.md", &engram("Giant", "giant", &giant_body()));
    // A small engram whose whole body is unrelated filler: it must lose to the
    // giant engram's one matching chunk.
    write(root, "small.md", &engram("Brief", "small", FILLER));
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(store, "d", root, &fake).await;

    // The corpus really is the shape this test exists for. Under the old query
    // this many chunks over this body is a multi-gigabyte sorter spill.
    let coverage = store.embedding_coverage().await.unwrap();
    assert!(
        coverage.total_chunks > 1000,
        "the giant engram is chunked into a four-figure count, got {}",
        coverage.total_chunks
    );

    let page = store.search(&semantic_query(NEEDLE, &fake)).await.unwrap();

    assert_eq!(page.total, 2, "both engrams are candidates");
    assert_eq!(
        page.items[0].permalink, "giant",
        "the engram holding the needle in one of its ~1200 chunks ranks first"
    );
    assert!(
        page.items[0].score > page.items[1].score,
        "its one close chunk beats the small engram's whole body"
    );

    // The hydrate carries every column the old projection returned. A dropped
    // or reordered column would surface as an empty field here.
    let hit = &page.items[0];
    assert_eq!(hit.domain, "d");
    assert_eq!(hit.title, "Giant");
    assert_eq!(hit.engram_type, "engram");
    assert_eq!(hit.status, "current");
    assert_eq!(hit.tags, vec!["t".to_string()]);
    assert!(
        !hit.snippet.is_empty(),
        "the snippet is cut from the hydrated body"
    );
}
parity!(
    semantic_search_ranks_a_multi_mb_engram_by_its_closest_chunk,
    the_closest_chunk_still_decides_the_rank
);

/// Two engrams with byte-identical embedded text tie exactly on distance, so the
/// order between them is decided by the tiebreak rather than by the score. The
/// phase-1 `ORDER BY dist ASC, c.engram_id ASC` makes that deterministic; the
/// hydrate reapplies it in Rust. Repeating the query must not shuffle them, and
/// neither must switching backends.
async fn an_exact_distance_tie_is_ordered_deterministically(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Same title and same body, so the chunk text (which carries the title) is
    // identical and the embeddings are equal to the bit.
    write(root, "tie-a.md", &engram("Tie", "tie-a", NEEDLE));
    write(root, "tie-b.md", &engram("Tie", "tie-b", NEEDLE));
    write(root, "tie-c.md", &engram("Tie", "tie-c", NEEDLE));
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(store, "d", root, &fake).await;

    let first: Vec<String> = store
        .search(&semantic_query(NEEDLE, &fake))
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|h| h.permalink)
        .collect();
    let second: Vec<String> = store
        .search(&semantic_query(NEEDLE, &fake))
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|h| h.permalink)
        .collect();

    assert_eq!(first.len(), 3, "all three tied engrams are returned");
    assert_eq!(first, second, "the tie order is stable across runs");
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "tie-a".to_string(),
            "tie-b".to_string(),
            "tie-c".to_string()
        ],
        "no tied engram is dropped by the top-k cut"
    );
}
parity!(
    a_semantic_distance_tie_orders_deterministically,
    an_exact_distance_tie_is_ordered_deterministically
);

/// The hybrid path runs the same split. The lexical half finds the needle by
/// term and the semantic half by vector, and the merge keys on
/// `(domain, permalink)`, so a hydrate that returned the wrong domain or
/// permalink would split one engram into two hits.
async fn hybrid_search_merges_the_hydrated_candidates(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "giant.md", &engram("Giant", "giant", &giant_body()));
    write(root, "small.md", &engram("Brief", "small", FILLER));
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(store, "d", root, &fake).await;

    let query = SearchQuery {
        mode: SearchMode::Hybrid,
        ..semantic_query(NEEDLE, &fake)
    };
    let page = store.search(&query).await.unwrap();

    assert_eq!(
        page.total, 2,
        "the two engrams merge into two hits, not four"
    );
    assert_eq!(
        page.items[0].permalink, "giant",
        "the engram matched by both signals leads"
    );
    assert_eq!(page.items[0].domain, "d");
    assert!(page.items[0].score > 0.0);
}
parity!(
    hybrid_search_merges_split_semantic_candidates,
    hybrid_search_merges_the_hydrated_candidates
);
