//! No-network tests for the embedding pipeline: chunking, and a deterministic
//! fake provider driving storage, semantic and hybrid search, the staleness
//! path, the min-similarity cutoff and the unchanged-file zero-rework guarantee.

use std::path::Path;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, EmbeddingProvider, IndexError, Result, SearchMode, SearchQuery, Store, TursoStore,
    chunk_engram, run_embedding_pass, sync_domain_with,
};

// --- chunking (no store) -----------------------------------------------------

#[test]
fn packs_paragraphs_up_to_the_budget() {
    // Five one-token-ish paragraphs, a tiny budget: they pack into a few chunks
    // rather than one per paragraph or one giant chunk.
    let body = "alpha\n\nbeta\n\ngamma\n\ndelta\n\nepsilon\n";
    let params = ChunkParams {
        model_id: "m".into(),
        max_tokens: 3,
    };
    let chunks = chunk_engram("", None, body, &params);
    assert!(
        chunks.len() > 1 && chunks.len() < 5,
        "packed into {} chunks",
        chunks.len()
    );
    // Sequences are contiguous from zero.
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(c.seq, i as i64);
    }
}

#[test]
fn code_fence_stays_one_unit() {
    // A fenced block with a blank line inside must not be split, even though the
    // blank line would otherwise be a paragraph boundary.
    let body = "intro paragraph\n\n```\nline one\n\nline two after a blank\n```\n\nafter fence\n";
    let params = ChunkParams {
        model_id: "m".into(),
        max_tokens: 10_000,
    };
    let chunks = chunk_engram("", None, body, &params);
    // With a huge budget everything packs into one chunk, and the fence survives
    // intact with its inner blank line.
    let joined = chunks.iter().map(|c| c.text.as_str()).collect::<String>();
    assert!(joined.contains("line one"));
    assert!(joined.contains("line two after a blank"));

    // With a tiny budget the fence is its own chunk and is never cut in half.
    let small = ChunkParams {
        model_id: "m".into(),
        max_tokens: 4,
    };
    let chunks = chunk_engram("", None, body, &small);
    let fence_chunk = chunks
        .iter()
        .find(|c| c.text.contains("line one"))
        .expect("a chunk holds the fence");
    assert!(
        fence_chunk.text.contains("line two after a blank"),
        "the fence is kept whole: {:?}",
        fence_chunk.text
    );
}

#[test]
fn first_chunk_gets_title_and_description() {
    let params = ChunkParams::default();
    let chunks = chunk_engram(
        "Auth Flow",
        Some("How login works"),
        "The service issues a token.\n",
        &params,
    );
    assert!(chunks[0].text.contains("Auth Flow"));
    assert!(chunks[0].text.contains("How login works"));
    assert!(chunks[0].text.contains("issues a token"));
}

#[test]
fn fingerprints_are_stable_and_model_scoped() {
    let a = ChunkParams::for_model("model-a");
    let b = ChunkParams::for_model("model-b");
    let body = "same body text\n";
    let ca1 = chunk_engram("T", None, body, &a);
    let ca2 = chunk_engram("T", None, body, &a);
    let cb = chunk_engram("T", None, body, &b);
    assert_eq!(ca1[0].text_hash, ca2[0].text_hash, "stable across calls");
    assert_ne!(
        ca1[0].text_hash, cb[0].text_hash,
        "the model id namespaces the fingerprint"
    );
}

// --- fake provider -----------------------------------------------------------

/// A deterministic, network-free provider. It hashes each word into one of eight
/// buckets and L2-normalizes, so texts that share vocabulary get similar
/// vectors: enough structure to exercise ranking, dedupe, filters and cutoffs.
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

// --- helpers -----------------------------------------------------------------

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

fn semantic_query(provider: &FakeProvider, text: &str, min_similarity: Option<f32>) -> SearchQuery {
    let qvec = embed_one(text);
    SearchQuery {
        text: Some(text.to_string()),
        mode: SearchMode::Semantic,
        query_embedding: Some(qvec),
        active_model: Some(provider.model_id().to_string()),
        min_similarity,
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    }
}

// --- store and search tests --------------------------------------------------

