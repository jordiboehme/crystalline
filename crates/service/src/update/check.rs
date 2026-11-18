//! The daemon's passive update check: at most one lookup a day while
//! `update.check` is on, its result cached in `update-check.json` in the
//! state folder, and the facts `status` asks for over the control socket.
//!
//! The check is off by default, and never runs under `NO_UPDATE_NOTIFIER`,
//! `CI`, in a container or in the Claude Desktop extension. It never blocks
//! the daemon's start or its requests: it runs in its own task, after a
//! random wait so many daemons started together do not ask GitHub at the
//! same second. Failures are cached as well, so a broken network costs one
//! request every four hours.
//!
//! Nothing from the network reaches an agent as it came (S23): the cache
//! keeps the tag only as the release client built it, a failure keeps a
//! fixed sentence, and the ETag never leaves the daemon.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crystalline_core::time::{self, Delta, Stamp};
use crystalline_core::update::{
    CheckOutcome, CheckState, DetectedInstall, Detection, LatestRelease, Lookup, OffReason,
    UPDATE_CACHE_FILE, UPDATE_CACHE_FORMAT, UpdateCache, UpdateFacts, detect, official_build,
};
use crystalline_core::version::Precedence;
use tokio::sync::watch;

use super::client::{UpdateError, latest_release};

/// How long a successful check's result stands before the next one: one
/// day.
pub fn check_interval() -> Delta {
    time::hours(24)
}

/// How long a failed check holds off the next one: four hours, so a laptop
/// that started offline or a passing GitHub error does not hide a release
/// for a day.
pub fn retry_interval() -> Delta {
    time::hours(4)
}

/// Whether `cache` still stands at `now`: a release for
/// [`check_interval`], a failure for [`retry_interval`]. A `checked_at` in
/// the future never stands.
pub fn still_fresh(cache: &UpdateCache, now: Stamp) -> bool {
    let within = match cache.outcome {
        CheckOutcome::Release { .. } => check_interval(),
        CheckOutcome::Failed { .. } => retry_interval(),
    };
    cache.fresh(now, within)
}

/// How often the check task wakes to see whether a check is due. A
/// `configure set update.check true` takes effect within this time; a wake
/// with nothing due reads one setting and, with the check on, one small
/// file.
pub const CHECK_WAKE: Duration = Duration::from_secs(5 * 60);

/// How long a lookup may hold the check lock before another process takes
/// it as left behind: well past the lookup's own limit (twice 15 s).
fn lock_stale_after() -> Delta {
    time::minutes(2)
}

/// How often a process that finds the check lock taken looks again.
const LOCK_POLL: Duration = Duration::from_millis(250);

/// The longest random wait before a daemon's first look: short, so a
/// daemon that exits when idle still gets to check. Short-lived daemons
/// still ask no more often than one daemon would, since every look starts
/// from the cache file the last attempt wrote.
pub const MAX_START_JITTER: Duration = Duration::from_secs(60);

/// The file the .deb's package list is read from, as a file and never
/// through `dpkg`.
#[cfg(target_os = "linux")]
const DPKG_LIST: &str = "/var/lib/dpkg/info/crystalline.list";

/// Why the passive check must not run whatever the setting says, as its
/// fixed sentence, or `None`: see [`OffReason::suppressing`]. The setting
/// and the state folder are the caller's to add ([`OffReason::NotTurnedOn`],
/// [`OffReason::NoStateFolder`]).
pub fn check_suppressed(
    env: &dyn Fn(&str) -> Option<String>,
    detection: &Detection,
) -> Option<&'static str> {
    OffReason::suppressing(env, detection).map(|reason| reason.text())
}

/// The cache at `path`, or `None` when there is none, it cannot be read or
/// it does not parse (it is rewritten at the next check).
pub fn read_cache(path: &Path) -> Option<UpdateCache> {
    let text = std::fs::read_to_string(path).ok()?;
    UpdateCache::parse(&text)
}

/// One check against the cache at `cache_path`: while the cached check
/// still stands ([`still_fresh`]: a release for a day, a failure for four
/// hours) it is returned as it is and nothing is asked. Otherwise the
/// newest release is looked up, the result written through a temporary
/// file and a rename, and returned. A write that fails is logged; the
/// result is still returned.
///
/// The lookup runs under a lock file beside the cache, so two processes
/// that share a state folder never both ask: the second waits, then takes
/// the check the first wrote. A lock older than two minutes was left by a
/// process that died and is taken over. Under the lock, temporary files a
/// killed writer left are removed.
///
/// The ETag is sent only when the cache holds a release, so a "not
/// modified" always has one to keep. A release seen before keeps its
/// `first_seen`, and its ETag and publish time when only the fallback
/// answered. The first time a version newer than this binary is seen, one
/// log line `newer release <version> seen` goes to the journal.
pub async fn run_check(cache_path: &Path, now: Stamp) -> UpdateCache {
    checked(cache_path, now, |cache| still_fresh(cache, now), || {}).await
}

