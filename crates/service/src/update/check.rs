//! The daemon's passive update check: at most one lookup a day while
//! `update.check` is on, its result cached in `update-check.json` in the
//! state folder, and the facts `status` asks for over the control socket.
//!
//! The check is off by default, and never runs under `NO_UPDATE_NOTIFIER`,
//! `CI`, in a container or in the Claude Desktop extension. It never blocks
//! the daemon's start or its requests: it runs in its own task, after a
//! random wait so many daemons started together do not ask GitHub at the
//! same second. Failures are cached as well, so a broken network costs one
//! request a day.
//!
//! Nothing from the network reaches an agent as it came (S23): the cache
//! keeps the tag only as the release client built it, a failure keeps a
//! fixed sentence, and the ETag never leaves the daemon.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crystalline_core::time::{self, Delta, Stamp};
use crystalline_core::update::{
    CheckOutcome, Detection, InstallMethod, LatestRelease, Lookup, UPDATE_CACHE_FILE,
    UPDATE_CACHE_FORMAT, UpdateCache, detect, official_build,
};
use crystalline_core::version::Precedence;
use serde_json::{Value, json};
use tokio::sync::watch;

use super::client::{UpdateError, latest_release};

/// How long a check's result stands before the next one: one day, failed
/// checks included.
pub fn check_interval() -> Delta {
    time::hours(24)
}

/// How often the check task wakes to see whether a check is due. A
/// `configure set update.check true` takes effect within this time.
pub const CHECK_WAKE: Duration = Duration::from_secs(60 * 60);

/// The longest random wait before a daemon's first look: short, so a
/// daemon that exits when idle still gets to check. Short-lived daemons
/// still ask at most once a day, since every look starts from the cache
/// file the last attempt wrote.
pub const MAX_START_JITTER: Duration = Duration::from_secs(60);

/// The reason the check is off when nothing suppresses it but the setting.
pub const OFF_BY_SETTING: &str = "not turned on";

/// The file the .deb's package list is read from, as a file and never
/// through `dpkg`.
#[cfg(target_os = "linux")]
const DPKG_LIST: &str = "/var/lib/dpkg/info/crystalline.list";

/// Why the passive check must not run whatever the setting says, or `None`:
/// `NO_UPDATE_NOTIFIER` or `CI` set to anything but an empty value, a
/// Docker install or the Claude Desktop extension. The method is asked, not
/// the refusal, since an unofficial build replaces the refusal and keeps the
/// method. The setting itself is the caller's to add ([`OFF_BY_SETTING`]).
pub fn check_suppressed(
    env: &dyn Fn(&str) -> Option<String>,
    detection: &Detection,
) -> Option<&'static str> {
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    if set("NO_UPDATE_NOTIFIER") {
        Some("NO_UPDATE_NOTIFIER is set")
    } else if set("CI") {
        Some("CI is set")
    } else if detection.method == InstallMethod::Docker {
        Some("Docker")
    } else if detection.method == InstallMethod::Mcpb {
        Some("Claude Desktop extension")
    } else {
        None
    }
}

/// The cache at `path`, or `None` when there is none, it cannot be read or
/// it does not parse (it is rewritten at the next check).
pub fn read_cache(path: &Path) -> Option<UpdateCache> {
    let text = std::fs::read_to_string(path).ok()?;
    UpdateCache::parse(&text)
}

/// One check against the cache at `cache_path`: when the cached check is
/// younger than a day (a failed one too) it is returned as it is and
/// nothing is asked. Otherwise the newest release is looked up, the result
/// written through a temporary file and a rename, and returned. A write
/// that fails is logged; the result is still returned.
///
/// The ETag is sent only when the cache holds a release, so a "not
/// modified" always has one to keep. A release seen before keeps its
/// `first_seen`, and its ETag and publish time when only the fallback
/// answered. The first time a version newer than this binary is seen, one
/// log line `newer release <version> seen` goes to the journal.
pub async fn run_check(cache_path: &Path, now: Stamp) -> UpdateCache {
    let previous = read_cache(cache_path);
    if let Some(cache) = previous.as_ref().filter(|c| c.fresh(now, check_interval())) {
        return cache.clone();
    }
    let etag = previous
        .as_ref()
        .filter(|c| matches!(c.outcome, CheckOutcome::Release { .. }))
        .and_then(|c| c.etag.as_deref());
    let lookup = latest_release(etag).await;
    if let Err(err) = &lookup {
        tracing::info!("update check failed: {err}");
    }
    let next = next_cache(previous.as_ref(), lookup, now);
    if let Some(version) = newly_seen(previous.as_ref(), &next) {
        tracing::info!("newer release {} seen", version.render());
    }
    if let Err(err) = write_cache(cache_path, &next) {
        tracing::warn!(
            "could not write the update check cache {}: {err}",
            cache_path.display()
        );
    }
    next
}

