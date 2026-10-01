//! The local NLI scorer: one checkpoint from [`super::models`] via candle,
//! always in F32. mDeBERTa overflows in F16, so an F16 or BF16 file is upcast
//! on load and no NLI model ever runs in half precision. The files are
//! fetched into the same cache as the embedding model through [`crate::hub`],
//! with the same first-use notice and progress line, and a load failure from a
//! corrupt cache removes the failed snapshot and fetches once more.
//!
//! The device is the embedding model's pick ([`crate::device`]): the Metal GPU
//! of an Apple Silicon Mac when the checkpoint builds there and scores a
//! warm-up pair, the CPU otherwise. A GPU error or panic only falls back to
//! the CPU, so the self-heal above runs on a CPU failure alone and never
//! removes a good snapshot because of the GPU.
//!
//! candle's `DebertaV2SeqClassificationModel` reads `pooler.dense` and
//! `classifier` through `vb.root()`, so it is handed `vb.pp("deberta")`. The
//! pair template is the tokenizer's own post-processor (`[CLS] a [SEP] b
//! [SEP]`), and the pad id is the checkpoint's.
//!
//! Shutdown: the fetch is an async future a caller can stop awaiting, but
//! hf-xet runs parts of a download as `spawn_blocking` tasks on the calling
//! runtime, and the weight load here is one more. A runtime drop waits for
//! every such task, so a process that owns the index must never end by
//! dropping its runtime while a load is in flight: the daemon leaves through
//! `std::process::exit` (its `Departure`), which waits for none of them.

use std::collections::BTreeMap;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::debertav2::{
    Config as DebertaConfig, DebertaV2SeqClassificationModel,
};
use crystalline_core::config;
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationStrategy};

use super::models::NliModel;
#[cfg(test)]
use super::models::{NliProfile, nli_model};
use super::{ContradictionScorer, MAX_LINE_TOKENS, contradiction_index};
use crate::device::{DeviceReport, load_on_best_device};
use crate::embed::models::{note_loaded_snapshot, remove_failed_snapshot};
use crate::error::{IndexError, Result};
use crate::hub::{HubFiles, HubRepo, ensure_files, models_cache_dir, pad_id, read};

/// Two lines of [`MAX_LINE_TOKENS`] plus the separators fit this window.
const MAX_PAIR_TOKENS: usize = 512;

/// The character cut applied to a line before it is tokenized at all. The
/// tokenizer materializes the whole `Encoding` (tens of bytes per token)
/// before any truncation, so an uncut megabyte line would cost memory in
/// proportion to its length (the 70 GB spike). Sixteen characters per token
/// is far past any real ratio (sentencepiece pieces are at most 16
/// characters), so this never changes where the token cut lands on a line a
/// person wrote; it only bounds a pathological one. `evals/nli/parity.py`
/// applies the same cut.
const MAX_LINE_CHARS: usize = MAX_LINE_TOKENS * 16;

/// A loaded NLI checkpoint.
pub struct LocalNli {
    model: &'static NliModel,
    head: Box<DebertaV2SeqClassificationModel>,
    tokenizer: Tokenizer,
    device: Device,
    report: DeviceReport,
    contradiction: usize,
}