/// The check `crystalline update check` asks for: always a lookup, unless
/// another process wrote a check since `now` (its `checked_at` lies
/// between `now` and the clock's present, so a cache dated in the future
/// never counts), which is as new as a second lookup would be.
/// It takes the same lock and writes the same cache as [`run_check`], so
/// the daemon and the CLI never both ask and the file has one writer.
///
/// `on_wait` runs once, the first time the lock is found taken, so the
/// caller can say why it waits: a lock a process left when it died is
/// taken over only after two minutes.
pub async fn check_now(cache_path: &Path, now: Stamp, on_wait: impl FnOnce()) -> UpdateCache {
    let since_now =
        |cache: &UpdateCache| cache.checked_at >= now && cache.checked_at <= time::now();
    checked(cache_path, now, since_now, on_wait).await
}

/// [`run_check`] and [`check_now`]: `stands` says whether a cache on disk
/// answers without a lookup.
async fn checked(
    cache_path: &Path,
    now: Stamp,
    stands: impl Fn(&UpdateCache) -> bool,
    on_wait: impl FnOnce(),
) -> UpdateCache {
    let fresh = || read_cache(cache_path).filter(|c| stands(c));
    if let Some(cache) = fresh() {
        return cache;
    }
    let mut on_wait = Some(on_wait);
    // A folder the lock cannot be made in cannot hold the cache either:
    // the check then goes on without the lock and its write fails.
    let _lock = loop {
        match CheckLock::try_take(cache_path) {
            Ok(Some(lock)) => break Some(lock),
            Ok(None) => {
                if let Some(say) = on_wait.take() {
                    say();
                }
                tokio::time::sleep(LOCK_POLL).await;
                if let Some(cache) = fresh() {
                    return cache;
                }
            }
            Err(_) => break None,
        }
    };
    // Read again under the lock: the holder before may have just written.
    let previous = read_cache(cache_path);
    if let Some(cache) = previous.as_ref().filter(|c| stands(c)) {
        return cache.clone();
    }
    remove_left_temp_files(cache_path);
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
    let temp = sibling(path, &format!(".tmp-{}", std::process::id()));
    // Synced before the rename, so a power loss leaves the old cache or the
    // new one, never an empty file.
    let written = write_synced(&temp, bytes)
        .and_then(|()| before_rename(&temp))
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The check lock, `.update-check.json.lock` beside the cache. It holds a
/// token of its own, and is removed when dropped (also when the lookup is
/// cancelled by a shutdown) only while it still holds that token: a holder
/// whose lock was taken over (a machine that slept in the middle of a
/// lookup) leaves its successor's lock alone.
struct CheckLock {
    path: PathBuf,
    token: String,
}

impl CheckLock {
    /// The lock, `None` while another process holds it, or the error that
    /// kept it from being made.
    fn try_take(cache_path: &Path) -> std::io::Result<Option<CheckLock>> {
        use std::io::Write as _;
        let path = sibling(cache_path, ".lock");
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        for _ in 0..3 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let token = lock_token();
                    file.write_all(token.as_bytes())?;
                    return Ok(Some(CheckLock { path, token }));
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let modified = match std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                        // Released in between: try again.
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(_) => None,
                        Ok(at) => Stamp::from_system_time(at),
                    };
                    if !lock_left(modified, time::now()) {
                        return Ok(None);
                    }
                    // Left by a process that died: take it over.
                    let _ = std::fs::remove_file(&path);
                }
                Err(err) => return Err(err),
            }
        }
        Ok(None)
    }
}

impl Drop for CheckLock {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|held| held == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Whether a lock last written at `modified` was left by a process that
/// died: two minutes old or more, written in the future (a clock set back
/// after a crash, as a future `checked_at` is stale too), or of no readable
/// age.
fn lock_left(modified: Option<Stamp>, now: Stamp) -> bool {
    match modified {
        Some(at) => at > now || now.since(at) >= lock_stale_after(),
        None => true,
    }
}

/// A token no other lock holder has: this process and a random number.
fn lock_token() -> String {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    format!("{}-{:016x}", std::process::id(), u64::from_le_bytes(bytes))
}

/// `.<cache file name><suffix>` beside the cache.
fn sibling(cache_path: &Path, suffix: &str) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(cache_path.file_name().unwrap_or_default());
    name.push(suffix);
    cache_path.with_file_name(name)
}

/// Remove the temporary files a writer killed between its write and its
/// rename left beside the cache. Called under the check lock, so no live
/// writer's file is taken.
fn remove_left_temp_files(cache_path: &Path) {
    let prefix = sibling(cache_path, ".tmp-");
    let (Some(dir), Some(prefix)) = (
        cache_path.parent(),
        prefix.file_name().and_then(|n| n.to_str()),
    ) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    }) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(prefix))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// What the daemon knows about updating, fixed at its start (S18) but for