/// The cache after a lookup, from the one before it.
fn next_cache(
    previous: Option<&UpdateCache>,
    lookup: Result<Lookup, UpdateError>,
    now: Stamp,
) -> UpdateCache {
    // A failed attempt keeps the release a check found before it, with its
    // first_seen, beside it.
    let failed = |reason: String| UpdateCache {
        v: UPDATE_CACHE_FORMAT,
        checked_at: now,
        outcome: CheckOutcome::Failed { reason },
        etag: None,
        last_release: previous.and_then(UpdateCache::known_release),
    };
    match lookup {
        Err(err) => failed(failure_reason(&err)),
        Ok(Lookup::NotModified) => match previous {
            Some(cache) if matches!(cache.outcome, CheckOutcome::Release { .. }) => UpdateCache {
                checked_at: now,
                ..cache.clone()
            },
            // Only reachable when GitHub answers 304 to a request without an
            // ETag: there is nothing to keep.
            _ => failed("GitHub's answer could not be read".to_string()),
        },
        Ok(Lookup::Fresh(release, etag)) => fresh_cache(previous, release, etag, now),
    }
}

fn fresh_cache(
    previous: Option<&UpdateCache>,
    release: LatestRelease,
    etag: Option<String>,
    now: Stamp,
) -> UpdateCache {
    // The release known before, also across a failed attempt in between.
    let same = previous.and_then(|cache| {
        cache
            .known_release()
            .filter(|known| known.version == release.tag)
            .map(|known| (known.published_at, known.first_seen, cache.etag.clone()))
    });
    let (published_at, first_seen, etag) = match same {
        // The fallback carries neither an ETag nor a publish time: keep the
        // ones the API gave for this same release. A failed attempt keeps
        // no ETag, so there is none to keep after one.
        Some((published_before, first_seen, etag_before)) => (
            release.published_at.or(published_before),
            first_seen,
            etag.or(etag_before),
        ),
        None => (release.published_at, now, etag),
    };
    UpdateCache {
        v: UPDATE_CACHE_FORMAT,
        checked_at: now,
        outcome: CheckOutcome::Release {
            version: release.tag,
            published_at,
            first_seen,
        },
        etag,
        last_release: None,
    }
}

/// The version `next` names when it is newer than this binary and was not
/// the release `previous` knew, also across a failed attempt, the one
/// moment the journal hears of it.
fn newly_seen(previous: Option<&UpdateCache>, next: &UpdateCache) -> Option<Precedence> {
    let current = Precedence::parse(crystalline_core::VERSION)?;
    let found = next.newer_release(&current)?;
    let CheckOutcome::Release { version, .. } = &next.outcome else {
        return None;
    };
    let seen_before = previous
        .and_then(UpdateCache::known_release)
        .is_some_and(|known| known.version == *version);
    (!seen_before).then_some(found)
}

/// The fixed sentence a failed check is cached with. A connection error's
/// own text stays in the log: the cache holds nothing the network chose.
fn failure_reason(err: &UpdateError) -> String {
    match err {
        UpdateError::Http(_) => "cannot reach GitHub".to_string(),
        UpdateError::Io(_) => "cannot write the download".to_string(),
        other => other.to_string(),
    }
}

/// Write `cache` to `path` through a temporary file beside it and a rename,
/// creating the state folder when it is missing.
fn write_cache(path: &Path, cache: &UpdateCache) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(cache).map_err(std::io::Error::other)?;
    write_atomically(path, text.as_bytes(), |_| Ok(()))
}

/// Write `bytes` to a temporary sibling of `path`, run `before_rename` on
/// it, then rename it into place. On any error the temporary file is
/// removed and `path` is left as it was.
fn write_atomically(
    path: &Path,
    bytes: &[u8],
    before_rename: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".tmp-{}", std::process::id()));
    let temp = path.with_file_name(name);
    let written = std::fs::write(&temp, bytes)
        .and_then(|()| before_rename(&temp))
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// What the daemon knows about updating, fixed at its start (S18) but for
/// the cache: the install method detected once, so a package upgrade that
/// removes the running exe does not turn it into "unknown"; whether the
/// environment or the method suppresses the check; and the last check.
#[derive(Debug)]
pub struct UpdateState {
    detection: Detection,
    official: bool,
    suppressed: Option<&'static str>,
    cache_path: Option<PathBuf>,
    cache: watch::Sender<Option<UpdateCache>>,
}

