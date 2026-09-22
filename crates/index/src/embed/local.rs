//! The local embedding provider: one of the models in [`crate::embed::models`]
//! on CPU via candle.
//!
//! The model files are fetched with hf-hub into Crystalline's own model cache
//! on first use (not hf-hub's default location). Which encoder the weights load
//! into and what a search query is prefixed with come from the model's table
//! entry: bge runs through candle's stock `bert` and wants a short instruction
//! in front of a query, granite runs through the vendored [`super::modernbert`]
//! and wants nothing in front of anything. Both produce a sentence embedding
//! from the `[CLS]` position followed by L2 normalization, and documents are
//! embedded bare under either. Inference is CPU only (no metal or cuda
//! features) so the release binaries stay portable, and both it and the weight
//! load run on a blocking thread so neither stalls the async runtime; the
//! download itself is async and is awaited before that thread starts. A load
//! failure from a truncated or corrupt cache self-heals: the model directory is
//! wiped and fetched once more before giving up.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use crystalline_core::config::{self, EmbeddingsConfig};
use hf_hub::progress::{DownloadEvent, ProgressEvent, ProgressHandler};
use hf_hub::{HFClient, HFError};
use indexmap::IndexMap;
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

use super::models::{Architecture, LocalModel, lookup_local_model};
use super::modernbert::{Config as ModernBertConfig, ModernBert};
use super::{DEFAULT_MODEL_ID, EmbeddingProvider};
use crate::error::{IndexError, Result};

/// The model's maximum input length in tokens. Both table entries truncate
/// here: the chunker never produces more, and granite's 32k positions
/// notwithstanding the cap is what bounds the tokenizer's own allocation.
const MAX_INPUT_TOKENS: usize = 512;
/// The hard character cap applied to every input before tokenization. The
/// tokenizer materializes the full `Encoding` (ids, tokens, offsets and masks,
/// tens of bytes per token) and only then truncates to [`MAX_INPUT_TOKENS`], so
/// an oversized input costs memory proportional to its length. Six characters
/// per token is well above any real ratio, so this never cuts a chunk the
/// chunker produced; it only bounds a caller that skipped chunking.
const MAX_INPUT_CHARS: usize = MAX_INPUT_TOKENS * 6;

/// A locally hosted provider, running the model its configuration named.
pub struct LocalProvider {
    inner: Arc<Encoder>,
    model: &'static LocalModel,
}

/// The loaded model, tokenizer and device, shared into the blocking inference
/// task.
struct Encoder {
    loaded: Loaded,
    tokenizer: Tokenizer,
    device: Device,
}

/// The two encoders behind one seam. Both return `(batch, seq, hidden)`.
enum Loaded {
    Bert(BertModel),
    ModernBert(ModernBert),
}

impl LocalProvider {
    /// Load the provider, downloading the model on first use. The fetch is
    /// awaited; the weight load runs on a blocking thread because it mmaps and
    /// parses the weights.
    pub async fn load(cfg: &EmbeddingsConfig) -> Result<LocalProvider> {
        // Refused here rather than guessed: a model whose architecture and
        // prefix the code does not know produces vectors that are quietly
        // wrong.
        let model = lookup_local_model(configured_or_default(cfg))?;
        let cache_dir = models_cache_dir()?;
        let encoder = load_encoder(&cache_dir, model).await?;
        Ok(LocalProvider {
            inner: Arc::new(encoder),
            model,
        })
    }

    /// The batch path both entry points share: already prefixed and capped.
    async fn run(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || embed_texts(&inner, &texts))
            .await
            .map_err(|e| IndexError::Embedding(format!("embedding task failed: {e}")))?
    }
}

#[async_trait]
impl EmbeddingProvider for LocalProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // Documents are embedded bare under both models. The character cap is
        // applied here, where the batch is first copied for the blocking task,
        // so an oversized chunk row bounds every copy downstream. See
        // MAX_INPUT_CHARS.
        let prepared: Vec<String> = texts
            .iter()
            .map(|t| cap_chars(t, MAX_INPUT_CHARS).to_string())
            .collect();
        self.run(prepared).await
    }

    async fn embed_queries(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // The table entry's query prefix: bge's instruction, granite's nothing.
        // Capped before the prefix so the prefix always survives the cap.
        let prepared: Vec<String> = texts
            .iter()
            .map(|t| self.model.query_text(cap_chars(t, MAX_INPUT_CHARS)))
            .collect();
        self.run(prepared).await
    }

    fn model_id(&self) -> &str {
        self.model.id
    }

    fn dims(&self) -> usize {
        self.model.dims
    }

    fn max_input_tokens(&self) -> usize {
        MAX_INPUT_TOKENS
    }
}

