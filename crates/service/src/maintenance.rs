//! The maintenance state file: the small throttle record that tells a Stop
//! hook whether this machine owes the human a consolidation sweep.
//!
//! One JSON file, `<state_dir>/hooks/maintenance.json`, holding which domains
//! were written by a human since the last sweep, when that backlog started,
//! when a sweep last ran and when the human was last nudged about it. It is
//! machine state rather than knowledge: nothing here is ever an engram, it
//! never syncs anywhere and deleting it costs at most one delayed nudge.
//!
//! Three writers, all outside this module:
//!
//! - the REST write handlers (`crate::rest::engrams`), which mark a domain
//!   pending when a human creates, saves or retires an engram in it;
//! - the evolve run recorder (`Engine::evolve_engrams`), which stamps
//!   `last_run_at` and drops the swept domains from the pending list - or,
//!   when the sweep named no scope at all and so covered every registered
//!   domain, empties that list outright;
//! - the Stop hook, which reads the file to decide whether to nudge, starts
//!   the machine's clock with [`record_first_seen`] on its very first call and
//!   stamps `last_nudge_at` with [`record_nudge`] when it asks.
//!
//! Every one of them writes through a recorder here rather than saving a state
//! it loaded earlier, and that is a correctness rule rather than a convention:
//! a caller that decides between its read and its write - the Stop hook is the
//! one that does - would otherwise install the file as it was before the
//! decision and erase whatever landed meanwhile.
//!
//! Concurrency has two halves, and only one of them is last-write-wins.
//!
//! Within this process every write - load, merge, encode, install - runs under
//! `WRITE_LOCK`, a plain mutex held for the few microseconds the sequence
//! takes and never across an `.await`. That is not tidiness: the atomic write
//! underneath ([`crystalline_core::config::save_bytes`]) names its temporary
//! sibling after the process id alone, so two writers in one process share one
//! temporary path, and two unsynchronized handlers could interleave their
//! truncating writes into it and rename the splice into place. The lock is
//! what makes "the file a reader sees is one writer's complete bytes" true
//! here; the rename itself is what makes it true against a reader.
//!
//! Across processes it stays last-write-wins, and deliberately so. Two
//! installs writing at the same instant can lose one of the two merges - the
//! temporary names differ, so the loser is a whole coherent file rather than a
//! splice - and the loser costs a nudge that arrives one session later. The
//! alternative, a lock file around a throttle record, would let a stuck lock
//! block the write path it exists to annotate.
//!
//! For the same reason every writer treats failure as log-and-continue.
//! [`record_pending`], [`record_run`], [`record_nudge`] and
//! [`record_first_seen`] swallow their errors at
//! `tracing::debug` and nothing louder: a knowledge write that succeeded must
//! never be reported as failed because a throttle record could not be
//! updated.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use crystalline_core::config::{self, ConfigError};
use serde::{Deserialize, Serialize};

/// The file name under the state directory's `hooks` folder.
pub const MAINTENANCE_FILE: &str = "maintenance.json";

/// The folder the hook-facing state files live in, beside the daemon's own
/// state rather than mixed into it. The same folder the Stop hook already
/// keeps its per-session files in (`crates/cli/src/hook.rs`), which is why a
/// per-machine file here needs a fixed name rather than a session id.
const HOOKS_DIR: &str = "hooks";

/// The schema version [`save`] stamps. A reader that finds a higher one is
/// reading a file a newer install wrote; every field is optional, so the
/// worst case is a nudge decided on fewer facts than the writer had.
const STATE_VERSION: u32 = 1;

/// What this machine owes the human, and when it last said so.
///
/// Every field is `#[serde(default)]` so a file written by any version of
/// this struct loads, and so a truncated or hand-edited file degrades to the
/// fields it still carries rather than to nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceState {
    /// The schema version, `1` for everything [`save`] writes.
    #[serde(default)]
    pub v: u32,
    /// The domains a human has written to since the last sweep, in the order
    /// they were first written to. Never duplicated.
    #[serde(default)]
    pub pending_domains: Vec<String>,
    /// When the current backlog started: the moment the first domain of this
    /// round went pending, not the most recent one. It is what a "the backlog
    /// is getting old" nudge measures against.
    #[serde(default)]
    pub pending_since: Option<DateTime<Utc>>,
    /// When a consolidation sweep last ran on this machine.
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    /// When the human was last nudged about the backlog. Written by the Stop
    /// hook through [`record_nudge`], which is the only thing that nudges.
    #[serde(default)]
    pub last_nudge_at: Option<DateTime<Utc>>,
    /// When this file was first written on this machine. Written by the Stop
    /// hook through [`record_first_seen`], which owns it: the other recorders
    /// round-trip it untouched, so a human write is never the thing that starts
    /// the clock.
    #[serde(default)]
    pub first_seen: Option<DateTime<Utc>>,
}