impl UpdateState {
    /// The state of the running daemon: its own exe, the process
    /// environment and the cache in the state folder, loaded now so the
    /// facts are there before any check.
    pub fn at_start() -> UpdateState {
        let cache_path = crystalline_core::config::state_dir()
            .ok()
            .map(|dir| dir.join(UPDATE_CACHE_FILE));
        let env = |name: &str| std::env::var(name).ok();
        // An exe that cannot be named is an empty path, which detects as
        // unknown.
        let exe = std::env::current_exe().unwrap_or_default();
        UpdateState::detect(&exe, &env, cache_path)
    }

    /// The state for the binary at `exe` under `env`, with the cache at
    /// `cache_path` read once. The detection is made here and kept.
    pub fn detect(
        exe: &Path,
        env: &dyn Fn(&str) -> Option<String>,
        cache_path: Option<PathBuf>,
    ) -> UpdateState {
        let detection = detect(exe, env, dpkg_list().as_deref()).detection;
        let official = official_build();
        let packaged = crate::runs_in::PackageContext::here().is_packaged();
        let detection = detection.for_build(official, packaged);
        let cache = cache_path.as_deref().and_then(read_cache);
        UpdateState {
            detection,
            official,
            suppressed: check_suppressed(env, &detection),
            cache_path,
            cache: watch::Sender::new(cache),
        }
    }

    /// The detection made at start.
    pub fn detection(&self) -> Detection {
        self.detection
    }

    /// A receiver of the last check, which changes after every check.
    pub fn subscribe(&self) -> watch::Receiver<Option<UpdateCache>> {
        self.cache.subscribe()
    }

    /// The `update_facts` answer: the method's label, whether this is an
    /// official build, `on` or `off: <reason>` given whether `update.check`
    /// is on, and the last check without its ETag (`null` when there is
    /// none).
    pub fn facts(&self, setting_on: bool) -> Value {
        let check = match (self.suppressed, setting_on) {
            (Some(reason), _) => format!("off: {reason}"),
            (None, false) => format!("off: {OFF_BY_SETTING}"),
            (None, true) => "on".to_string(),
        };
        let cache = self.cache.borrow().clone().map(|mut cache| {
            cache.etag = None;
            cache
        });
        json!({
            "method": self.detection.method.label(),
            "official": self.official,
            "check": check,
            "cache": cache,
        })
    }

    /// Whether a check may run now: nothing suppresses it, the setting is
    /// on and there is a state folder to cache it in.
    fn may_check(&self, setting_on: bool) -> bool {
        setting_on && self.suppressed.is_none() && self.cache_path.is_some()
    }
}

/// The text of the .deb's package list, when this system has one.
fn dpkg_list() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(DPKG_LIST).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// A random wait of up to [`MAX_START_JITTER`] before the first look.
pub fn start_jitter() -> Duration {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return MAX_START_JITTER / 2;
    }
    let max = MAX_START_JITTER.as_millis() as u64;
    Duration::from_millis(u64::from_le_bytes(bytes) % max.max(1))
}