/// The model a configuration names, or the default when it names none.
fn configured_or_default(cfg: &EmbeddingsConfig) -> &str {
    let requested = cfg.model.trim();
    if requested.is_empty() {
        DEFAULT_MODEL_ID
    } else {
        requested
    }
}

/// Pre-fetch the model files and report the cache location and size.
pub async fn download(cfg: &EmbeddingsConfig) -> Result<super::ModelDownload> {
    let model = lookup_local_model(configured_or_default(cfg))?;
    let cache_dir = models_cache_dir()?;
    let files = ensure_files(&cache_dir, model).await?;
    // Metadata sizing only: a handful of stat calls, which no blocking task
    // has to carry now that the fetch itself is async.
    let bytes = files
        .paths
        .values()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    let path = files
        .weights()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(cache_dir);
    Ok(super::ModelDownload { path, bytes })
}

fn models_cache_dir() -> Result<PathBuf> {
    config::models_dir().map_err(|e| IndexError::Embedding(format!("model cache dir: {e}")))
}

/// The fetched file paths for the model repo, by file name, in fetch order.
struct ModelFiles {
    paths: IndexMap<String, PathBuf>,
}

impl ModelFiles {
    fn get(&self, name: &str) -> Option<&PathBuf> {
        self.paths.get(name)
    }

    /// A file the loader cannot do without. Missing means the table entry's
    /// file list and the loader disagree, which is a bug rather than a bad
    /// download, so it says so plainly.
    fn required(&self, name: &str) -> Result<&PathBuf> {
        self.get(name).ok_or_else(|| {
            IndexError::Embedding(format!(
                "the model's file list does not name {name}, which the loader needs"
            ))
        })
    }

    fn config(&self) -> Result<&PathBuf> {
        self.required("config.json")
    }

    fn tokenizer(&self) -> Result<&PathBuf> {
        self.required("tokenizer.json")
    }

    /// Absent for bge, which never shipped one in its three-file list.
    fn tokenizer_config(&self) -> Option<&PathBuf> {
        self.get("tokenizer_config.json")
    }

    fn weights(&self) -> Result<&PathBuf> {
        self.required("model.safetensors")
    }
}

/// Adapts hf-hub's [`ProgressHandler`] events into a single carriage-return
/// byte-progress line on stderr, throttled to about ten renders a second so a
/// fast local connection does not flood the terminal with one write per event.
/// Only ever constructed when stderr is a live terminal (see [`ensure_files`]),
/// so it never needs to check that itself.
///
/// `on_progress` takes `&self` and may be called from any task, so the counters
/// are interior-mutable. The file name is not carried by the events - a
/// download's `Start` reports totals only - so it is set at construction, where
/// the caller knows which file it asked for.
struct ByteProgress {
    filename: String,
    total: AtomicU64,
    downloaded: AtomicU64,
    last_render: Mutex<Option<Instant>>,
}

impl ByteProgress {
    fn new(filename: &str) -> Self {
        ByteProgress {
            filename: filename.to_string(),
            total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
            last_render: Mutex::new(None),
        }
    }

    fn render(&self) {
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        eprint!(
            "\r  {}: {:.1} / {:.1} MB",
            self.filename,
            mb(self.downloaded.load(Ordering::Relaxed)),
            mb(self.total.load(Ordering::Relaxed))
        );
        let _ = std::io::stderr().flush();
    }

    /// The render throttle, as one step: records this instant and answers
    /// whether the caller should draw. A poisoned lock is no reason to stop
    /// drawing a progress line.
    fn render_due(&self) -> bool {
        let now = Instant::now();
        let mut last = self
            .last_render
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100)) {
            *last = Some(now);
            true
        } else {
            false
        }
    }
}