fn hub_repo(model: &'static NliModel) -> HubRepo<'static> {
    HubRepo {
        repo: model.repo,
        revision: model.revision,
        files: model.files,
        download_mb: model.download_mb,
        what: "contradiction model",
    }
}

/// `hub.rs` is shared with the embedding loader and labels its own errors
/// `Embedding`, correctly, for that path. The contradiction loader relabels
/// them here rather than in `hub.rs`, so a download or cache-dir failure
/// while loading an NLI model never renders as "embedding error" in status or
/// doctor; the text underneath already names "contradiction model" and its
/// repository (`hub_repo`'s `what`), so nothing is lost by the rename.
///
/// Everything that comes out of the cache directory lookup and the fetch is
/// download-class, [`IndexError::NliFetch`], which the daemon tries again
/// later on its own; only a build error blocks the model until the setting
/// changes.
fn as_fetch(e: IndexError) -> IndexError {
    match e {
        IndexError::Embedding(m) | IndexError::Nli(m) | IndexError::NliFetch(m) => {
            IndexError::NliFetch(m)
        }
        other => IndexError::NliFetch(other.to_string()),
    }
}

impl LocalNli {
    /// Load `model` at its pinned commit, downloading on first use. The
    /// fetch is awaited; the weight load runs on a blocking thread.
    ///
    /// A build failure self-heals once the way the embedding model's pinned
    /// path does ([`remove_failed_snapshot`]): only the pinned snapshot that
    /// failed goes, and in a `CRYSTALLINE_MODELS_DIR` only its directory,
    /// never a blob, a ref or another snapshot. Unlike the embedding model
    /// there is no start on an older snapshot: no released build ever fetched
    /// another commit of an NLI checkpoint, so there is nothing to fall back
    /// to and nothing to settle.
    pub async fn load(model: &'static NliModel) -> Result<LocalNli> {
        let cache_dir = models_cache_dir().map_err(as_fetch)?;
        let fetch = || async {
            let files = ensure_files(&cache_dir, &hub_repo(model), true)
                .await
                .map_err(as_fetch)?;
            let snapshot = files.snapshot_dir().map_err(as_fetch)?;
            Ok((files, snapshot))
        };
        let build = |(files, snapshot): (HubFiles, std::path::PathBuf)| async move {
            let nli = build_on_blocking(files, model).await?;
            note_loaded_snapshot(&snapshot);
            Ok(nli)
        };
        let heal = || {
            remove_failed_snapshot(
                &cache_dir,
                &model.pinned(),
                config::models_dir_is_user_provided(),
            )
        };
        load_healing(fetch, build, heal).await
    }

    /// The token ids each `(premise, hypothesis)` pair runs as, padding
    /// removed: both lines cut, the pair template applied. The parity test
    /// compares these against Python before it compares logits, so a miss
    /// says whether the tokenizer or the model is at fault.
    pub fn pair_ids(&self, pairs: &[(String, String)]) -> Result<Vec<Vec<u32>>> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .encode(pairs)?
            .iter()
            .map(|enc| {
                enc.get_ids()
                    .iter()
                    .zip(enc.get_attention_mask())
                    .filter(|(_, m)| **m == 1)
                    .map(|(id, _)| *id)
                    .collect()
            })
            .collect())
    }

    /// Cut both lines of every pair and encode the batch, padded to its
    /// longest pair.
    fn encode(&self, pairs: &[(String, String)]) -> Result<Vec<tokenizers::Encoding>> {
        let mut inputs = Vec::with_capacity(pairs.len());
        for (premise, hypothesis) in pairs {
            inputs.push((
                cap_line(&self.tokenizer, premise)?,
                cap_line(&self.tokenizer, hypothesis)?,
            ));
        }
        self.tokenizer
            .encode_batch(inputs, true)
            .map_err(|e| IndexError::Nli(format!("tokenizing: {e}")))
    }

    /// The raw classifier logits of each `(premise, hypothesis)` pair, as one
    /// padded batch. What the parity test compares against Python; the scorer
    /// takes the softmax of these.
    pub fn logits(&self, pairs: &[(String, String)]) -> Result<Vec<Vec<f32>>> {
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self.encode(pairs)?;
        let batch = encodings.len();
        let seq = encodings.first().map(|e| e.get_ids().len()).unwrap_or(0);
        let mut ids: Vec<u32> = Vec::with_capacity(batch * seq);
        let mut types: Vec<u32> = Vec::with_capacity(batch * seq);
        let mut mask: Vec<u32> = Vec::with_capacity(batch * seq);
        for enc in &encodings {
            ids.extend_from_slice(enc.get_ids());
            types.extend_from_slice(enc.get_type_ids());
            mask.extend_from_slice(enc.get_attention_mask());
        }
        let compute = || -> candle_core::Result<Vec<Vec<f32>>> {
            let input_ids = Tensor::from_vec(ids, (batch, seq), &self.device)?;
            let attention = Tensor::from_vec(mask, (batch, seq), &self.device)?;
            // DebertaV2Model's own default mask is I64 and its token types
            // U32, so these are the dtypes it expects.
            let token_types = Tensor::from_vec(types, (batch, seq), &self.device)?;
            let logits = self.head.forward(
                &input_ids,
                Some(token_types),
                Some(attention.to_dtype(DType::I64)?),
            )?;
            logits.to_vec2::<f32>()
        };
        compute().map_err(|e| IndexError::Nli(format!("inference: {e}")))
    }
}