/// The daemon's check task: waits `first_wait`, then every `wake` runs a
/// check when one may run (`setting_on` is read each time, so a change of
/// `update.check` needs no restart) and the last one is a day old. "A day
/// old" is decided by [`run_check`] from the cache file, so a new daemon
/// never asks sooner than the last attempt of an earlier one allows. Ends
/// on the shutdown signal, also in the middle of a lookup.
pub async fn run_update_checks(
    state: std::sync::Arc<UpdateState>,
    setting_on: impl Fn() -> bool,
    first_wait: Duration,
    wake: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut wait = first_wait;
    loop {
        tokio::select! {
            _ = crate::daemon::wait_true(&mut shutdown) => break,
            _ = tokio::time::sleep(wait) => {}
        }
        wait = wake;
        if !state.may_check(setting_on()) {
            continue;
        }
        let now = time::now();
        if state
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|cache| cache.fresh(now, check_interval()))
        {
            continue;
        }
        let Some(path) = state.cache_path.clone() else {
            continue;
        };
        let cache = tokio::select! {
            _ = crate::daemon::wait_true(&mut shutdown) => break,
            cache = run_check(&path, now) => cache,
        };
        state.cache.send_replace(Some(cache));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn at(day: u32, hour: u32) -> Stamp {
        Stamp::from_ymd_hms(2026, 11, day, hour, 0, 0).unwrap()
    }

    fn release(tag: &str, published_at: Option<Stamp>) -> LatestRelease {
        LatestRelease {
            tag: tag.to_string(),
            version: crystalline_core::version::release_version(tag).unwrap(),
            published_at,
            digests: BTreeMap::new(),
            from_api: published_at.is_some(),
        }
    }

    fn cached(tag: &str, published_at: Option<Stamp>, first_seen: Stamp) -> UpdateCache {
        UpdateCache {
            v: UPDATE_CACHE_FORMAT,
            checked_at: first_seen,
            outcome: CheckOutcome::Release {
                version: tag.to_string(),
                published_at,
                first_seen,
            },
            etag: Some("\"e1\"".to_string()),
            last_release: None,
        }
    }

    #[test]
    fn the_fallback_keeps_the_etag_and_publish_time_of_the_same_release() {
        let before = cached("v99.0.0", Some(at(10, 8)), at(10, 9));
        let next = next_cache(
            Some(&before),
            Ok(Lookup::Fresh(release("v99.0.0", None), None)),
            at(12, 9),
        );
        assert_eq!(next.etag.as_deref(), Some("\"e1\""));
        assert_eq!(next.checked_at, at(12, 9));
        assert_eq!(
            next.outcome,
            CheckOutcome::Release {
                version: "v99.0.0".to_string(),
                published_at: Some(at(10, 8)),
                first_seen: at(10, 9),
            }
        );
    }

    #[test]
    fn a_new_version_starts_its_own_first_seen() {
        let before = cached("v98.0.0", Some(at(10, 8)), at(10, 9));
        let next = next_cache(
            Some(&before),
            Ok(Lookup::Fresh(
                release("v99.0.0", Some(at(11, 8))),
                Some("\"e2\"".to_string()),
            )),
            at(12, 9),
        );
        assert_eq!(
            next.outcome,
            CheckOutcome::Release {
                version: "v99.0.0".to_string(),
                published_at: Some(at(11, 8)),
                first_seen: at(12, 9),
            }
        );
        assert_eq!(next.etag.as_deref(), Some("\"e2\""));
        assert!(
            next.newer_release(&Precedence::parse("0.1.0").unwrap())
                .is_some()
        );
        assert_eq!(
            newly_seen(Some(&before), &next).map(|v| v.render()),
            Some("99.0.0".to_string())
        );
        assert_eq!(newly_seen(Some(&next), &next), None, "seen once only");
    }

    #[test]
    fn not_modified_keeps_the_release_and_moves_checked_at() {
        let before = cached("v99.0.0", Some(at(10, 8)), at(10, 9));
        let next = next_cache(Some(&before), Ok(Lookup::NotModified), at(12, 9));
        assert_eq!(
            next,
            UpdateCache {
                checked_at: at(12, 9),
                ..before
            }
        );
    }

    #[test]
    fn a_failure_keeps_a_fixed_sentence_and_no_etag() {
        let before = cached("v99.0.0", Some(at(10, 8)), at(10, 9));
        let next = next_cache(Some(&before), Err(UpdateError::Status(502)), at(12, 9));
        assert_eq!(
            next.outcome,
            CheckOutcome::Failed {
                reason: "GitHub answered with status 502".to_string()
            }
        );
        assert_eq!(next.etag, None);
        assert_eq!(newly_seen(Some(&before), &next), None);
    }

    #[test]
    fn a_failed_rename_leaves_the_old_cache_and_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(UPDATE_CACHE_FILE);
        let good = cached("v99.0.0", Some(at(10, 8)), at(10, 9));
        write_cache(&path, &good).unwrap();

        let mut seen_temp = None;
        let err = write_atomically(&path, b"{\"v\":1,\"half", |temp| {
            seen_temp = Some(temp.to_path_buf());
            Err(std::io::Error::other("forced between write and rename"))
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "forced between write and rename");
        let temp = seen_temp.expect("the hook ran");
        assert_eq!(
            temp.parent(),
            Some(dir.path()),
            "the temp file is a sibling"
        );
        assert!(!temp.exists(), "the temporary file is removed");
        assert_eq!(read_cache(&path), Some(good), "the old cache is intact");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(UPDATE_CACHE_FILE)]);
    }

    #[test]
    fn the_start_jitter_stays_under_its_bound() {
        for _ in 0..100 {
            assert!(start_jitter() < MAX_START_JITTER);
        }
    }
}
