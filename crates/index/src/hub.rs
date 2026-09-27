//! The hf-hub plumbing every local model shares: the embedding model in
//! [`crate::embed`] and the contradiction model in [`crate::nli`].
//!
//! Files are fetched with hf-hub into Crystalline's own model cache (not
//! hf-hub's default location), announced once on stderr when the weights are
//! not cached yet, and drawn as a byte-progress line on a live terminal. A file
//! already in the cache is resolved with no network call at all. The fetch is
//! an async future, but hf-xet runs parts of a download as `spawn_blocking`
//! tasks on the calling runtime, which a runtime drop waits for: a process
//! that owns the index leaves through `std::process::exit`, never a runtime
//! drop, while a download may be in flight.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crystalline_core::config;
use hf_hub::progress::{DownloadEvent, ProgressEvent, ProgressHandler};
use hf_hub::{HFClient, HFError};
use indexmap::IndexMap;
use tokenizers::Tokenizer;

use crate::embed::hub_dir_name;
use crate::error::{IndexError, Result};

/// What a fetch needs to know about one repository.
pub(crate) struct HubRepo<'a> {
    /// The Hugging Face repository id, `<owner>/<name>`.
    pub repo: &'a str,
    /// The files to fetch, in fetch order.
    pub files: &'a [&'a str],
    /// The approximate first-use download, in megabytes.
    pub download_mb: u64,
    /// What the first-use notice calls it: "embedding model" or "contradiction model".
    pub what: &'a str,
}

pub(crate) fn models_cache_dir() -> Result<PathBuf> {
    config::models_dir().map_err(|e| IndexError::Embedding(format!("model cache dir: {e}")))
}

/// The fetched file paths for one repository, by file name, in fetch order.
pub(crate) struct HubFiles {
    paths: IndexMap<String, PathBuf>,
}

impl HubFiles {
    pub(crate) fn get(&self, name: &str) -> Option<&PathBuf> {
        self.paths.get(name)
    }