/// The maintenance state path, `<state_dir>/hooks/maintenance.json`.
///
/// Errors only when the state directory itself cannot be resolved, which is a
/// broken environment rather than a missing file: the file not existing yet is
/// the normal first-run case and [`load`] answers it with a fresh state.
pub fn path() -> Result<PathBuf, ConfigError> {
    Ok(config::state_dir()?.join(HOOKS_DIR).join(MAINTENANCE_FILE))
}

/// The stored state, or a fresh one on any failure: no state directory, no
/// file, an unreadable file, or bytes that are not this schema.
///
/// Deliberately infallible. Every caller wants the same thing from a failure -
/// treat this machine as owing nothing yet - and a corrupt file heals on the
/// next write rather than blocking it.
pub fn load() -> MaintenanceState {
    match path() {
        Ok(p) => load_from(&p),
        Err(e) => {
            tracing::debug!("maintenance state path unresolved: {e}");
            MaintenanceState::default()
        }
    }
}

/// Write the state atomically, stamping the current schema version.
///
/// The error type is [`ConfigError`] rather than a bare unit so a caller that
/// does care (the hook's own diagnostics) can say what went wrong; every
/// caller in this crate discards it.
pub fn save(state: &MaintenanceState) -> Result<(), ConfigError> {
    save_to(&path()?, state)
}

/// Mark `domain` as carrying human writes that no sweep has looked at yet.
///
/// Load, merge, save: the domain joins the pending list if it is not already
/// there and `pending_since` is set only when the backlog was empty, so a
/// second write to the same domain never resets the age of the backlog.
/// Failures are logged at debug and swallowed - the write this annotates has
/// already landed.
pub fn record_pending(domain: &str) {
    if let Err(e) = path().and_then(|p| record_pending_at(&p, domain)) {
        tracing::debug!("maintenance state not marked pending for '{domain}': {e}");
    }
}

/// Record that a consolidation sweep just ran over `swept_domains`.
///
/// Stamps `last_run_at`, drops exactly the swept domains from the pending list
/// and clears `pending_since` once the list empties. A sweep scoped to one
/// domain therefore settles that domain and leaves the rest of the backlog
/// standing, with its original age intact. Failures are logged at debug and
/// swallowed, for the same reason [`record_pending`] swallows them.
pub fn record_run(swept_domains: &[String]) {
    if let Err(e) = path().and_then(|p| record_run_at(&p, swept_domains)) {
        tracing::debug!("maintenance state not stamped with the sweep: {e}");
    }
}

/// Record that a consolidation sweep just ran over the whole install.
///
/// The unscoped counterpart of [`record_run`]: it stamps `last_run_at` and
/// empties the pending list outright rather than subtracting a scope from it.
/// That difference is what heals the file. A sweep with no scope looked at
/// every registered domain, so any name still standing afterwards is one no
/// scope can ever cover again (a domain a human wrote to and then
/// unregistered), and subtracting the swept set would leave it pending for
/// ever, with the Stop hook naming a ghost once a day. Failures are logged at
/// debug and swallowed, like every writer here.
pub fn record_run_unscoped() {
    if let Err(e) = path().and_then(|p| record_run_unscoped_at(&p)) {
        tracing::debug!("maintenance state not stamped with the full sweep: {e}");
    }
}

/// Record that the human was nudged about the backlog at `now`.
///
/// The Stop hook's half of the file, and the reason it is a recorder rather
/// than a save: the hook reads the state, decides whether to ask and only then
/// stamps, and a `record_pending` from the daemon can land inside that gap.
/// Writing the whole state the hook had read would erase it; stamping the one
/// field under the lock cannot. `now` is the caller's instant, the same one the
/// decision was made against, rather than a fresh clock read. Failures are
/// logged at debug and swallowed, like every writer here.
pub fn record_nudge(now: DateTime<Utc>) {
    if let Err(e) = path().and_then(|p| record_nudge_at(&p, now)) {
        tracing::debug!("maintenance state not stamped with the nudge: {e}");
    }
}