impl ProgressHandler for ByteProgress {
    fn on_progress(&self, event: &ProgressEvent) {
        match event {
            ProgressEvent::Download(DownloadEvent::Start { total_bytes, .. }) => {
                self.total.store(*total_bytes, Ordering::Relaxed);
                self.downloaded.store(0, Ordering::Relaxed);
                // Opens the throttle window here, so the first byte event does
                // not redraw the line it has just drawn.
                self.render_due();
                self.render();
            }
            ProgressEvent::Download(DownloadEvent::Progress { files }) => {
                // One handler is attached per `download_file`, so the list is
                // this one file, and its `bytes_completed` is a running total
                // rather than a delta: stored, never added. A xet-backed file
                // (the weights are one) reports that total only when a segment
                // lands, so the line redraws ten times a second but the number
                // moves in a few big steps; hf-xet's own aggregate count sits
                // at zero just as long, so there is nothing finer to read.
                let Some(file) = files.last() else { return };
                self.downloaded
                    .store(file.bytes_completed, Ordering::Relaxed);
                if self.render_due() {
                    self.render();
                }
            }
            ProgressEvent::Download(DownloadEvent::Complete) => {
                // A final render so the line lands on the true total even when
                // the last event landed inside the throttle window, then a
                // newline so whatever prints next starts clean instead of
                // overwriting this line.
                self.render();
                eprintln!();
            }
            _ => {}
        }
    }
}

/// Fetch the model entry's own file list into the cache, announcing a
/// first-use download once to stderr. The list is per model, so a model that
/// never needed a file is never made to dial out for it. When the whole
/// download is needed (nothing cached yet) and stderr is a live terminal, each
/// file also gets a `\r`-updated byte-progress line via [`ByteProgress`];
/// piped or redirected stderr (a log file, `--json`'s non-interactive
/// callers, CI) keeps exactly the single notice line, never per-byte output.
/// A file already present in the cache is resolved with no network call at
/// all, so a fully warmed cache - the air-gapped and CI-prefetch paths -
/// never dials out just to check.
async fn ensure_files(cache_dir: &Path, model: &LocalModel) -> Result<ModelFiles> {
    std::fs::create_dir_all(cache_dir).map_err(|e| IndexError::Io {
        path: cache_dir.display().to_string(),
        source: e,
    })?;

    let client = hub_client(cache_dir)?;
    // The weights alone decide whether this is a first-use download: they are
    // the file worth a notice and a progress line.
    let cached = is_cached(&client, model).await?;
    if !cached {
        eprintln!(
            "crystalline: downloading embedding model {} to {} (first use, about {} MB)...",
            model.repo,
            cache_dir.display(),
            model.download_mb
        );
    }
    let show_progress = !cached && std::io::stderr().is_terminal();

    let (owner, name) = repo_parts(model)?;
    let repo = client.model(owner, name);
    let mut paths = IndexMap::with_capacity(model.files.len());
    for file in model.files {
        let path = match cached_path(&client, model, file).await? {
            Some(path) => path,
            None => repo
                .download_file()
                .filename(*file)
                // Progress is opt in in hf-hub: leaving the handler off is the
                // suppression, so exactly one progress mechanism is ever active
                // and it is the one this module controls and TTY-gates itself.
                .maybe_progress(show_progress.then(|| ByteProgress::new(file)))
                .send()
                .await
                .map_err(|e| IndexError::Embedding(format!("downloading {file}: {e}")))?,
        };
        paths.insert((*file).to_string(), path);
    }
    Ok(ModelFiles { paths })
}

/// `("BAAI", "bge-small-en-v1.5")` from a table entry's repository id, which is
/// the pair `HFClient::model` takes. A refusal rather than a guess: hf-hub's own
/// `split_id` answers an empty owner for a malformed id, which would silently
/// resolve to a cache directory [`LocalModel::cache_dir_name`] does not spell.
fn repo_parts(model: &LocalModel) -> Result<(&'static str, &'static str)> {
    model.repo.split_once('/').ok_or_else(|| {
        IndexError::Embedding(format!(
            "the model table's repository id {} is not <owner>/<name>",
            model.repo
        ))
    })
}

/// The hf-hub client, cache-pinned to Crystalline's own model directory rather
/// than hf-hub's default location.
fn hub_client(cache_dir: &Path) -> Result<HFClient> {
    HFClient::builder()
        .cache_dir(cache_dir.to_path_buf())
        .build()
        .map_err(|e| IndexError::Embedding(format!("hub client: {e}")))
}