    /// Every fetched path, in fetch order.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.paths.values()
    }

    /// A file the loader cannot do without. Missing means the table entry's
    /// file list and the loader disagree, which is a bug rather than a bad
    /// download, so it says so plainly.
    pub(crate) fn required(&self, name: &str) -> Result<&PathBuf> {
        self.get(name).ok_or_else(|| {
            IndexError::Embedding(format!(
                "the model's file list does not name {name}, which the loader needs"
            ))
        })
    }

    pub(crate) fn config(&self) -> Result<&PathBuf> {
        self.required("config.json")
    }

    pub(crate) fn tokenizer(&self) -> Result<&PathBuf> {
        self.required("tokenizer.json")
    }

    /// Absent for bge, which never shipped one in its three-file list.
    pub(crate) fn tokenizer_config(&self) -> Option<&PathBuf> {
        self.get("tokenizer_config.json")
    }

    pub(crate) fn weights(&self) -> Result<&PathBuf> {
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
pub(crate) struct ByteProgress {
    filename: String,
    total: AtomicU64,
    downloaded: AtomicU64,
    last_render: Mutex<Option<Instant>>,
}

impl ByteProgress {
    pub(crate) fn new(filename: &str) -> Self {
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

/// Fetch the repository's own file list into the cache, announcing a
/// first-use download once to stderr. The list is per model, so a model that
/// never needed a file is never made to dial out for it. When the whole
/// download is needed (nothing cached yet) and stderr is a live terminal, each
/// file also gets a `\r`-updated byte-progress line via [`ByteProgress`];
/// piped or redirected stderr (a log file, `--json`'s non-interactive
/// callers, CI) keeps exactly the single notice line, never per-byte output.
/// A file already present in the cache is resolved with no network call at
/// all, so a fully warmed cache - the air-gapped and CI-prefetch paths -
/// never dials out just to check.
pub(crate) async fn ensure_files(cache_dir: &Path, repo: &HubRepo<'_>) -> Result<HubFiles> {
    std::fs::create_dir_all(cache_dir).map_err(|e| IndexError::Io {
        path: cache_dir.display().to_string(),
        source: e,
    })?;

    let client = hub_client(cache_dir)?;
    // The weights alone decide whether this is a first-use download: they are
    // the file worth a notice and a progress line.
    let cached = is_cached(&client, repo).await?;
    if !cached {
        eprintln!(
            "crystalline: downloading {} {} to {} (first use, about {} MB)...",
            repo.what,
            repo.repo,
            cache_dir.display(),
            repo.download_mb
        );
    }
    let show_progress = !cached && std::io::stderr().is_terminal();

    let (owner, name) = repo_parts(repo.repo)?;
    let remote = client.model(owner, name);
    let mut paths = IndexMap::with_capacity(repo.files.len());
    for file in repo.files {
        let path = match cached_path(&client, repo, file).await? {
            Some(path) => path,
            None => remote
                .download_file()
                .filename(*file)
                // Progress is opt in in hf-hub: leaving the handler off is the
                // suppression, so exactly one progress mechanism is ever active
                // and it is the one this module controls and TTY-gates itself.
                .maybe_progress(show_progress.then(|| ByteProgress::new(file)))
                .send()
                .await
                .map_err(|e| {
                    IndexError::Embedding(format!(
                        "downloading {file} for {} {}: {e}",
                        repo.what, repo.repo
                    ))
                })?,
        };
        paths.insert((*file).to_string(), path);
    }
    Ok(HubFiles { paths })
}

/// `("BAAI", "bge-small-en-v1.5")` from a table entry's repository id, which is
/// the pair `HFClient::model` takes. A refusal rather than a guess: hf-hub's own
/// `split_id` answers an empty owner for a malformed id, which would silently
/// resolve to a cache directory [`hub_dir_name`] does not spell.
pub(crate) fn repo_parts(repo: &str) -> Result<(&str, &str)> {
    repo.split_once('/').ok_or_else(|| {
        IndexError::Embedding(format!(
            "the model table's repository id {repo} is not <owner>/<name>"
        ))
    })
}

/// The hf-hub client, cache-pinned to Crystalline's own model directory rather
/// than hf-hub's default location.
pub(crate) fn hub_client(cache_dir: &Path) -> Result<HFClient> {
    HFClient::builder()
        .cache_dir(cache_dir.to_path_buf())
        .build()
        .map_err(|e| IndexError::Embedding(format!("hub client: {e}")))
}

/// The cached path for one of the repository's files, or `None` when the cache
/// does not hold it. `local_files_only` answers from the cache directory alone
/// and never touches the network, and `LocalEntryNotFound` is the miss; a plain
/// download would HEAD the repository even on a warm cache, which is what the
/// air-gapped and CI-prefetch paths must not do.
pub(crate) async fn cached_path(
    client: &HFClient,
    repo: &HubRepo<'_>,
    name: &str,
) -> Result<Option<PathBuf>> {
    let (owner, repo_name) = repo_parts(repo.repo)?;
    match client
        .model(owner, repo_name)
        .download_file()
        .filename(name)
        .local_files_only(true)
        .send()
        .await
    {
        Ok(path) => Ok(Some(path)),
        Err(HFError::LocalEntryNotFound { .. }) => Ok(None),
        Err(e) => Err(IndexError::Embedding(format!(
            "reading the model cache for {name} of {} {}: {e}",
            repo.what, repo.repo
        ))),
    }
}

/// True when the weights are already in the cache, with no network call at all.
pub(crate) async fn is_cached(client: &HFClient, repo: &HubRepo<'_>) -> Result<bool> {
    Ok(cached_path(client, repo, "model.safetensors")
        .await?
        .is_some())
}

pub(crate) fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| IndexError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

/// The pad id: the tokenizer's id for `tokenizer_config.json`'s `pad_token`,
/// which has to agree with `config.json`'s `pad_token_id`. The default of 0 is
/// a real content token in granite's vocabulary (its pad is 179935), and XLM-R
/// pads with 1, so it is never assumed. The check is free and a mismatch means
/// a mixed cache.
pub(crate) fn pad_id(
    tokenizer: &Tokenizer,
    tokenizer_config_json: &str,
    config_json: &str,
) -> Result<u32> {
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

/// Remove one repository's directory from the cache, so the next fetch starts
/// clean. Best effort: a directory that is already gone is the goal.
pub(crate) fn wipe_repo_dir(cache_dir: &Path, repo: &str) {
    let dir = cache_dir.join(hub_dir_name(repo));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(test)]
mod tests {
    use super::pad_id;

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
}
