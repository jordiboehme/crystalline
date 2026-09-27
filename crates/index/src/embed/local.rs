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

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use crystalline_core::config::EmbeddingsConfig;
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

use super::models::{Architecture, LocalModel, lookup_local_model};
use super::modernbert::{Config as ModernBertConfig, ModernBert};
use super::{DEFAULT_MODEL_ID, EmbeddingProvider};
use crate::error::{IndexError, Result};
use crate::hub::{HubFiles, HubRepo, ensure_files, models_cache_dir, pad_id, read, wipe_repo_dir};

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
    let files = ensure_files(&cache_dir, &hub_repo(model)).await?;
    // Metadata sizing only: a handful of stat calls, which no blocking task
    // has to carry now that the fetch itself is async.
    let bytes = files
        .paths()
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

/// What the shared hub fetch needs to know about an embedding model.
fn hub_repo(model: &'static LocalModel) -> HubRepo<'static> {
    HubRepo {
        repo: model.repo,
        files: model.files,
        download_mb: model.download_mb,
        what: "embedding model",
    }
}

/// Load the model, self-healing once from a corrupt cache. The fetch is awaited
/// here; only the weight load goes to a blocking thread.
async fn load_encoder(cache_dir: &Path, model: &'static LocalModel) -> Result<Encoder> {
    let files = ensure_files(cache_dir, &hub_repo(model)).await?;
    match build_on_blocking(files, model).await {
        Ok(encoder) => Ok(encoder),
        Err(first) => {
            // A truncated or corrupt cache: wipe the model directory and fetch
            // once more before surfacing the failure.
            eprintln!(
                "crystalline: embedding model failed to load ({first}); re-downloading once..."
            );
            wipe_repo_dir(cache_dir, model.repo);
            let files = ensure_files(cache_dir, &hub_repo(model)).await?;
            build_on_blocking(files, model).await
        }
    }
}

/// [`build_encoder`] on a blocking thread: it mmaps and parses the weights.
async fn build_on_blocking(files: HubFiles, model: &'static LocalModel) -> Result<Encoder> {
    tokio::task::spawn_blocking(move || build_encoder(&files, model))
        .await
        .map_err(|e| IndexError::Embedding(format!("model load task failed: {e}")))?
}

fn build_encoder(files: &HubFiles, model: &LocalModel) -> Result<Encoder> {
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

#[cfg(test)]
mod tests {
    use candle_core::{Device, Tensor};

    use super::{MAX_INPUT_CHARS, cap_chars, check_model_type, cls_pool};
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