#[tokio::test]
async fn store_and_retrieve_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram("Alpha", "a", "", "postgres index query"),
    );
    write(
        root,
        "b.md",
        &engram("Beta", "b", "", "recipe kitchen food"),
    );
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(&store, "d", root, &fake).await;

    let coverage = store.embedding_coverage().await.unwrap();
    assert!(coverage.total_chunks >= 2);
    assert_eq!(
        coverage.embedded_chunks, coverage.total_chunks,
        "every chunk embedded"
    );
    assert_eq!(coverage.embedded_for("fake-8"), coverage.total_chunks);
    assert!(coverage.has_active_embeddings("fake-8"));
    // Nothing left to embed.
    assert!(
        store
            .chunks_needing_embedding("fake-8", None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn semantic_ranking_orders_by_similarity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "db.md",
        &engram(
            "Databases",
            "databases",
            "",
            "postgres postgres index index query query",
        ),
    );
    write(
        root,
        "cook.md",
        &engram(
            "Cooking",
            "cooking",
            "",
            "recipe recipe kitchen kitchen food food",
        ),
    );
    write(
        root,
        "sport.md",
        &engram(
            "Sports",
            "sports",
            "",
            "soccer soccer team team match match",
        ),
    );
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(&store, "d", root, &fake).await;

    let page = store
        .search(&semantic_query(&fake, "postgres index query", Some(0.0)))
        .await
        .unwrap();
    assert_eq!(
        page.items[0].permalink, "databases",
        "the database engram ranks first for a database query"
    );
    // One hit per engram (chunks collapsed to their owning engram).
    let mut perms: Vec<&str> = page.items.iter().map(|h| h.permalink.as_str()).collect();
    perms.sort();
    perms.dedup();
    assert_eq!(perms.len(), page.items.len(), "deduplicated per engram");
}

#[tokio::test]
async fn min_similarity_cutoff_drops_weak_hits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "db.md",
        &engram(
            "Databases",
            "databases",
            "",
            "postgres postgres index index query query",
        ),
    );
    write(
        root,
        "cook.md",
        &engram(
            "Cooking",
            "cooking",
            "",
            "recipe recipe kitchen kitchen food food",
        ),
    );
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(&store, "d", root, &fake).await;

    let permissive = store
        .search(&semantic_query(&fake, "postgres index query", Some(0.0)))
        .await
        .unwrap();
    let strict = store
        .search(&semantic_query(&fake, "postgres index query", Some(0.5)))
        .await
        .unwrap();
    assert!(
        strict.total < permissive.total,
        "raising the cutoff drops weakly related hits ({} < {})",
        strict.total,
        permissive.total
    );
    assert!(
        strict.items.iter().all(|h| h.permalink != "cooking"),
        "the unrelated engram is below the cutoff"
    );
    assert!(strict.items.iter().any(|h| h.permalink == "databases"));
}

#[tokio::test]
async fn hybrid_merges_dedupes_and_pushes_filters_down() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Two engrams share the query vocabulary; one is current, one is a draft.
    write(
        root,
        "current.md",
        &engram(
            "Current Auth",
            "current-auth",
            "",
            "token session token session login",
        ),
    );
    write(
        root,
        "draft.md",
        "---\ntype: engram\ntitle: Draft Auth\npermalink: draft-auth\ntags:\n  - t\nstatus: draft\nrecorded_at: 2026-01-01\n---\n\ntoken session token session login\n",
    );
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(&store, "d", root, &fake).await;

    let qvec = embed_one("token session login");
    let base = SearchQuery {
        text: Some("token session login".into()),
        mode: SearchMode::Hybrid,
        query_embedding: Some(qvec.clone()),
        active_model: Some("fake-8".into()),
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    };

    // Unfiltered: both engrams hit, each exactly once (dedupe across the lexical
    // and semantic halves).
    let both = store.search(&base).await.unwrap();
    let mut perms: Vec<&str> = both.items.iter().map(|h| h.permalink.as_str()).collect();
    perms.sort();
    assert_eq!(perms, vec!["current-auth", "draft-auth"], "both, once each");

    // Status filter pushed into the SQL of both halves: the draft is gone even
    // though it is a strong lexical and semantic match.
    let filtered = SearchQuery {
        status: Some("current".into()),
        ..base.clone()
    };
    let page = store.search(&filtered).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].permalink, "current-auth");
}