impl ContradictionScorer for LocalNli {
    fn score(&self, pairs: &[(String, String)]) -> Result<Vec<f32>> {
        Ok(self
            .logits(pairs)?
            .iter()
            .map(|row| softmax_at(row, self.contradiction))
            .collect())
    }

    fn model_repo(&self) -> &str {
        self.model.repo
    }

    fn device(&self) -> Option<DeviceReport> {
        Some(self.report.clone())
    }
}

/// Fetch, then build; when the build fails, `heal` (remove the failed
/// snapshot), fetch again and build once more. A fetch error is returned as
/// it is and heals nothing. A GPU failure never reaches `heal` either:
/// [`build_on_blocking`] falls back to the CPU inside the build, so only the
/// CPU's error counts as a broken snapshot.
async fn load_healing<Files, T, FetchFut, BuildFut>(
    fetch: impl Fn() -> FetchFut,
    build: impl Fn(Files) -> BuildFut,
    heal: impl FnOnce(),
) -> Result<T>
where
    FetchFut: std::future::Future<Output = Result<Files>>,
    BuildFut: std::future::Future<Output = Result<T>>,
{
    match build(fetch().await?).await {
        Ok(loaded) => Ok(loaded),
        Err(first) => {
            eprintln!(
                "crystalline: contradiction model failed to load ({first}); re-downloading once..."
            );
            heal();
            build(fetch().await?).await
        }
    }
}

/// [`build`] on a blocking thread, on the device [`crate::device`] picks: the
/// GPU when the checkpoint builds there and scores [`warm_up`], the CPU
/// otherwise. The error this returns is always the CPU's.
async fn build_on_blocking(files: HubFiles, model: &'static NliModel) -> Result<LocalNli> {
    tokio::task::spawn_blocking(move || {
        let (mut nli, report) = load_on_best_device(
            "contradiction model",
            |device| build(&files, model, device),
            warm_up,
        )?;
        nli.report = report;
        Ok(nli)
    })
    .await
    .map_err(|e| IndexError::Nli(format!("contradiction model load task failed: {e}")))?
}

/// One pair through the whole scoring path, logits read back to the host, so
/// an op the GPU lacks (the I64 mask cast, a kernel) or an error it reports
/// only at sync surfaces while the CPU is still there to fall back to.
fn warm_up(nli: &LocalNli) -> Result<()> {
    nli.logits(&[("warm-up".to_string(), "warm-up".to_string())])
        .map(|_| ())
}