/// The cached path for one of the model's files, or `None` when the cache does
/// not hold it. `local_files_only` answers from the cache directory alone and
/// never touches the network, and `LocalEntryNotFound` is the miss; a plain
/// download would HEAD the repository even on a warm cache, which is what the
/// air-gapped and CI-prefetch paths must not do.
async fn cached_path(client: &HFClient, model: &LocalModel, name: &str) -> Result<Option<PathBuf>> {
    let (owner, repo) = repo_parts(model)?;
    match client
        .model(owner, repo)
        .download_file()
        .filename(name)
        .local_files_only(true)
        .send()
        .await
    {
        Ok(path) => Ok(Some(path)),
        Err(HFError::LocalEntryNotFound { .. }) => Ok(None),
        Err(e) => Err(IndexError::Embedding(format!(
            "reading the model cache for {name}: {e}"
        ))),
    }
}

/// True when the weights are already in the cache, with no network call at all.
async fn is_cached(client: &HFClient, model: &LocalModel) -> Result<bool> {
    Ok(cached_path(client, model, "model.safetensors")
        .await?
        .is_some())
}

/// Load the model, self-healing once from a corrupt cache. The fetch is awaited
/// here; only the weight load goes to a blocking thread.
async fn load_encoder(cache_dir: &Path, model: &'static LocalModel) -> Result<Encoder> {
    let files = ensure_files(cache_dir, model).await?;
    match build_on_blocking(files, model).await {
        Ok(encoder) => Ok(encoder),
        Err(first) => {
            // A truncated or corrupt cache: wipe the model directory and fetch
            // once more before surfacing the failure.
            eprintln!(
                "crystalline: embedding model failed to load ({first}); re-downloading once..."
            );
            wipe_model_dir(cache_dir, model);
            let files = ensure_files(cache_dir, model).await?;
            build_on_blocking(files, model).await
        }
    }
}

/// [`build_encoder`] on a blocking thread: it mmaps and parses the weights.
async fn build_on_blocking(files: ModelFiles, model: &'static LocalModel) -> Result<Encoder> {
    tokio::task::spawn_blocking(move || build_encoder(&files, model))
        .await
        .map_err(|e| IndexError::Embedding(format!("model load task failed: {e}")))?
}

fn build_encoder(files: &ModelFiles, model: &LocalModel) -> Result<Encoder> {
    let config_text = read(files.config()?)?;
    // A cache holding another model's files fails here, by name, rather than
    // deep inside candle on a missing tensor.
    check_model_type(&config_text, model)?;

    let mut tokenizer = Tokenizer::from_file(files.tokenizer()?)
        .map_err(|e| IndexError::Embedding(format!("loading tokenizer.json: {e}")))?;
    let tokenizer_config_text = match files.tokenizer_config() {
        Some(path) => read(path)?,
        None => "{}".to_string(),
    };
    let pad = pad_id(&tokenizer, &tokenizer_config_text, &config_text)?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        pad_id: pad,
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
    // standard candle load path. BF16 weights (granite) become F32 here.
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(
            std::slice::from_ref(files.weights()?),
            DType::F32,
            &device,
        )
        .map_err(|e| IndexError::Embedding(format!("loading weights: {e}")))?
    };
    let loaded = match model.architecture {
        Architecture::Bert => {
            let config: BertConfig = parse_config(&config_text)?;
            Loaded::Bert(BertModel::load(vb, &config).map_err(build_error)?)
        }
        Architecture::ModernBert => {
            let config: ModernBertConfig = parse_config(&config_text)?;
            Loaded::ModernBert(ModernBert::load(vb, &config).map_err(build_error)?)
        }
    };

    Ok(Encoder {
        loaded,
        tokenizer,
        device,
    })
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| IndexError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

fn parse_config<T: serde::de::DeserializeOwned>(config_json: &str) -> Result<T> {
    serde_json::from_str(config_json)
        .map_err(|e| IndexError::Embedding(format!("parsing config.json: {e}")))
}

fn build_error(e: candle_core::Error) -> IndexError {
    IndexError::Embedding(format!("building model: {e}"))
}

/// The `model_type` in `config.json` must be the table entry's.
fn check_model_type(config_json: &str, model: &LocalModel) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(config_json)
        .map_err(|e| IndexError::Embedding(format!("parsing config.json: {e}")))?;
    match value.get("model_type").and_then(|v| v.as_str()) {
        Some(found) if found == model.model_type() => Ok(()),
        found => Err(IndexError::Embedding(format!(
            "the cached config.json for {} declares model_type {:?}, expected \"{}\"; the model cache holds the wrong files",
            model.repo,
            found,
            model.model_type()
        ))),
    }
}