#[tokio::test]
async fn model_swap_returns_stale_embeddings_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram("Alpha", "a", "", "postgres index query"),
    );
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    sync_and_embed(&store, "d", root, &fake).await;

    // Search with a different active model: the stored embeddings are in the
    // wrong space, so semantic search refuses with a typed staleness error.
    let query = SearchQuery {
        text: Some("postgres".into()),
        mode: SearchMode::Semantic,
        query_embedding: Some(embed_one("postgres")),
        active_model: Some("other-model".into()),
        limit: 10,
        page: 1,
        ..SearchQuery::default()
    };
    let err = store.search(&query).await.unwrap_err();
    match err {
        IndexError::StaleEmbeddings {
            stored_model,
            active_model,
            embedded,
            total,
        } => {
            assert_eq!(stored_model, "fake-8");
            assert_eq!(active_model, "other-model");
            assert_eq!(embedded, 0, "nothing embedded for the active model");
            assert!(total >= 1);
        }
        other => panic!("expected StaleEmbeddings, got {other:?}"),
    }

    // Text search is unaffected by the model swap.
    let text = store.search(&SearchQuery::text("postgres")).await.unwrap();
    assert_eq!(text.total, 1);
}

#[tokio::test]
async fn unchanged_files_do_zero_reembedding_work() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..5 {
        write(
            root,
            &format!("e{i}.md"),
            &engram(
                &format!("E{i}"),
                &format!("e{i}"),
                "",
                &format!("body words {i} shared"),
            ),
        );
    }
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    let params = ChunkParams::for_model("fake-8");

    sync_domain_with(&store, "d", root, &params).await.unwrap();
    let first = run_embedding_pass(&store, &fake, |_, _| {}).await.unwrap();
    assert!(first.chunks >= 5, "first pass embeds every chunk");

    // A warm resync touches nothing on disk, so no chunk needs re-embedding.
    let report = sync_domain_with(&store, "d", root, &params).await.unwrap();
    assert_eq!(report.unchanged, 5);
    assert!(
        store
            .chunks_needing_embedding("fake-8", None)
            .await
            .unwrap()
            .is_empty(),
        "unchanged files leave no embedding work"
    );
    let second = run_embedding_pass(&store, &fake, |_, _| {}).await.unwrap();
    assert_eq!(second.chunks, 0, "zero re-embeds on a warm resync");
}

#[tokio::test]
async fn editing_one_paragraph_only_reembeds_that_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // A body with several paragraphs, chunked small so each paragraph is a chunk.
    let body = "para one alpha\n\npara two beta\n\npara three gamma\n\npara four delta";
    write(root, "a.md", &engram("A", "a", "", body));
    let store = open().await;
    let fake = FakeProvider::new("fake-8");
    let params = ChunkParams {
        model_id: "fake-8".into(),
        max_tokens: 6,
    };
    sync_domain_with(&store, "d", root, &params).await.unwrap();
    run_embedding_pass(&store, &fake, |_, _| {}).await.unwrap();
    assert!(
        store
            .chunks_needing_embedding("fake-8", None)
            .await
            .unwrap()
            .is_empty()
    );

    // Edit only the last paragraph, keeping the rest byte-identical.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let edited = "para one alpha\n\npara two beta\n\npara three gamma\n\npara four EDITED";
    write(root, "a.md", &engram("A", "a", "", edited));
    let report = sync_domain_with(&store, "d", root, &params).await.unwrap();
    assert_eq!(report.updated, 1);

    // Only the changed chunk (and the title-bearing first chunk if it moved) lost
    // its embedding; the fingerprint carry-over kept the untouched ones.
    let pending = store
        .chunks_needing_embedding("fake-8", None)
        .await
        .unwrap();
    assert!(
        !pending.is_empty(),
        "the edited paragraph needs re-embedding"
    );
    let coverage = store.embedding_coverage().await.unwrap();
    assert!(
        coverage.embedded_chunks >= coverage.total_chunks.saturating_sub(2),
        "most chunks kept their embedding across the edit ({} of {})",
        coverage.embedded_chunks,
        coverage.total_chunks
    );
}