/// Start this machine's clock at `now`, unless it is already running.
///
/// The other half of what the Stop hook owns: the first hook call this machine
/// ever makes seeds [`MaintenanceState::first_seen`], which is what the weekly
/// arm measures against until a sweep is recorded. Merging rather than saving
/// for exactly the reason [`record_nudge`] does, and a no-op once the stamp
/// exists, so a later call never restarts the quiet week. Failures are logged
/// at debug and swallowed.
pub fn record_first_seen(now: DateTime<Utc>) {
    if let Err(e) = path().and_then(|p| record_first_seen_at(&p, now)) {
        tracing::debug!("maintenance state clock not started: {e}");
    }
}

// --- path-taking internals ---------------------------------------------------
//
// The functions above resolve `path()` and swallow; these do the work
// against an explicit file, which is what the unit tests drive so they need
// neither the real state directory nor a process-global environment override.

/// [`load`] against an explicit file.
fn load_from(path: &Path) -> MaintenanceState {
    let Ok(bytes) = std::fs::read(path) else {
        return MaintenanceState::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(e) => {
            tracing::debug!("maintenance state at {} did not parse: {e}", path.display());
            MaintenanceState::default()
        }
    }
}

/// Serializes this process's writers. Held across the whole load-merge-write
/// sequence and never across an `.await`, which is why every function that
/// takes it is synchronous and stays that way.
///
/// Poisoning is ignored (`unwrap_or_else(PoisonError::into_inner)`): a writer
/// that panicked mid-sequence installed nothing, since the install is one
/// rename of a fully written temporary file, so the next writer's own
/// load-merge starts from a coherent file. Aborting the whole daemon over a
/// throttle record would be the larger failure by far.
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`WRITE_LOCK`], ignoring poisoning.
fn write_lock() -> std::sync::MutexGuard<'static, ()> {
    WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`save`] against an explicit file, under [`WRITE_LOCK`].
fn save_to(path: &Path, state: &MaintenanceState) -> Result<(), ConfigError> {
    let _write = write_lock();
    write_locked(path, state)
}

/// Encode and install one state. The caller must already hold [`WRITE_LOCK`],
/// which is why this is separate from [`save_to`]: the recorders below hold it
/// across their load-merge, and a second acquisition here would deadlock on a
/// mutex that is not reentrant.
fn write_locked(path: &Path, state: &MaintenanceState) -> Result<(), ConfigError> {
    let stamped = MaintenanceState {
        v: STATE_VERSION,
        ..state.clone()
    };
    // Compact, like the per-session files the Stop hook already writes beside
    // this one. A serialization failure is impossible for this struct, so it
    // is reported as invalid data at the path rather than given an error
    // variant of its own.
    let bytes = serde_json::to_vec(&stamped).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
    config::save_bytes(path, &bytes)
}

/// [`record_pending`] against an explicit file. The read and the write are one
/// critical section, so a concurrent recorder in this process merges onto what
/// this one installed rather than onto what it read.
fn record_pending_at(path: &Path, domain: &str) -> Result<(), ConfigError> {
    let _write = write_lock();
    let mut state = load_from(path);
    if !state.pending_domains.iter().any(|d| d == domain) {
        state.pending_domains.push(domain.to_string());
    }
    if state.pending_since.is_none() {
        state.pending_since = Some(Utc::now());
    }
    write_locked(path, &state)
}

/// [`record_run`] against an explicit file, one critical section like
/// [`record_pending_at`].
fn record_run_at(path: &Path, swept_domains: &[String]) -> Result<(), ConfigError> {
    let _write = write_lock();
    let mut state = load_from(path);
    state.last_run_at = Some(Utc::now());
    state
        .pending_domains
        .retain(|d| !swept_domains.iter().any(|swept| swept == d));
    if state.pending_domains.is_empty() {
        state.pending_since = None;
    }
    write_locked(path, &state)
}

/// [`record_nudge`] against an explicit file, one critical section like the
/// recorders above. `now` comes from the caller rather than the clock because
/// the hook decided at a particular instant and stamps that same one.
fn record_nudge_at(path: &Path, now: DateTime<Utc>) -> Result<(), ConfigError> {
    let _write = write_lock();
    let mut state = load_from(path);
    state.last_nudge_at = Some(now);
    write_locked(path, &state)
}

/// [`record_first_seen`] against an explicit file, one critical section like
/// the recorders above. The check is inside it too: the first writer to reach
/// this wins, and a later one leaves the stamp it finds alone.
fn record_first_seen_at(path: &Path, now: DateTime<Utc>) -> Result<(), ConfigError> {
    let _write = write_lock();
    let mut state = load_from(path);
    if state.first_seen.is_some() {
        return Ok(());
    }
    state.first_seen = Some(now);
    write_locked(path, &state)
}