fn build(files: &HubFiles, model: &'static NliModel, device: &Device) -> Result<LocalNli> {
    let config_text = read(files.config()?)?;
    let raw: serde_json::Value = serde_json::from_str(&config_text)
        .map_err(|e| IndexError::Nli(format!("parsing config.json: {e}")))?;
    check_model_type(&raw, model)?;
    let labels = id2label(&raw)?;
    let contradiction = contradiction_index(&labels)?;

    let mut tokenizer = Tokenizer::from_file(files.tokenizer()?)
        .map_err(|e| IndexError::Nli(format!("loading tokenizer.json: {e}")))?;
    let tokenizer_config_text = match files.tokenizer_config() {
        Some(path) => read(path)?,
        None => "{}".to_string(),
    };
    let pad = pad_id(&tokenizer, &tokenizer_config_text, &config_text)?;
    let pad_token = tokenizer.id_to_token(pad).unwrap_or_default();
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        pad_id: pad,
        pad_token,
        ..PaddingParams::default()
    }));
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: MAX_PAIR_TOKENS,
            strategy: TruncationStrategy::LongestFirst,
            ..TruncationParams::default()
        }))
        .map_err(|e| IndexError::Nli(format!("configuring truncation: {e}")))?;

    let device = device.clone();
    // Safety: a freshly verified download on the standard candle mmap path.
    // DType::F32 upcasts an F16 or BF16 file; nothing here runs in F16.
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(
            std::slice::from_ref(files.weights()?),
            DType::F32,
            &device,
        )
        .map_err(|e| IndexError::Nli(format!("loading weights: {e}")))?
    };
    let build_error = |e: candle_core::Error| IndexError::Nli(format!("building model: {e}"));
    let config: DebertaConfig = serde_json::from_str(&config_text)
        .map_err(|e| IndexError::Nli(format!("parsing config.json: {e}")))?;
    let head = Box::new(
        DebertaV2SeqClassificationModel::load(vb.pp("deberta"), &config, None)
            .map_err(build_error)?,
    );
    Ok(LocalNli {
        model,
        head,
        tokenizer,
        device,
        report: DeviceReport::cpu(),
        contradiction,
    })
}

/// The `model_type` in `config.json` must be the table entry's, so a cache
/// holding another model's files fails here by name.
fn check_model_type(config: &serde_json::Value, model: &NliModel) -> Result<()> {
    let want = model.model_type;
    match config.get("model_type").and_then(|v| v.as_str()) {
        Some(found) if found == want => Ok(()),
        found => Err(IndexError::Nli(format!(
            "the cached config.json for {} declares model_type {found:?}, expected \"{want}\"; the model cache holds the wrong files",
            model.repo
        ))),
    }
}

/// `config.json`'s `id2label`, keys parsed to row indices, which candle's
/// DeBERTa config does not carry.
fn id2label(config: &serde_json::Value) -> Result<BTreeMap<u32, String>> {
    let map = config
        .get("id2label")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            IndexError::Invalid("the checkpoint's config.json carries no id2label".to_string())
        })?;
    map.iter()
        .map(|(k, v)| {
            let index = k.parse::<u32>().map_err(|_| {
                IndexError::Invalid(format!("id2label key '{k}' is not a row index"))
            })?;
            let label = v.as_str().ok_or_else(|| {
                IndexError::Invalid(format!("id2label entry {k} is not a string"))
            })?;
            Ok((index, label.to_string()))
        })
        .collect()
}

