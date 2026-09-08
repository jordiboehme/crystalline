//! `Store::lead_vectors` on both backends: one row per engram whose first
//! chunk carries an embedding for the model asked about, the vector as stored,
//! ordered by engram id, and nothing for a model that never embedded here.
//!
//! The `FakeProvider` harness, the `write`/`engram`/`sync_and_embed` helpers
//! and the `parity!` backend runner are mirrored from `tests/retired.rs`
//! (Turso always, Postgres when `CRYSTALLINE_TEST_POSTGRES_URL` is set).

use std::path::Path;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, DomainId, DomainKind, EmbeddingProvider, Result, Store, TursoStore,
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

// --- helpers (mirrored from tests/retired.rs) ---------------------------------

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

// --- backend runner (mirrored from tests/retired.rs) --------------------------

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

// --- tests --------------------------------------------------------------------

/// Two engrams, one current and one superseded, both embedded by the active
/// model. Both come back: the retirement filter belongs to the sweep, not to
/// the store. The vectors arrive exactly as stored, one row per engram rather
/// than one per chunk, and a model that never embedded here yields nothing.
async fn lead_vectors_are_one_per_engram_for_the_active_model(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "alpha.md",
        &engram(
            "Alpha",
            "alpha",
            "stable",
            "",
            "The retry queue doubles its backoff on every failure.",
        ),
    );
    write(
        root,
        "beta.md",
        &engram(
            "Beta",
            "beta",
            "superseded",
            "",
            "Clamp three reads locked before it seats.",
        ),
    );
    let provider = FakeProvider::new("fake");
    sync_and_embed(store, "notes", root, &provider).await;
    let domain = store
        .upsert_domain("notes", Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap();

    let rows = store.lead_vectors(domain, "fake").await.unwrap();
    assert_eq!(rows.len(), 2, "alpha and beta, retired included: {rows:?}");
    assert!(
        rows.windows(2).all(|w| w[0].engram_id.0 < w[1].engram_id.0),
        "ordered by id"
    );
    for row in &rows {
        assert_eq!(row.dims, 8);
        assert_eq!(row.vector.len(), 8);
        let norm: f32 = row.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-3,
            "unit vectors come back as stored"
        );
    }
    let alpha = store.find_engram("notes", "alpha").await.unwrap().unwrap();
    let lead = rows.iter().find(|r| r.engram_id == alpha.id).unwrap();
    let expected = embed_one("Alpha\n\nThe retry queue doubles its backoff on every failure.");
    let dot: f32 = lead.vector.iter().zip(&expected).map(|(a, b)| a * b).sum();
    assert!(
        (dot - 1.0).abs() < 1e-3,
        "the lead vector is chunk 0's embedding: {dot}"
    );

    assert!(
        store
            .lead_vectors(domain, "other-model")
            .await
            .unwrap()
            .is_empty(),
        "a model that never embedded here has no lead vectors"
    );
    assert!(
        store
            .lead_vectors(DomainId(9999), "fake")
            .await
            .unwrap()
            .is_empty(),
        "an unknown domain has no lead vectors"
    );
}
parity!(
    lead_vectors_parity,
    lead_vectors_are_one_per_engram_for_the_active_model
);

/// The projection is the lead chunk, not the chunk table: an engram long
/// enough to split into several embedded chunks still yields exactly one row.
/// An engram synced after the embedding pass has no embedding on chunk 0 yet
/// and is absent until it is embedded.
async fn lead_vectors_are_one_per_engram_and_skip_the_unembedded(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Two paragraphs, each well past the 450-token chunk budget, so the engram
    // is chunked into more than one embedded chunk.
    let long = "the retry queue drains slowly ".repeat(80);
    write(
        root,
        "gamma.md",
        &engram("Gamma", "gamma", "stable", "", &format!("{long}\n\n{long}")),
    );
    let provider = FakeProvider::new("fake");
    sync_and_embed(store, "notes", root, &provider).await;

    let domain = store
        .upsert_domain("notes", Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap();
    let gamma = store.find_engram("notes", "gamma").await.unwrap().unwrap();
    let embedded = store
        .embedding_coverage()
        .await
        .unwrap()
        .embedded_for("fake");
    assert!(
        embedded > 1,
        "the fixture must split into several embedded chunks, got {embedded}"
    );

    // Synced but not embedded: its chunk 0 carries no vector.
    write(
        root,
        "delta.md",
        &engram("Delta", "delta", "stable", "", "Clamp three reads locked."),
    );
    sync_domain_with(store, "notes", root, &ChunkParams::for_model("fake"))
        .await
        .unwrap();
    let delta = store.find_engram("notes", "delta").await.unwrap().unwrap();

    let rows = store.lead_vectors(domain, "fake").await.unwrap();
    assert_eq!(
        rows.iter().filter(|r| r.engram_id == gamma.id).count(),
        1,
        "one row per engram, never one per chunk: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.engram_id == delta.id),
        "an engram whose chunk 0 has no embedding is not listed"
    );
    assert_eq!(rows.len(), 1, "gamma alone: {rows:?}");
}
parity!(
    lead_vectors_skip_unembedded_parity,
    lead_vectors_are_one_per_engram_and_skip_the_unembedded
);
