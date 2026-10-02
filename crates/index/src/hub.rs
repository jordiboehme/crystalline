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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crystalline_core::config;
use hf_hub::progress::{DownloadEvent, Progress, ProgressEvent, ProgressHandler};
use hf_hub::{HFClient, HFError};
use indexmap::IndexMap;
use tokenizers::Tokenizer;

use crate::embed::hub_dir_name;
use crate::error::{IndexError, Result};

/// What a fetch needs to know about one repository.
pub(crate) struct HubRepo<'a> {
    /// The Hugging Face repository id, `<owner>/<name>`.
    pub repo: &'a str,
    /// The pinned commit every file is fetched at and resolved from, a full
    /// 40-character hash: hf-hub answers it from `snapshots/<hash>/` alone,
    /// with no network call and no `refs/main` lookup.
    pub revision: &'a str,
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
    /// The files of one cached snapshot, read straight from
    /// `snapshots/<commit>/`, which the caller has already found complete.
    pub(crate) fn from_snapshot(cache_dir: &Path, repo: &HubRepo<'_>, commit: &str) -> HubFiles {
        let dir = cache_dir
            .join(hub_dir_name(repo.repo))
            .join("snapshots")
            .join(commit);
        let paths = repo
            .files
            .iter()
            .map(|f| ((*f).to_string(), dir.join(f)))
            .collect();
        HubFiles { paths }
    }

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

    /// The `snapshots/<commit>/` directory the files were resolved in.
    pub(crate) fn snapshot_dir(&self) -> Result<PathBuf> {
        self.weights()?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| IndexError::Embedding("the model weights have no directory".into()))
    }
}

/// How long a download may go without a progress event before it is given
/// up: a no-progress watchdog, not an overall timeout, because 558 MB on a
/// slow link is a legitimate long download. See [`ensure_files_with`].
pub(crate) const HUB_STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// The limits one fetch runs under. Production uses [`HubLimits::default`];
/// a test passes a short stall window so it runs in well under a second.
pub(crate) struct HubLimits {
    /// How long one file's download may go without a progress event.
    pub stall: Duration,
}

impl Default for HubLimits {
    fn default() -> Self {
        HubLimits {
            stall: HUB_STALL_TIMEOUT,
        }
    }
}

/// The instant of the last download event of any kind, the clock the stall
/// watchdog in [`ensure_files_with`] reads. It is attached to every download,
/// alone or inside [`ByteProgress`], so the watchdog works whether or not a
/// progress line is drawn. The first window starts at construction, which
/// covers the HEAD and metadata requests that emit no event.
pub(crate) struct StallClock {
    origin: Instant,
    /// Nanoseconds from `origin` to the last event.
    last: AtomicU64,
}

impl StallClock {
    pub(crate) fn new() -> Self {
        StallClock {
            origin: Instant::now(),
            last: AtomicU64::new(0),
        }
    }

    fn tick(&self) {
        let nanos = u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.last.fetch_max(nanos, Ordering::Relaxed);
    }

    /// How long it has been since the last event, or since construction.
    fn quiet_for(&self) -> Duration {
        self.origin
            .elapsed()
            .saturating_sub(Duration::from_nanos(self.last.load(Ordering::Relaxed)))
    }
}

