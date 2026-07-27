//! The embedding pipeline: the provider trait, its local and remote
//! implementations, chunking and the batch executor that fills the index.
//!
//! A provider turns text into unit-normalized vectors. The default is a local
//! bge model run on CPU with candle (behind the `local-embeddings` feature); the
//! alternative is any OpenAI-compatible `/embeddings` endpoint. The [`Store`]
//! itself never depends on a provider: callers embed the query and hand the
//! vector to [`crate::SearchQuery`], and the batch executor embeds chunk text and
//! writes it back through [`Store::store_embeddings`].

pub mod chunk;
mod remote;

#[cfg(feature = "local-embeddings")]
mod local;

use std::path::PathBuf;

use async_trait::async_trait;
use crystalline_core::config::EmbeddingsConfig;

use crate::error::{IndexError, Result};
use crate::store::{ChunkJob, EmbeddingRow, Store};

pub use chunk::{
    ChunkParams, DEFAULT_MAX_TOKENS, DEFAULT_MODEL_ID, chunk_engram, chunk_engram_with,
    estimate_tokens, fingerprint,
};

/// The bge query instruction prefix. bge embeds documents bare but expects a
/// short instruction in front of a search query; the provider applies it in
/// [`EmbeddingProvider::embed_queries`].
pub const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// How many chunks are embedded per provider call.
pub const EMBED_BATCH_SIZE: usize = 16;

/// How many outstanding chunks one backlog page pulls from the store. An embed
/// pass loops pages instead of snapshotting the whole backlog, so a first index
/// over a large corpus never holds every chunk's text at once. A multiple of
/// [`EMBED_BATCH_SIZE`] so a full page splits into whole batches.
pub const EMBED_PAGE_SIZE: usize = 512;

/// Turns text into unit-normalized embedding vectors.
///
/// `embed` is for documents (chunk text, embedded bare). `embed_queries` is for
/// search queries; its default just calls `embed`, and a model that wants a
/// query instruction prefix (bge) overrides it.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed document texts, returning one unit-normalized vector per input in
    /// order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// The model identifier stored against every embedding and folded into chunk
    /// fingerprints.
    fn model_id(&self) -> &str;

    /// The embedding dimensionality. A remote provider may only know this after
    /// its first response, reporting `0` until then.
    fn dims(&self) -> usize;

    /// The maximum input length in tokens, used to size chunk packing.
    fn max_input_tokens(&self) -> usize;

    /// Embed search-query texts. The default falls back to [`Self::embed`]; bge
    /// overrides it to add the query instruction prefix.
    async fn embed_queries(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts).await
    }
}

/// The model id implied by a config, defaulting to the local model when none is
/// configured. The chunker and the provider must agree on this string so
/// fingerprints computed at sync time match the model that later embeds them.
pub fn configured_model_id(cfg: Option<&EmbeddingsConfig>) -> String {
    match cfg {
        Some(c) if !c.model.trim().is_empty() => c.model.clone(),
        _ => DEFAULT_MODEL_ID.to_string(),
    }
}

/// Build a provider from its configuration. The local provider loads the model
/// (downloading it on first use); the remote provider validates its endpoint and
/// API key. A `local` provider on a build without the `local-embeddings` feature
/// is an [`IndexError::Unsupported`].
pub async fn provider_from_config(cfg: &EmbeddingsConfig) -> Result<Box<dyn EmbeddingProvider>> {
    match cfg.provider.as_str() {
        "local" => build_local(cfg).await,
        "openai-compatible" | "openai" | "remote" => {
            Ok(Box::new(remote::RemoteProvider::from_config(cfg)?))
        }
        other => Err(IndexError::Invalid(format!(
            "unknown embeddings provider '{other}' (expected 'local' or 'openai-compatible')"
        ))),
    }
}

#[cfg(feature = "local-embeddings")]
async fn build_local(cfg: &EmbeddingsConfig) -> Result<Box<dyn EmbeddingProvider>> {
    Ok(Box::new(local::LocalProvider::load(cfg).await?))
}

#[cfg(not(feature = "local-embeddings"))]
async fn build_local(_cfg: &EmbeddingsConfig) -> Result<Box<dyn EmbeddingProvider>> {
    Err(IndexError::Unsupported(
        "this build has no local embedding support; rebuild with the 'local-embeddings' feature or configure an 'openai-compatible' provider".into(),
    ))
}

/// The outcome of pre-fetching the local model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDownload {
    /// The on-disk model directory.
    pub path: PathBuf,
    /// The total size of the fetched model files in bytes.
    pub bytes: u64,
}

/// Pre-fetch the local embedding model into the cache, for offline or CI use.
/// Errors (including "built without local support") are returned so the CLI can
/// exit non-zero.
pub async fn download_local_model(cfg: &EmbeddingsConfig) -> Result<ModelDownload> {
    #[cfg(feature = "local-embeddings")]
    {
        local::download(cfg).await
    }
    #[cfg(not(feature = "local-embeddings"))]
    {
        let _ = cfg;
        Err(IndexError::Unsupported(
            "this build has no local embedding support; rebuild with the 'local-embeddings' feature".into(),
        ))
    }
}