/// the cache: the install method detected once, so a package upgrade that
/// removes the running exe does not turn it into "unknown"; whether the
/// environment or the method suppresses the check; and the last check,
/// held only in the shape that may leave the daemon
/// ([`UpdateCache::answered`]), so no reader of it ever sees the ETag, a
/// stored failure sentence or an unchecked tag.
#[derive(Debug)]
pub struct UpdateState {
    detection: Detection,
    official: bool,
    suppressed: Option<OffReason>,
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
        let cache = cache_path
            .as_deref()
            .and_then(read_cache)
            .and_then(UpdateCache::answered);
        UpdateState {
            detection,
            official,
            suppressed: OffReason::suppressing(env, &detection),
            cache_path,
            cache: watch::Sender::new(cache),
        }
    }

    /// The detection made at start.
    pub fn detection(&self) -> Detection {
        self.detection
    }

    /// A receiver of the last check, already [`UpdateCache::answered`],
    /// which changes after every check.
    pub fn subscribe(&self) -> watch::Receiver<Option<UpdateCache>> {
        self.cache.subscribe()
    }

    /// The `update_facts` answer: the method detected at start, whether
    /// this is an official build, why `update now` would not act, whether
    /// the check is on given `update.check` (the environment's reasons
    /// win, and without a state folder no check could run), and the last
    /// check, already answered.
    pub fn facts(&self, setting_on: bool) -> UpdateFacts {
        let check = match (self.suppressed, setting_on) {
            (Some(reason), _) => CheckState::Off(reason),
            (None, false) => CheckState::Off(OffReason::NotTurnedOn),
            (None, true) if self.cache_path.is_none() => CheckState::Off(OffReason::NoStateFolder),
            (None, true) => CheckState::On,
        };
        UpdateFacts {
            method: self.detection.method,
            official: self.official,
            refusal: self.detection.refusal,
            check,
            cache: self.cache.borrow().clone(),
        }
    }

    /// Whether a check may run now: nothing suppresses it, the setting is
    /// on and there is a state folder to cache it in.
    fn may_check(&self, setting_on: bool) -> bool {
        setting_on && self.suppressed.is_none() && self.cache_path.is_some()
    }
}

/// How the running binary was installed, as `update check` and `update
/// now` act on it: its own canonical exe under the process environment,
/// with the unofficial-build and app-package refusals added
/// ([`Detection::for_build`]). The daemon keeps its own detection from its
/// start instead (S18).
pub fn detect_here() -> DetectedInstall {
    let env = |name: &str| std::env::var(name).ok();
    // An exe that cannot be named is an empty path, which detects as
    // unknown.
    let exe = std::env::current_exe().unwrap_or_default();
    let found = detect(&exe, &env, dpkg_list().as_deref());
    let packaged = crate::runs_in::PackageContext::here().is_packaged();
    DetectedInstall {
        exe: found.exe,
        detection: found.detection.for_build(official_build(), packaged),
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
/// `update.check` needs no restart) and the last one no longer stands
/// ([`still_fresh`]). That is decided from the cache file, so a new daemon
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
        let Some(path) = state.cache_path.clone() else {
            continue;
        };
        let now = time::now();
        // A fresh file wins over memory: a check another process wrote (a
        // `crystalline update check`, another daemon) reaches the answer
        // within one wake.
        if let Some(file) = read_cache(&path).filter(|c| still_fresh(c, now)) {
            let file = file.answered();
            state.cache.send_if_modified(|held| {
                let changed = *held != file;
                *held = file;
                changed
            });
            continue;
        }
        // Memory still holds a standing check when the file could not be
        // written: no second request before it lapses. A state folder that stays
        // unwritable still costs one request per daemon start.
        if state
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|cache| still_fresh(cache, now))
        {
            continue;
        }
        let cache = tokio::select! {
            _ = crate::daemon::wait_true(&mut shutdown) => break,
            cache = run_check(&path, now) => cache,
        };
        state.cache.send_replace(cache.answered());
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
    fn a_lock_is_left_when_old_in_the_future_or_of_no_age() {
        let now = at(18, 9);
        assert!(!lock_left(Some(now), now));
        assert!(!lock_left(now.checked_sub(time::seconds(119)), now));
        assert!(lock_left(now.checked_sub(time::minutes(2)), now));
        assert!(
            lock_left(now.checked_add(time::minutes(1)), now),
            "a clock set back"
        );
        assert!(lock_left(None, now), "an age that cannot be read");
    }

    #[test]
    fn a_lock_taken_over_is_not_removed_by_its_old_holder() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join(UPDATE_CACHE_FILE);
        let first = CheckLock::try_take(&cache).unwrap().expect("free");
        assert!(CheckLock::try_take(&cache).unwrap().is_none(), "held");
        // A successor took it over while the first slept.
        std::fs::remove_file(&first.path).unwrap();
        let second = CheckLock::try_take(&cache).unwrap().expect("free again");
        drop(first);
        assert!(second.path.exists(), "the successor's lock stays");
        assert!(CheckLock::try_take(&cache).unwrap().is_none());
        drop(second);
        assert!(!dir.path().join(".update-check.json.lock").exists());
    }

    #[test]
    fn the_start_jitter_stays_under_its_bound() {
        for _ in 0..100 {
            assert!(start_jitter() < MAX_START_JITTER);
        }
    }
}
