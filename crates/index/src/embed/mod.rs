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
use crate::store::{EmbeddingRow, Store};

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

/// Embed every chunk that needs it for the active provider's model, in batches,
/// writing the vectors back through the store. `progress` is called after each
/// batch with `(done, total)`.
///
/// This is the synchronous fill used by `sync --embed` and `reindex --embed`.
/// The M5 daemon reuses the same batching from its background queue.
pub async fn run_embedding_pass(
    store: &dyn Store,
    provider: &dyn EmbeddingProvider,
    mut progress: impl FnMut(usize, usize),
) -> Result<EmbedReport> {
    let jobs = store.chunks_needing_embedding(provider.model_id()).await?;
    let total = jobs.len();
    if total == 0 {
        return Ok(EmbedReport::default());
    }

    let mut done = 0usize;
    let mut batches = 0usize;
    for batch in jobs.chunks(EMBED_BATCH_SIZE) {
        let texts: Vec<String> = batch.iter().map(|j| j.text.clone()).collect();
        let vectors = provider.embed(&texts).await?;
        if vectors.len() != batch.len() {
            return Err(IndexError::Embedding(format!(
                "provider returned {} vectors for {} inputs",
                vectors.len(),
                batch.len()
            )));
        }
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
    Ok(EmbedReport {
        chunks: total,
        batches,
    })
}