/// The outcome of an embedding pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbedReport {
    /// Chunks embedded in this pass.
    pub chunks: usize,
    /// Provider calls made.
    pub batches: usize,
}

/// Order jobs ascending by estimated token count before batching.
///
/// The local tokenizer pads every batch to its longest member
/// (`PaddingStrategy::BatchLongest`), so a batch that mixes one long chunk into
/// fifteen short ones pays the long sequence's cost sixteen times over. Sorting
/// first makes batches length-homogeneous and cuts that padding waste on full
/// passes. Correctness never depends on batch order: each vector is written
/// back against its own `chunk_id`. The sort is stable so equal-length jobs
/// keep the order the store returned them in.
pub fn order_jobs_for_batching(jobs: &mut [ChunkJob]) {
    // Cache each job's token estimate so the counter runs once per job rather
    // than once per comparison; the sort stays stable so equal-length jobs keep
    // the store's order.
    jobs.sort_by_cached_key(|job| estimate_tokens(&job.text));
}

/// Embed every chunk that needs it for the active provider's model, in batches,
/// writing the vectors back through the store. `progress` is called after each
/// batch with `(done, total)`, where `total` is every chunk pulled so far: the
/// backlog is paged, so the final call reports the true total and the earlier
/// ones a lower bound.
///
/// A batch the provider rejects is logged and skipped, not fatal: its chunks
/// keep no embedding and stay in the backlog for a later pass, so one poisoned
/// batch cannot starve everything queued behind it. Only store errors abort.
///
/// This is the synchronous fill used by `sync --embed` and `reindex --embed`.
/// The M5 daemon reuses the same batching from its background queue.
pub async fn run_embedding_pass(
    store: &dyn Store,
    provider: &dyn EmbeddingProvider,
    progress: impl FnMut(usize, usize),
) -> Result<EmbedReport> {
    run_embedding_pass_with_page(store, provider, EMBED_PAGE_SIZE, progress).await
}

/// [`run_embedding_pass`] with an explicit backlog page size. Production callers
/// take [`EMBED_PAGE_SIZE`] through the wrapper; the parameter lets a test drive
/// several pages over a small corpus.
pub async fn run_embedding_pass_with_page(
    store: &dyn Store,
    provider: &dyn EmbeddingProvider,
    page_size: usize,
    mut progress: impl FnMut(usize, usize),
) -> Result<EmbedReport> {
    let page_size = page_size.max(1);
    let mut done = 0usize;
    let mut batches = 0usize;
    let mut cursor: Option<(i64, i64)> = None;
    loop {
        // The standalone `sync --embed` / `reindex --embed` fill embeds every
        // domain (`None`); the daemon's background queue scopes its own pass.
        let mut jobs = store
            .chunks_needing_embedding(provider.model_id(), None, page_size, cursor)
            .await?;
        if jobs.is_empty() {
            break;
        }
        // A short page is the last one. The cursor is taken from the store's
        // ordering, before the length sort reorders the page.
        let last_page = jobs.len() < page_size;
        cursor = jobs.last().map(|j| (j.engram_id, j.seq));
        order_jobs_for_batching(&mut jobs);
        let total = done + jobs.len();

        for batch in jobs.chunks(EMBED_BATCH_SIZE) {
            let texts: Vec<String> = batch.iter().map(|j| j.text.clone()).collect();
            // A batch the provider cannot handle is logged and skipped, never
            // fatal: its chunks keep no embedding, so they stay in the backlog
            // for a later pass instead of starving every batch behind them.
            let vectors = match provider.embed(&texts).await {
                Ok(v) if v.len() == batch.len() => v,
                Ok(v) => {
                    tracing::warn!(
                        chunks = ?batch.iter().map(|j| j.chunk_id).collect::<Vec<_>>(),
                        "skipping an embed batch: the provider returned {} vectors for {} inputs",
                        v.len(),
                        batch.len()
                    );
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        chunks = ?batch.iter().map(|j| j.chunk_id).collect::<Vec<_>>(),
                        "skipping an embed batch the provider rejected: {e}"
                    );
                    continue;
                }
            };
            let rows: Vec<EmbeddingRow> = batch
                .iter()
                .zip(vectors)
                .map(|(job, embedding)| EmbeddingRow {
                    chunk_id: job.chunk_id,
                    dims: embedding.len(),
                    embedding,
                })
                .collect();
            store.store_embeddings(&rows, provider.model_id()).await?;
            done += batch.len();
            batches += 1;
            progress(done, total);
        }
        if last_page {
            break;
        }
    }
    Ok(EmbedReport {
        chunks: done,
        batches,
    })
}