impl ProgressHandler for StallClock {
    fn on_progress(&self, event: &ProgressEvent) {
        if let ProgressEvent::Download(_) = event {
            self.tick();
        }
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
/// the caller knows which file it asked for. Every download event also feeds
/// the fetch's [`StallClock`], so one handler is attached per file.
pub(crate) struct ByteProgress {
    filename: String,
    clock: Arc<StallClock>,
    total: AtomicU64,
    downloaded: AtomicU64,
    last_render: Mutex<Option<Instant>>,
}

impl ByteProgress {
    pub(crate) fn new(filename: &str, clock: Arc<StallClock>) -> Self {
        ByteProgress {
            filename: filename.to_string(),
            clock,
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
        self.clock.on_progress(event);
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
/// never dials out just to check. Every file is fetched at the pinned commit
/// ([`HubRepo::revision`]), so a cache holding another commit counts as not
/// cached here. `announce` false drops the notice and the progress line, for
/// a daemon's quiet background fetch.
///
/// A download that sends nothing for [`HUB_STALL_TIMEOUT`] fails as a
/// download error instead of hanging (see [`ensure_files_with`]). Both the
/// embedding model's first-use download and the contradiction model's come
/// through here, so the watchdog guards both.
pub(crate) async fn ensure_files(
    cache_dir: &Path,
    repo: &HubRepo<'_>,
    announce: bool,
) -> Result<HubFiles> {
    let client = cache_client(cache_dir)?;
    ensure_files_with(&client, cache_dir, repo, announce, &HubLimits::default()).await
}

/// The hub client on a cache directory that exists.
pub(crate) fn cache_client(cache_dir: &Path) -> Result<HFClient> {
    std::fs::create_dir_all(cache_dir).map_err(|e| IndexError::Io {
        path: cache_dir.display().to_string(),
        source: e,
    })?;
    hub_client(cache_dir)
}

/// [`ensure_files`] on a given client, so a test can point the network side
/// at a listener it controls. Every request names `repo.revision`.
///
/// Each file's download runs under a no-progress watchdog: when no download
/// event of any kind arrives for `limits.stall`, the download is dropped and
/// the call fails with "no data arrived", an `Embedding` error the NLI loader
/// relabels `NliFetch`, so the daemon's backoff runs and the pass is released.
/// The first window starts with the request, so a server that accepts the
/// connection and never answers the HEAD fails here. A xet-backed file (the
/// weights are one) is covered up to the start of its data transfer: the
/// HEAD, the token request and the download group setup. During the transfer
/// hf-hub polls hf-xet ten times a second and emits an event each time, so
/// the watchdog stays quiet there, and hf-xet's own read timeout (300 s by
/// default) bounds a connection that stops sending.
///
/// A dropped xet download may leave `spawn_blocking` parts running on
/// detached; the process still leaves through `std::process::exit`. Before a
/// file goes to the network, a `<blob>.incomplete` in the repository's
/// `blobs/` that was written to inside the stall window means such a download
/// may still be writing, and the call refuses with a download error rather
/// than start a second writer into the same blob.
pub(crate) async fn ensure_files_with(
    client: &HFClient,
    cache_dir: &Path,
    repo: &HubRepo<'_>,
    announce: bool,
    limits: &HubLimits,
) -> Result<HubFiles> {
    // The weights alone decide whether this is a first-use download: they are
    // the file worth a notice and a progress line.
    let cached = is_cached(client, repo).await?;
    if !cached && announce {
        eprintln!(
            "crystalline: downloading {} {} to {} (first use, about {} MB)...",
            repo.what,
            repo.repo,
            cache_dir.display(),
            repo.download_mb
        );
    }
    let show_progress = !cached && announce && std::io::stderr().is_terminal();

    let (owner, name) = repo_parts(repo.repo)?;
    let remote = client.model(owner, name);
    let mut paths = IndexMap::with_capacity(repo.files.len());
    for file in repo.files {
        let path = match cached_path(client, repo, file).await? {
            Some(path) => path,
            None => {
                if partial_download_running(cache_dir, repo, limits.stall) {
                    return Err(IndexError::Embedding(format!(
                        "downloading {file} for {} {}: a previous download of {file} may still be running",
                        repo.what, repo.repo
                    )));
                }
                let clock = Arc::new(StallClock::new());
                // Progress is opt in in hf-hub, and the clock needs it, so a
                // handler is always attached; the line is drawn only when
                // `show_progress` says so, which keeps the one visible
                // progress mechanism the one this module TTY-gates itself.
                let handler: Progress = if show_progress {
                    Arc::new(ByteProgress::new(file, clock.clone())).into()
                } else {
                    clock.clone().into()
                };
                let download = remote
                    .download_file()
                    .filename(*file)
                    .revision(repo.revision)
                    .progress(handler)
                    .send();
                match until_stalled(download, &clock, limits.stall).await {
                    Some(fetched) => fetched.map_err(|e| {
                        IndexError::Embedding(format!(
                            "downloading {file} for {} {} at commit {}: {e}",
                            repo.what, repo.repo, repo.revision
                        ))
                    })?,
                    None => {
                        return Err(IndexError::Embedding(format!(
                            "downloading {file} for {} {}: no data arrived for {} s",
                            repo.what,
                            repo.repo,
                            limits.stall.as_secs()
                        )));
                    }
                }
            }
        };
        paths.insert((*file).to_string(), path);
    }
    Ok(HubFiles { paths })
}

/// Runs `fetch` until it finishes, or answers `None` once `clock` has been
/// quiet for `stall`; the unfinished future is dropped then.
async fn until_stalled<F: std::future::Future>(
    fetch: F,
    clock: &StallClock,
    stall: Duration,
) -> Option<F::Output> {
    let mut fetch = std::pin::pin!(fetch);
    let period = (stall / 4).clamp(Duration::from_millis(10), Duration::from_secs(1));
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            out = &mut fetch => return Some(out),
            _ = ticker.tick() => {
                if clock.quiet_for() >= stall {
                    return None;
                }
            }
        }
    }
}

/// True when the repository's `blobs/` holds a `<blob>.incomplete` written to
/// inside the last `stall`. hf-hub's plain HTTP path and its xet path both
/// write there and rename on success, so a fresh one is a download that may
/// still be running, perhaps one this process dropped on a stall. An older
/// one is a leftover that a new download overwrites.
fn partial_download_running(cache_dir: &Path, repo: &HubRepo<'_>, stall: Duration) -> bool {
    let blobs = cache_dir.join(hub_dir_name(repo.repo)).join("blobs");
    let Ok(entries) = std::fs::read_dir(&blobs) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name().to_string_lossy().ends_with(".incomplete")
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < stall)
    })
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
/// air-gapped and CI-prefetch paths must not do. It asks for the pinned
/// commit, which hf-hub answers from `snapshots/<hash>/` alone; without the
/// revision it would look up `refs/main`, which a pinned download never writes.
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
        .revision(repo.revision)
        .local_files_only(true)
        .send()
        .await
    {
        Ok(path) => Ok(Some(path)),
        Err(HFError::LocalEntryNotFound { .. }) => Ok(None),
        Err(e) => Err(IndexError::Embedding(format!(
            "reading the model cache for {name} of {} {} at commit {}: {e}",
            repo.what, repo.repo, repo.revision
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository no Hub serves: the tests point the client at a listener.
    const TEST_REPO: HubRepo<'static> = HubRepo {
        repo: "test-owner/test-model",
        revision: "0123456789abcdef0123456789abcdef01234567",
        files: &["config.json", "model.safetensors"],
        download_mb: 1,
        what: "test model",
    };

    fn test_client(cache: &Path, endpoint: &str) -> HFClient {
        HFClient::builder()
            .endpoint(endpoint)
            .cache_dir(cache.to_path_buf())
            .build()
            .unwrap()
    }

    /// M9 of the 0.22 review: a server that accepts the connection and never
    /// answers must fail the fetch within the stall window, as a download
    /// error the daemon retries later, not hang it.
    #[tokio::test]
    async fn a_download_that_sends_nothing_fails_as_a_fetch_error_and_is_retried_later() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept and hold every connection, never write a byte.
        let _hold = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                if let Ok((s, _)) = listener.accept().await {
                    held.push(s);
                }
            }
        });
        let cache = tempfile::tempdir().unwrap();
        let client = test_client(cache.path(), &format!("http://{addr}"));
        let started = std::time::Instant::now();
        let err = ensure_files_with(
            &client,
            cache.path(),
            &TEST_REPO,
            false,
            &HubLimits {
                stall: Duration::from_millis(300),
            },
        )
        .await
        .err()
        .expect("a server that sends nothing cannot deliver the files");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(err.to_string().contains("no data arrived"), "{err}");
        assert!(err.to_string().contains("config.json"), "{err}");
    }

    /// A `<blob>.incomplete` written to inside the stall window may be a
    /// dropped download still writing, so the retry refuses instead of
    /// starting a second writer into the same blob; an older one is a
    /// leftover and does not stand in the way.
    #[tokio::test]
    async fn a_fresh_partial_download_holds_off_a_new_fetch_and_an_old_one_does_not() {
        let cache = tempfile::tempdir().unwrap();
        let blobs = cache
            .path()
            .join(hub_dir_name(TEST_REPO.repo))
            .join("blobs");
        std::fs::create_dir_all(&blobs).unwrap();
        let partial = blobs.join("abc123.incomplete");
        std::fs::write(&partial, b"part").unwrap();
        // Nothing listens on the discard port, so a fetch that dials out
        // fails at once with a connection error.
        let client = test_client(cache.path(), "http://127.0.0.1:9");
        let limits = HubLimits {
            stall: Duration::from_secs(60),
        };

        let err = ensure_files_with(&client, cache.path(), &TEST_REPO, false, &limits)
            .await
            .err()
            .expect("a fresh partial download refuses the fetch");
        assert!(err.to_string().contains("may still be running"), "{err}");

        std::fs::File::options()
            .write(true)
            .open(&partial)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        let err = ensure_files_with(&client, cache.path(), &TEST_REPO, false, &limits)
            .await
            .err()
            .expect("nothing serves the files");
        assert!(!err.to_string().contains("may still be running"), "{err}");
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
}