/// The first [`MAX_LINE_CHARS`] characters of `text`, on a char boundary: the
/// bound applied before the tokenizer sees a line at all.
fn precut(text: &str) -> &str {
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// Cut a line to [`MAX_LINE_TOKENS`] of the model's own tokens, on a token
/// and char boundary, so `LongestFirst` truncation of the pair has nothing to
/// do and neither side eats the other's half of the window. The line is cut
/// to [`MAX_LINE_CHARS`] characters first, so the tokenizer never encodes an
/// unbounded text.
fn cap_line(tokenizer: &Tokenizer, text: &str) -> Result<String> {
    let text = precut(text);
    let enc = tokenizer
        .encode(text, false)
        .map_err(|e| IndexError::Nli(format!("tokenizing: {e}")))?;
    if enc.get_ids().len() <= MAX_LINE_TOKENS {
        return Ok(text.to_string());
    }
    let mut end = enc.get_offsets()[MAX_LINE_TOKENS - 1].1.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(text[..end].to_string())
}

/// The softmax of `row` at `index`, shifted by the maximum so large logits do
/// not overflow.
fn softmax_at(row: &[f32], index: usize) -> f32 {
    let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = row.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.get(index).copied().unwrap_or(0.0) / sum
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word-level tokenizer built in place: no download, no fixture.
    fn words() -> Tokenizer {
        Tokenizer::from_bytes(
            br#"{"version": "1.0", "truncation": null, "padding": null,
                 "added_tokens": [], "normalizer": null,
                 "pre_tokenizer": {"type": "Whitespace"},
                 "post_processor": null, "decoder": null,
                 "model": {"type": "WordLevel", "unk_token": "[UNK]",
                           "vocab": {"[UNK]": 0, "w": 1, "<pad>": 2}}}"#,
        )
        .unwrap()
    }

    #[test]
    fn a_line_over_254_tokens_is_cut_on_a_token_boundary() {
        let t = words();
        let long = "w ".repeat(300);
        let cut = cap_line(&t, &long).unwrap();
        assert_eq!(cut.split_whitespace().count(), MAX_LINE_TOKENS);
        assert!(
            cut.ends_with('w'),
            "cut after a whole token: {:?}",
            &cut[cut.len() - 3..]
        );
        assert_eq!(
            cap_line(&t, "w w w").unwrap(),
            "w w w",
            "a short line passes untouched"
        );
    }

    /// Lesson 1 (the 70 GB spike): a line is cut by characters before the
    /// tokenizer sees it, so a megabyte line is never encoded whole.
    #[test]
    fn a_megabyte_line_is_cut_by_characters_before_it_is_tokenized() {
        let t = words();
        let megabyte = "w ".repeat(500_000);
        assert_eq!(megabyte.len(), 1_000_000);
        assert_eq!(precut(&megabyte).chars().count(), MAX_LINE_CHARS);
        let cut = cap_line(&t, &megabyte).unwrap();
        assert_eq!(cut.split_whitespace().count(), MAX_LINE_TOKENS);

        // One megabyte word is a single token, so only the character cut can
        // bound it: without it the whole megabyte would come back.
        let one_word = "w".repeat(1_000_000);
        let cut = cap_line(&t, &one_word).unwrap();
        assert_eq!(cut.chars().count(), MAX_LINE_CHARS);

        // The character cut never splits a character and never touches a line
        // a real observation would be.
        let wide = "ü".repeat(MAX_LINE_CHARS + 10);
        assert_eq!(precut(&wide).chars().count(), MAX_LINE_CHARS);
        assert_eq!(precut("Der Build nutzt Node 18"), "Der Build nutzt Node 18");
        const {
            assert!(
                MAX_LINE_CHARS >= MAX_LINE_TOKENS * 8,
                "far past any real chars-per-token ratio"
            )
        };
    }

    #[test]
    fn the_probability_is_the_softmax_at_the_contradiction_row() {
        let p = softmax_at(&[0.0, 0.0, 0.0], 2);
        assert!((p - 1.0 / 3.0).abs() < 1e-6);
        let high = softmax_at(&[-4.0, -2.0, 6.0], 2);
        assert!(high > 0.99, "{high}");
        let stable = softmax_at(&[1000.0, 0.0, 1000.0], 0);
        assert!((stable - 0.5).abs() < 1e-6, "no overflow on large logits");
        // Lesson 20: the rows are a probability distribution.
        let row = [1.25f32, -0.5, 3.0];
        let sum: f32 = (0..row.len()).map(|i| softmax_at(&row, i)).sum();
        assert!((sum - 1.0).abs() < 1e-6, "{sum}");
    }

    #[test]
    fn id2label_and_model_type_come_from_config_json() {
        let deberta = nli_model(NliProfile::Full);
        let raw: serde_json::Value = serde_json::from_str(
            r#"{"model_type": "deberta-v2", "id2label": {"0": "entailment", "1": "neutral", "2": "contradiction"}}"#,
        )
        .unwrap();
        assert!(check_model_type(&raw, deberta).is_ok());
        let labels = id2label(&raw).unwrap();
        assert_eq!(labels.get(&2).map(String::as_str), Some("contradiction"));
        let other: serde_json::Value =
            serde_json::from_str(r#"{"model_type": "xlm-roberta"}"#).unwrap();
        let err = check_model_type(&other, deberta).unwrap_err().to_string();
        assert!(
            err.contains("xlm-roberta") && err.contains("deberta-v2"),
            "{err}"
        );
        let bare: serde_json::Value =
            serde_json::from_str(r#"{"model_type": "deberta-v2"}"#).unwrap();
        assert!(
            id2label(&bare).is_err(),
            "a checkpoint without id2label is refused"
        );
    }

    /// A Task 4 minor, fixed here: an NLI loader failure must never render as
    /// "embedding error" in status or doctor, since the checkpoint is not an
    /// embedding model. `check_model_type`'s refusal, and every error this
    /// file constructs, carry the `Nli` variant instead.
    #[test]
    fn an_nli_failure_is_never_labeled_an_embedding_error() {
        let raw: serde_json::Value =
            serde_json::from_str(r#"{"model_type": "xlm-roberta"}"#).unwrap();
        let err = check_model_type(&raw, nli_model(NliProfile::Full)).unwrap_err();
        assert!(matches!(err, IndexError::Nli(_)), "{err:?}");
        let text = err.to_string();
        assert!(!text.contains("embedding error"), "{text}");
        assert!(text.starts_with("contradiction model error: "), "{text}");
    }

    /// `hub.rs`'s errors stay labeled `Embedding` there (the embedding loader
    /// shares that module), so the NLI loader relabels them at its own call
    /// boundary, as the download-class failure the daemon retries later.
    #[test]
    fn as_fetch_relabels_every_fetch_error_as_a_retryable_nli_error() {
        let hub_err = IndexError::Embedding("downloading model.safetensors: offline".to_string());
        let relabeled = as_fetch(hub_err);
        assert!(matches!(relabeled, IndexError::NliFetch(_)));
        let text = relabeled.to_string();
        assert!(text.contains("offline"), "{text}");
        assert!(text.starts_with("contradiction model error: "), "{text}");

        let io = IndexError::Io {
            path: "/models".to_string(),
            source: std::io::Error::other("read-only file system"),
        };
        let relabeled = as_fetch(io);
        assert!(matches!(relabeled, IndexError::NliFetch(_)));
        assert!(
            relabeled.to_string().contains("read-only file system"),
            "{relabeled}"
        );
    }

    /// The contradiction model's device pick and its self-heal, driven with
    /// the CPU standing in for the GPU (the same seam `crate::device` tests
    /// with): a GPU error or panic falls back to the CPU and never removes the
    /// snapshot; only a CPU failure reaches the self-heal.
    mod device_pick {
        use std::cell::Cell;

        use candle_core::Device;

        use super::super::load_healing;
        use crate::device::{Accelerator, DeviceKind, DeviceReport, load_with};
        use crate::error::{IndexError, Result};

        fn fake_gpu() -> Accelerator {
            Accelerator::Device(Device::Cpu, DeviceKind::Metal)
        }

        /// One load the way `LocalNli::load` runs it: fetch (here nothing),
        /// then the device pick with `build` and `warm`, healing on an error.
        async fn load(
            build: impl Fn(&Device) -> Result<u32>,
            warm: impl Fn(&u32) -> Result<()>,
            heals: &Cell<u32>,
        ) -> Result<(u32, DeviceReport)> {
            load_healing(
                || async { Ok(()) },
                |()| {
                    let picked = load_with("contradiction model", fake_gpu(), &build, &warm);
                    async move { picked }
                },
                || heals.set(heals.get() + 1),
            )
            .await
        }

        #[tokio::test]
        async fn a_gpu_error_falls_back_to_the_cpu_and_never_heals() {
            let (builds, heals) = (Cell::new(0), Cell::new(0));
            let (model, report) = load(
                |_| {
                    builds.set(builds.get() + 1);
                    match builds.get() {
                        1 => Err(IndexError::Nli("building model: no metal kernel".into())),
                        n => Ok(n),
                    }
                },
                |_| Ok(()),
                &heals,
            )
            .await
            .unwrap();
            assert_eq!(model, 2, "built again on the CPU");
            assert_eq!(heals.get(), 0, "a GPU error never removes the snapshot");
            assert_eq!(report.kind, DeviceKind::Cpu);
            let reason = report.fallback().unwrap();
            assert!(reason.starts_with("metal failed: "), "{reason}");
            assert!(reason.contains("no metal kernel"), "{reason}");
        }

        #[tokio::test]
        async fn a_gpu_panic_in_the_build_or_the_warm_up_falls_back_and_never_heals() {
            let (builds, heals) = (Cell::new(0), Cell::new(0));
            let (_, report) = load(
                |_| {
                    builds.set(builds.get() + 1);
                    if builds.get() == 1 {
                        panic!("called `Option::unwrap()` on a `None` value");
                    }
                    Ok(builds.get())
                },
                |_| Ok(()),
                &heals,
            )
            .await
            .unwrap();
            assert_eq!(heals.get(), 0);
            assert!(
                report.fallback().unwrap().starts_with("metal panicked: "),
                "{report}"
            );

            let (builds, heals) = (Cell::new(0), Cell::new(0));
            let (_, report) = load(
                |_| {
                    builds.set(builds.get() + 1);
                    Ok(builds.get())
                },
                |n| {
                    if *n == 1 {
                        panic!("{}", String::from("command buffer lost"));
                    }
                    Ok(())
                },
                &heals,
            )
            .await
            .unwrap();
            assert_eq!(heals.get(), 0);
            assert_eq!(report.kind, DeviceKind::Cpu);
            assert!(
                report.fallback().unwrap().contains("command buffer lost"),
                "{report}"
            );
        }

        #[tokio::test]
        async fn a_working_gpu_is_kept_and_reported() {
            let heals = Cell::new(0);
            let (_, report) = load(|_| Ok(1), |_| Ok(()), &heals).await.unwrap();
            assert_eq!(report, DeviceReport::metal());
            assert_eq!(heals.get(), 0);
        }

        #[tokio::test]
        async fn only_a_cpu_failure_reaches_the_self_heal_once() {
            let (builds, heals) = (Cell::new(0), Cell::new(0));
            let err = load(
                |_| {
                    builds.set(builds.get() + 1);
                    Err(IndexError::Nli(format!(
                        "building model: corrupt weights {}",
                        builds.get()
                    )))
                },
                |_| Ok(()),
                &heals,
            )
            .await
            .unwrap_err();
            assert_eq!(heals.get(), 1, "healed once, after the first CPU failure");
            assert_eq!(builds.get(), 4, "GPU and CPU, twice");
            assert!(err.to_string().contains("corrupt weights 4"), "{err}");
        }

        #[tokio::test]
        async fn a_fetch_error_is_returned_without_a_heal_or_a_build() {
            let (builds, heals) = (Cell::new(0), Cell::new(0));
            let err = load_healing(
                || async { Err::<(), _>(IndexError::NliFetch("offline".into())) },
                |()| {
                    builds.set(builds.get() + 1);
                    async { Ok(()) }
                },
                || heals.set(heals.get() + 1),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, IndexError::NliFetch(_)), "{err:?}");
            assert_eq!((builds.get(), heals.get()), (0, 0));
        }
    }

    /// Review focus 5, the half that runs without a download: the pad id is
    /// what the checkpoint says, here 1, never a default 0.
    #[test]
    fn the_pad_id_is_read_from_the_checkpoint() {
        let t = Tokenizer::from_bytes(
            br#"{"version": "1.0", "truncation": null, "padding": null,
                 "added_tokens": [], "normalizer": null, "pre_tokenizer": null,
                 "post_processor": null, "decoder": null,
                 "model": {"type": "WordLevel", "unk_token": "<unk>",
                           "vocab": {"<s>": 0, "<pad>": 1, "</s>": 2, "<unk>": 3}}}"#,
        )
        .unwrap();
        assert_eq!(
            crate::hub::pad_id(&t, r#"{"pad_token": "<pad>"}"#, r#"{"pad_token_id": 1}"#).unwrap(),
            1
        );
    }
}