/// [`record_run_unscoped`] against an explicit file, one critical section like
/// the two recorders above.
fn record_run_unscoped_at(path: &Path) -> Result<(), ConfigError> {
    let _write = write_lock();
    let mut state = load_from(path);
    state.last_run_at = Some(Utc::now());
    state.pending_domains.clear();
    state.pending_since = None;
    write_locked(path, &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(HOOKS_DIR).join(MAINTENANCE_FILE);
        (dir, path)
    }

    /// The file the public API resolves, spelled out: the hook that reads it
    /// from another process finds it by this shape and nothing else.
    #[test]
    fn the_path_is_the_hooks_folder_under_the_state_dir() {
        assert_eq!(
            path().unwrap(),
            config::state_dir()
                .unwrap()
                .join("hooks")
                .join("maintenance.json")
        );
    }

    #[test]
    fn load_returns_fresh_on_missing_or_corrupt_file() {
        let (dir, path) = scratch();
        assert_eq!(load_from(&path), MaintenanceState::default());

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json at all").unwrap();
        assert_eq!(load_from(&path), MaintenanceState::default());

        // Valid JSON of the wrong shape reads as fresh too rather than as a
        // half-filled state.
        std::fs::write(&path, b"[1, 2, 3]").unwrap();
        assert_eq!(load_from(&path), MaintenanceState::default());
        drop(dir);
    }

    #[test]
    fn record_pending_merges_domains_without_duplicates_and_sets_pending_since_once() {
        let (_dir, path) = scratch();

        record_pending_at(&path, "eng").unwrap();
        let first = load_from(&path);
        assert_eq!(first.v, STATE_VERSION);
        assert_eq!(first.pending_domains, vec!["eng".to_string()]);
        let started = first.pending_since.expect("the backlog started");

        record_pending_at(&path, "ops").unwrap();
        record_pending_at(&path, "eng").unwrap();
        let merged = load_from(&path);
        assert_eq!(
            merged.pending_domains,
            vec!["eng".to_string(), "ops".to_string()],
            "a domain joins once, in the order it was first written to"
        );
        assert_eq!(
            merged.pending_since,
            Some(started),
            "a later write must not reset the age of the backlog"
        );
        assert!(merged.last_run_at.is_none());
    }

    #[test]
    fn record_run_stamps_last_run_and_removes_exactly_the_swept_domains() {
        let (_dir, path) = scratch();
        record_pending_at(&path, "a").unwrap();
        record_pending_at(&path, "b").unwrap();
        let started = load_from(&path).pending_since.unwrap();

        record_run_at(&path, &["a".to_string()]).unwrap();
        let after_first = load_from(&path);
        assert_eq!(after_first.pending_domains, vec!["b".to_string()]);
        assert_eq!(
            after_first.pending_since,
            Some(started),
            "a partial sweep leaves the rest of the backlog at its original age"
        );
        let ran = after_first.last_run_at.expect("the sweep was stamped");

        record_run_at(&path, &["b".to_string()]).unwrap();
        let after_second = load_from(&path);
        assert!(after_second.pending_domains.is_empty());
        assert_eq!(
            after_second.pending_since, None,
            "an empty backlog has no age"
        );
        assert!(after_second.last_run_at.unwrap() >= ran);
    }

    /// A domain nobody marked pending is not invented by sweeping it, and a
    /// sweep over nothing still counts as a run.
    #[test]
    fn record_run_over_an_unpending_or_empty_scope_still_stamps_the_run() {
        let (_dir, path) = scratch();
        record_pending_at(&path, "eng").unwrap();

        record_run_at(&path, &["other".to_string()]).unwrap();
        let after = load_from(&path);
        assert_eq!(after.pending_domains, vec!["eng".to_string()]);
        assert!(after.last_run_at.is_some());

        record_run_at(&path, &[]).unwrap();
        assert_eq!(load_from(&path).pending_domains, vec!["eng".to_string()]);
    }

    /// An unscoped sweep looked at everything this install can reach, so it
    /// empties the backlog rather than subtracting a scope from it. The name
    /// outside the swept scope is the case that matters: a domain a human
    /// wrote to and then unregistered can never appear in any scope again, so
    /// subtracting would leave it pending for ever.
    #[test]
    fn record_run_unscoped_clears_the_whole_backlog_ghosts_included() {
        let (_dir, path) = scratch();
        record_pending_at(&path, "eng").unwrap();
        record_pending_at(&path, "ghost").unwrap();

        record_run_unscoped_at(&path).unwrap();

        let after = load_from(&path);
        assert!(
            after.pending_domains.is_empty(),
            "a full sweep settles the whole backlog: {:?}",
            after.pending_domains
        );
        assert_eq!(after.pending_since, None, "an empty backlog has no age");
        assert!(after.last_run_at.is_some(), "the run was stamped");
    }

    /// The Stop hook's stamp merges rather than overwrites. The hook reads the
    /// file, decides whether to ask, and only then stamps; a daemon-side
    /// `record_pending` landing inside that gap has to survive, which is the one
    /// thing a whole-file save of the state the hook read cannot do.
    #[test]
    fn record_nudge_keeps_a_domain_that_went_pending_after_the_hook_read() {
        let (_dir, path) = scratch();
        record_pending_at(&path, "seed").unwrap();

        // What the Stop hook reads before it decides.
        let read_by_the_hook = load_from(&path);
        assert_eq!(read_by_the_hook.pending_domains, vec!["seed".to_string()]);

        // A human writes through the daemon while the hook is still deciding.
        record_pending_at(&path, "landed-late").unwrap();

        let stamped: DateTime<Utc> = "2026-08-20T10:00:00Z".parse().unwrap();
        record_nudge_at(&path, stamped).unwrap();

        let after = load_from(&path);
        assert_eq!(after.last_nudge_at, Some(stamped), "the ask was stamped");
        assert_eq!(
            after.pending_domains,
            vec!["seed".to_string(), "landed-late".to_string()],
            "the domain that went pending during the decision must survive the stamp"
        );
        assert_eq!(
            after.pending_since, read_by_the_hook.pending_since,
            "the backlog keeps its original age"
        );
    }

    /// The machine's clock starts once and merges like the stamp above: a
    /// second call leaves the first instant standing, and neither call touches
    /// a backlog recorded in between.
    #[test]
    fn record_first_seen_starts_the_clock_once_and_keeps_the_backlog() {
        let (_dir, path) = scratch();
        let first: DateTime<Utc> = "2026-08-01T09:00:00Z".parse().unwrap();
        let later: DateTime<Utc> = "2026-08-20T09:00:00Z".parse().unwrap();

        record_first_seen_at(&path, first).unwrap();
        assert_eq!(load_from(&path).first_seen, Some(first));

        record_pending_at(&path, "eng").unwrap();
        record_first_seen_at(&path, later).unwrap();

        let after = load_from(&path);
        assert_eq!(
            after.first_seen,
            Some(first),
            "a later call never restarts the quiet week"
        );
        assert_eq!(
            after.pending_domains,
            vec!["eng".to_string()],
            "seeding the clock never disturbs the backlog"
        );
    }

    /// Concurrent writers in one process never splice their bytes together and
    /// never lose a domain: the atomic write's temporary sibling is named after
    /// the process id alone, so two handlers writing at once would share one
    /// scratch path without [`WRITE_LOCK`] holding them apart.
    #[test]
    fn concurrent_writers_in_one_process_all_land() {
        let (_dir, path) = scratch();
        let domains: Vec<String> = (0..16).map(|i| format!("d{i}")).collect();

        std::thread::scope(|scope| {
            for domain in &domains {
                let path = path.clone();
                scope.spawn(move || record_pending_at(&path, domain).unwrap());
            }
        });

        let state = load_from(&path);
        assert_eq!(state.v, STATE_VERSION, "the file parsed as this schema");
        let mut landed = state.pending_domains.clone();
        landed.sort();
        let mut expected = domains.clone();
        expected.sort();
        assert_eq!(landed, expected, "every writer's domain survived");
    }

    #[test]
    fn save_round_trips_every_field() {
        let (_dir, path) = scratch();
        let state = MaintenanceState {
            // Deliberately not 1: `save` stamps the current version over
            // whatever a caller carried, which is the one field that does not
            // round-trip.
            v: 99,
            pending_domains: vec!["eng".to_string(), "ops".to_string()],
            pending_since: Some("2026-08-01T10:00:00Z".parse().unwrap()),
            last_run_at: Some("2026-08-02T11:30:00Z".parse().unwrap()),
            last_nudge_at: Some("2026-08-03T12:45:00Z".parse().unwrap()),
            first_seen: Some("2026-07-01T09:15:00Z".parse().unwrap()),
        };

        save_to(&path, &state).unwrap();
        let read_back = load_from(&path);
        assert_eq!(
            read_back,
            MaintenanceState {
                v: STATE_VERSION,
                ..state
            }
        );
        assert!(
            path.exists(),
            "the parent folder is created by the atomic write"
        );
    }
}
