//! The local embedding provider: BAAI/bge-small-en-v1.5 on CPU via candle.
//!
//! The model, tokenizer and config are fetched with hf-hub into Crystalline's
//! own model cache on first use (not hf-hub's default location). bge produces a
//! sentence embedding from the `[CLS]` token followed by L2 normalization, and it
//! expects a short instruction prefix in front of a search query but embeds
//! documents bare; both are handled here. Inference is CPU only (no metal or cuda
//! features) so the release binaries stay portable, and it runs on a blocking
//! thread so it never stalls the async runtime. A load failure from a truncated
//! or corrupt cache self-heals: the model directory is wiped and fetched once
//! more before giving up.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use crystalline_core::config::{self, EmbeddingsConfig};
use hf_hub::api::Progress;
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Cache, Repo, RepoType};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

use super::{BGE_QUERY_PREFIX, DEFAULT_MODEL_ID, EmbeddingProvider};
use crate::error::{IndexError, Result};

/// The Hugging Face repository the weights come from.
const HF_REPO: &str = "BAAI/bge-small-en-v1.5";
/// bge-small-en-v1.5 embedding width.
const DIMS: usize = 384;
/// The model's maximum input length in tokens.
const MAX_INPUT_TOKENS: usize = 512;

/// A locally hosted bge provider.
pub struct LocalProvider {
    inner: Arc<Bert>,
    model_id: String,
}

/// The loaded model, tokenizer and device, shared into the blocking inference
/// task.
struct Bert {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl LocalProvider {
    /// Load the provider, downloading the model on first use. Runs on a blocking
    /// thread because loading mmaps and parses the weights.
    pub async fn load(cfg: &EmbeddingsConfig) -> Result<LocalProvider> {
        let model_id = if cfg.model.trim().is_empty() {
            DEFAULT_MODEL_ID.to_string()
        } else {
            cfg.model.clone()
        };
        let cache_dir = models_cache_dir()?;
        let id_for_task = model_id.clone();
        let bert = tokio::task::spawn_blocking(move || load_bert(&cache_dir, &id_for_task))
            .await
            .map_err(|e| IndexError::Embedding(format!("model load task failed: {e}")))??;
        Ok(LocalProvider {
            inner: Arc::new(bert),
            model_id,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for LocalProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let inner = self.inner.clone();
        let texts = texts.to_vec();
        tokio::task::spawn_blocking(move || embed_texts(&inner, &texts))
            .await
            .map_err(|e| IndexError::Embedding(format!("embedding task failed: {e}")))?
    }

    async fn embed_queries(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // bge expects the query instruction prefix; documents are embedded bare.
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format!("{BGE_QUERY_PREFIX}{t}"))
            .collect();
        self.embed(&prefixed).await
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dims(&self) -> usize {
        DIMS
    }

    fn max_input_tokens(&self) -> usize {
        MAX_INPUT_TOKENS
    }
}

/// Pre-fetch the model files and report the cache location and size.
pub async fn download(_cfg: &EmbeddingsConfig) -> Result<super::ModelDownload> {
    let cache_dir = models_cache_dir()?;
    tokio::task::spawn_blocking(move || {
        let files = ensure_files(&cache_dir)?;
        let bytes = [&files.config, &files.tokenizer, &files.weights]
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        let path = files
            .weights
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(cache_dir);
        Ok(super::ModelDownload { path, bytes })
    })
    .await
    .map_err(|e| IndexError::Embedding(format!("model download task failed: {e}")))?
}

fn models_cache_dir() -> Result<PathBuf> {
    config::models_dir().map_err(|e| IndexError::Embedding(format!("model cache dir: {e}")))
}

/// The fetched file paths for the model repo.
struct ModelFiles {
    config: PathBuf,
    tokenizer: PathBuf,
    weights: PathBuf,
}

/// Adapts hf-hub's per-chunk [`Progress`] callbacks (one `update` call per
/// ~8 KiB read, `std::io::copy`'s default buffer) into a single
/// carriage-return byte-progress line on stderr, throttled to about ten
/// renders a second so a fast local connection does not flood the terminal
/// with one write per chunk. Only ever constructed when stderr is a live
/// terminal (see [`ensure_files`]), so it never needs to check that itself.
struct ByteProgress {
    filename: String,
    total: usize,
    downloaded: usize,
    last_render: Option<Instant>,
}

impl ByteProgress {
    fn new() -> Self {
        ByteProgress {
            filename: String::new(),
            total: 0,
            downloaded: 0,
            last_render: None,
        }
    }

    fn render(&self) {
        let mb = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
        eprint!(
            "\r  {}: {:.1} / {:.1} MB",
            self.filename,
            mb(self.downloaded),
            mb(self.total)
        );
        let _ = std::io::stderr().flush();
    }
}

impl Progress for ByteProgress {
    fn init(&mut self, size: usize, filename: &str) {
        self.total = size;
        self.downloaded = 0;
        self.filename = filename.to_string();
        self.last_render = Some(Instant::now());
        self.render();
    }

    fn update(&mut self, size: usize) {
        self.downloaded += size;
        let now = Instant::now();
        let due = self
            .last_render
            .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100));
        if due {
            self.last_render = Some(now);
            self.render();
        }
    }