/// The pad id: the tokenizer's id for `tokenizer_config.json`'s `pad_token`,
/// which has to agree with `config.json`'s `pad_token_id`. The default of 0 is
/// a real content token in granite's vocabulary (its pad is 179935), so it is
/// never assumed. Only batched calls pad at all, and the mask hides the pad
/// positions either way; the check is free and a mismatch means a mixed cache.
fn pad_id(tokenizer: &Tokenizer, tokenizer_config_json: &str, config_json: &str) -> Result<u32> {
    let config: serde_json::Value = serde_json::from_str(config_json)
        .map_err(|e| IndexError::Embedding(format!("parsing config.json: {e}")))?;
    let from_model = config
        .get("pad_token_id")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let tokenizer_config: serde_json::Value = serde_json::from_str(tokenizer_config_json)
        .map_err(|e| IndexError::Embedding(format!("parsing tokenizer_config.json: {e}")))?;
    let from_tokenizer = tokenizer_config
        .get("pad_token")
        // A pad token is written either as the plain string or, in the
        // transformers "AddedToken" form, as an object carrying `content`.
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.as_str()),
            other => other.get("content").and_then(|c| c.as_str()),
        })
        .and_then(|t| tokenizer.token_to_id(t));
    match (from_tokenizer, from_model) {
        (Some(t), Some(m)) if t == m => Ok(t),
        (Some(t), Some(m)) => Err(IndexError::Embedding(format!(
            "pad token mismatch: tokenizer_config.json's pad token is id {t}, config.json says pad_token_id {m}; the model cache holds mixed files"
        ))),
        (Some(t), None) => Ok(t),
        (None, Some(m)) => Ok(m),
        (None, None) => Ok(0),
    }
}

/// Truncate to at most `max_chars` characters on a char boundary.
fn cap_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

fn embed_texts(encoder: &Encoder, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    // The last line of defense before `encode_batch`, a no-op on a batch the
    // caller already capped. The batch is copied for `encode_batch` either way,
    // so the cap costs nothing on top.
    let inputs: Vec<String> = texts
        .iter()
        .map(|t| cap_chars(t, MAX_INPUT_CHARS).to_string())
        .collect();
    // `true` is what puts `<|startoftext|>` at position 0 for granite and
    // `[CLS]` there for bge, which is what the pooling below reads.
    let encodings = encoder
        .tokenizer
        .encode_batch(inputs, true)
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
        let input_ids = Tensor::from_vec(ids, (batch, seq_len), &encoder.device)?;
        let attention = Tensor::from_vec(mask, (batch, seq_len), &encoder.device)?;
        let sequence = match &encoder.loaded {
            Loaded::Bert(model) => {
                let token_type = input_ids.zeros_like()?;
                model.forward(&input_ids, &token_type, Some(&attention))?
            }
            // ModernBERT has no segment embeddings and takes the raw
            // (batch, seq) mask: it builds its own 4D additive mask and the
            // sliding-window band inside `forward`. Do not pre-expand it.
            Loaded::ModernBert(model) => model.forward(&input_ids, &attention)?,
        };
        // (batch, seq, hidden) out of either encoder; the sentence vector is
        // the [CLS] position (both models' own pooling), then L2 norm.
        let pooled = cls_pool(&sequence)?;
        let normalized = normalize_l2(&pooled)?;
        normalized.to_vec2::<f32>()
    };
    compute().map_err(|e| IndexError::Embedding(format!("inference: {e}")))
}

/// The `[CLS]` position of every row: position 0, which the tokenizer's
/// post-processor guarantees is the start token for both table entries.
fn cls_pool(sequence: &Tensor) -> candle_core::Result<Tensor> {
    sequence.narrow(1, 0, 1)?.squeeze(1)
}

fn normalize_l2(v: &Tensor) -> candle_core::Result<Tensor> {
    v.broadcast_div(&v.sqr()?.sum_keepdim(1)?.sqrt()?)
}

fn wipe_model_dir(cache_dir: &Path, model: &LocalModel) {
    let dir = cache_dir.join(model.cache_dir_name());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(test)]
mod tests {
    use candle_core::{Device, Tensor};

    use super::{MAX_INPUT_CHARS, cap_chars, check_model_type, cls_pool, pad_id};
    use crate::embed::models::lookup_local_model;

    #[test]
    fn cls_pooling_takes_the_first_position_of_every_row() {
        let device = Device::Cpu;
        let sequence = Tensor::from_vec(
            vec![
                1.0f32, 2.0, 3.0, 4.0, 100.0, 100.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0,
            ],
            (2, 3, 2),
            &device,
        )
        .unwrap();
        let pooled = cls_pool(&sequence).unwrap().to_vec2::<f32>().unwrap();
        assert_eq!(pooled[0], vec![1.0, 2.0]);
        assert_eq!(pooled[1], vec![5.0, 6.0]);
    }

    /// A cache holding the wrong model's files would otherwise fail deep
    /// inside candle with a missing-tensor message; the refusal names both.
    #[test]
    fn a_config_whose_model_type_is_not_the_tables_is_refused() {
        let granite = lookup_local_model("granite-embedding-97m-multilingual-r2").unwrap();
        assert!(check_model_type(r#"{"model_type": "modernbert"}"#, granite).is_ok());
        let err = check_model_type(r#"{"model_type": "bert"}"#, granite)
            .unwrap_err()
            .to_string();
        assert!(err.contains("modernbert") && err.contains("bert"), "{err}");
        // bge's config.json declares "bert".
        let bge = lookup_local_model("bge-small-en-v1.5").unwrap();
        assert!(check_model_type(r#"{"model_type": "bert"}"#, bge).is_ok());
        // A config with no model_type at all is a refusal too, not a guess.
        assert!(check_model_type(r#"{}"#, granite).is_err());
    }

    /// The pad id is what the tokenizer says the configured pad token is, and
    /// it has to agree with the model config; a disagreement means a mixed or
    /// corrupt cache.
    #[test]
    fn the_pad_id_comes_from_the_tokenizer_config_and_must_match_the_model_config() {
        // A three-token tokenizer built in place: no download, no fixture.
        // Written as a tokenizer.json rather than through the WordLevel
        // builder, whose vocabulary type is a hasher this crate does not
        // depend on.
        let tokenizer = tokenizers::Tokenizer::from_bytes(
            br#"{"version": "1.0", "truncation": null, "padding": null,
                 "added_tokens": [], "normalizer": null, "pre_tokenizer": null,
                 "post_processor": null, "decoder": null,
                 "model": {"type": "WordLevel", "unk_token": "[UNK]",
                           "vocab": {"[UNK]": 0, "hello": 1, "<|pad|>": 2}}}"#,
        )
        .unwrap();

        let id = pad_id(
            &tokenizer,
            r#"{"pad_token": "<|pad|>"}"#,
            r#"{"pad_token_id": 2}"#,
        )
        .unwrap();
        assert_eq!(id, 2);
        let err = pad_id(
            &tokenizer,
            r#"{"pad_token": "<|pad|>"}"#,
            r#"{"pad_token_id": 7}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("pad"), "{err}");
        // bge's config.json carries pad_token_id 0 and its tokenizer_config
        // names "[PAD]"; a tokenizer config with no pad_token falls back to
        // the model config's id alone.
        assert_eq!(
            pad_id(&tokenizer, r#"{}"#, r#"{"pad_token_id": 2}"#).unwrap(),
            2
        );
        // The transformers "AddedToken" form of pad_token, an object with a
        // `content` field, is the same token said another way.
        assert_eq!(
            pad_id(
                &tokenizer,
                r#"{"pad_token": {"content": "<|pad|>", "lstrip": false}}"#,
                r#"{"pad_token_id": 2}"#
            )
            .unwrap(),
            2
        );
    }

    #[test]
    fn cap_chars_bounds_input_on_a_char_boundary() {
        // Short input passes through untouched.
        assert_eq!(cap_chars("hello", MAX_INPUT_CHARS), "hello");
        // A long input is cut to the character count, not the byte count, and
        // never inside a multi-byte character.
        let text = "aé漢".repeat(10);
        let capped = cap_chars(&text, 5);
        assert_eq!(capped.chars().count(), 5);
        assert_eq!(capped, "aé漢aé");
        // The cap is generous enough that no chunk-sized text is ever cut.
        let chunk = "word ".repeat(crate::embed::DEFAULT_MAX_TOKENS);
        assert_eq!(cap_chars(&chunk, MAX_INPUT_CHARS), chunk.as_str());
    }
}