    fn finish(&mut self) {
        // A final render so the line lands on the true total even when the
        // last chunk landed inside the throttle window, then a newline so
        // whatever prints next starts clean instead of overwriting this line.
        self.render();
        eprintln!();
    }
}

/// Fetch `config.json`, `tokenizer.json` and `model.safetensors` into the
/// cache, announcing a first-use download once to stderr. When the whole
/// download is needed (nothing cached yet) and stderr is a live terminal, each
/// file also gets a `\r`-updated byte-progress line via [`ByteProgress`];
/// piped or redirected stderr (a log file, `--json`'s non-interactive
/// callers, CI) keeps exactly the single notice line, never per-byte output.
/// A file already present in the cache is resolved with no network call at
/// all, so a fully warmed cache - the air-gapped and CI-prefetch paths -
/// never dials out just to check.
fn ensure_files(cache_dir: &Path) -> Result<ModelFiles> {
    std::fs::create_dir_all(cache_dir).map_err(|e| IndexError::Io {
        path: cache_dir.display().to_string(),
        source: e,
    })?;

    let hub_cache =
        Cache::new(cache_dir.to_path_buf()).repo(Repo::new(HF_REPO.to_string(), RepoType::Model));
    let cached = hub_cache.get("model.safetensors").is_some();
    if !cached {
        eprintln!(
            "crystalline: downloading embedding model {HF_REPO} to {} (first use, about 130 MB)...",
            cache_dir.display()
        );
    }
    let show_progress = !cached && std::io::stderr().is_terminal();

    let api = ApiBuilder::new()
        .with_cache_dir(cache_dir.to_path_buf())
        // Ours only: a stable, testable byte counter instead of hf-hub's own
        // default indicatif bar, so exactly one progress mechanism is ever
        // active and it is the one this module controls and TTY-gates itself.
        .with_progress(false)
        .build()
        .map_err(|e| IndexError::Embedding(format!("hub client: {e}")))?;
    let repo = api.model(HF_REPO.to_string());
    let fetch = |name: &str| -> Result<PathBuf> {
        if let Some(path) = hub_cache.get(name) {
            return Ok(path);
        }
        if show_progress {
            repo.download_with_progress(name, ByteProgress::new())
        } else {
            repo.download(name)
        }
        .map_err(|e| IndexError::Embedding(format!("downloading {name}: {e}")))
    };
    Ok(ModelFiles {
        config: fetch("config.json")?,
        tokenizer: fetch("tokenizer.json")?,
        weights: fetch("model.safetensors")?,
    })
}

/// Load the model, self-healing once from a corrupt cache.
fn load_bert(cache_dir: &Path, _model_id: &str) -> Result<Bert> {
    let files = ensure_files(cache_dir)?;
    match build_bert(&files) {
        Ok(bert) => Ok(bert),
        Err(first) => {
            // A truncated or corrupt cache: wipe the model directory and fetch
            // once more before surfacing the failure.
            eprintln!(
                "crystalline: embedding model failed to load ({first}); re-downloading once..."
            );
            wipe_model_dir(cache_dir);
            let files = ensure_files(cache_dir)?;
            build_bert(&files)
        }
    }
}

fn build_bert(files: &ModelFiles) -> Result<Bert> {
    let config_text = std::fs::read_to_string(&files.config).map_err(|e| IndexError::Io {
        path: files.config.display().to_string(),
        source: e,
    })?;
    let config: Config = serde_json::from_str(&config_text)
        .map_err(|e| IndexError::Embedding(format!("parsing config.json: {e}")))?;

    let mut tokenizer = Tokenizer::from_file(&files.tokenizer)
        .map_err(|e| IndexError::Embedding(format!("loading tokenizer.json: {e}")))?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        ..PaddingParams::default()
    }));
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: MAX_INPUT_TOKENS,
            ..TruncationParams::default()
        }))
        .map_err(|e| IndexError::Embedding(format!("configuring truncation: {e}")))?;

    let device = Device::Cpu;
    // Safety: the file is a trusted, freshly verified download; mmap is the
    // standard candle load path.
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(
            std::slice::from_ref(&files.weights),
            DType::F32,
            &device,
        )
        .map_err(|e| IndexError::Embedding(format!("loading weights: {e}")))?
    };
    let model = BertModel::load(vb, &config)
        .map_err(|e| IndexError::Embedding(format!("building model: {e}")))?;

    Ok(Bert {
        model,
        tokenizer,
        device,
    })
}

fn embed_texts(bert: &Bert, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let encodings = bert
        .tokenizer
        .encode_batch(texts.to_vec(), true)
        .map_err(|e| IndexError::Embedding(format!("tokenizing: {e}")))?;

    let batch = encodings.len();
    let seq_len = encodings.first().map(|e| e.get_ids().len()).unwrap_or(0);
    let mut ids: Vec<u32> = Vec::with_capacity(batch * seq_len);
    let mut mask: Vec<u32> = Vec::with_capacity(batch * seq_len);
    for enc in &encodings {
        ids.extend_from_slice(enc.get_ids());
        mask.extend_from_slice(enc.get_attention_mask());
    }

    let compute = || -> candle_core::Result<Vec<Vec<f32>>> {
        let input_ids = Tensor::from_vec(ids, (batch, seq_len), &bert.device)?;
        let attention = Tensor::from_vec(mask, (batch, seq_len), &bert.device)?;
        let token_type = input_ids.zeros_like()?;
        let sequence = bert
            .model
            .forward(&input_ids, &token_type, Some(&attention))?;
        // bge sentence embedding: the [CLS] token (position 0), then L2 norm.
        let cls = sequence.narrow(1, 0, 1)?.squeeze(1)?;
        let normalized = normalize_l2(&cls)?;
        normalized.to_vec2::<f32>()
    };
    compute().map_err(|e| IndexError::Embedding(format!("inference: {e}")))
}

fn normalize_l2(v: &Tensor) -> candle_core::Result<Tensor> {
    v.broadcast_div(&v.sqr()?.sum_keepdim(1)?.sqrt()?)
}

fn wipe_model_dir(cache_dir: &Path) {
    let dir = cache_dir.join(format!("models--{}", HF_REPO.replace('/', "--")));
    let _ = std::fs::remove_dir_all(&dir);
}
