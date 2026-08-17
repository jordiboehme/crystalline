//! `crystalline hook stop`: the once-per-session capture nudge.
//!
//! Static like `verify` and `prompt`: no database, service or daemon
//! connection is used, and a call completes in tens of milliseconds. A
//! harness (Claude Code, Codex) wires this to its Stop lifecycle event,
//! feeding it a small JSON payload over stdin; on the one call per session
//! that matters, a single line of JSON goes to stdout asking the agent to
//! review the conversation for durable learnings before it finishes. Every
//! other call is silent: exit 0, empty stdout. This is not a style
//! preference, it is a correctness requirement - a harness that sees any
//! other output on a lifecycle hook's stdout can misinterpret it (Codex
//! rejects a plain-text Stop response outright), so a bail path never
//! prints a word, logs a warning or returns a nonzero exit code. A hook must
//! never be the reason a harness's turn breaks.
//!
//! The bail order, every one of them silent: the stdin payload fails to
//! parse; the payload names an event other than `Stop`; `stop_hook_active`
//! is set (the loop guard both harnesses attach to a hook-caused
//! continuation, so a second pass over the same turn never doubles up and
//! never advances the stop counter); the session id fails
//! [`valid_session_id`] (a traversal attempt or a malformed id must never
//! turn into a filesystem path); the config overlay fails to load, resolves
//! to zero registered domains or the effective mode is read-only (nothing
//! worth capturing into, in any of the three cases); the session was
//! already nudged; or the transcript falls below the substance threshold.
//! Passing every one of those is what earns the nudge. The payload checks
//! run before any filesystem or config work; [`decide`] re-checks them so
//! the whole table stays testable as one pure function.
//!
//! State is one small JSON file per session at
//! `<state_dir>/hooks/<session_id>.json`, written before the nudge (if any)
//! is printed, so a crash between the write and the print can only cost a
//! missed nudge, never a repeated one. Every call also opportunistically
//! sweeps state files older than a week, so a long-lived install never
//! accumulates one file per session forever.
//!
//! One more file lives beside those, `maintenance.json`, and it is
//! per-machine rather than per-session: the throttle record
//! [`crystalline_service::maintenance`] keeps, saying which domains a human
//! has written to since the last consolidation sweep and when this machine
//! last swept or last asked. This handler reads it on every call that gets
//! as far as a decision, seeds its `first_seen` stamp the first time it ever
//! runs, and on the sessions where the capture nudge already fired it may
//! append a second paragraph asking for a sweep - a ride-along, never a
//! reason of its own to speak. Being long-lived by design, that file is the
//! one thing the stale sweep above spares.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crystalline_core::config;
use crystalline_service::maintenance::{self, MAINTENANCE_FILE, MaintenanceState};

/// The reminder printed on the one Stop call per session that earns it. Exact
/// wording is load-bearing: it is the whole of what the agent sees, so it
/// names the capture skill's own shape (propose, name the domain and the
/// folder that fits, wait for a yes) rather than re-describing it loosely. It also now carries the
/// salience-feedback half of the learning loop, closing it directly (no
/// propose-first) because that mirrors the routing prompt's own standing
/// bullet to raise the salience of an engram that turned out to be the key
/// to a task. It carries the reconcile half too: a correction that makes an
/// already captured engram wrong is an edit or a supersession, not a second
/// engram written beside the first.
pub const NUDGE_REASON: &str = "Review this conversation for durable learnings before finishing: new facts, decisions, patterns and antipatterns, gotchas, corrections from the user or researched answers worth keeping. Corrections include ones that make an existing engram wrong - for those propose the reconciling edit or supersession, not a new capture beside the old. If any are not yet captured, propose capturing each one as an engram into the fitting crystalline domain: name the insight, the domain and the folder when one fits and wait for a yes. If a recalled engram proved to be the key to the task, raise its salience. If nothing qualifies or everything is already captured, finish normally without mentioning this check.";

/// The ride-along maintenance ask, appended to [`NUDGE_REASON`] on the
/// sessions where the throttle says evolve is due. Wording is load-bearing:
/// it names the tool, restates the two authority classes and carries the
/// human-authored contract.
pub const EVOLVE_NUDGE_REASON: &str = "Also due now: knowledge maintenance. Call the crystalline evolve_engrams tool and work the queue it returns: apply mechanical findings directly and summarize once at the end; propose judgment findings one at a time and wait for a yes. Engrams captured by a person are judgment class - never rewrite a human's words without asking.";

/// How long a machine may go without a consolidation sweep before the ask
/// arms itself: one week. Measured from the last recorded sweep, or from
/// [`MaintenanceState::first_seen`] when this machine has never swept.
pub const EVOLVE_RUN_INTERVAL_DAYS: i64 = 7;

/// The floor between two maintenance asks, whichever arm raised them: a day.
/// Both arms are gated by it, so a backlog that stays pending asks once a
/// day rather than once a session.
pub const EVOLVE_NUDGE_COOLDOWN_HOURS: i64 = 24;

/// A transcript byte size at or above this is substantial on its own, no
/// read required: `run_stop` stats the file first and only reads it when the
/// size falls short of this bar.
const SUBSTANCE_BYTES: u64 = 32_768;

/// A transcript with at least this many newlines within the first
/// `SUBSTANCE_BYTES` bytes is substantial even when its total size is small,
/// so a session made of many short turns still qualifies.
const SUBSTANCE_LINES: usize = 20;

/// How many Stop events with no readable transcript this handler tolerates
/// before nudging anyway. A harness that never sends `transcript_path` (or
/// sends one this process cannot read) still deserves a nudge eventually
/// rather than staying silent for the life of the session.
const FALLBACK_STOPS: u32 = 3;

/// How long a session's state file is kept before the opportunistic sweep in
/// `run_stop` removes it: seven days, in seconds. Nothing schedules the
/// sweep; it just rides along on every invocation.
const STATE_STALE_SECS: u64 = 7 * 24 * 60 * 60;

/// The state file's schema version, bumped only if the shape below changes
/// incompatibly.
const STATE_VERSION: u32 = 1;

/// The stdin payload a Stop hook sends. Every field carries a serde default,
/// since a harness is free to add fields this handler does not know about
/// yet, or to omit one it considers optional; an unparseable payload (wrong
/// types, invalid JSON) is the only shape that fails to deserialize at all,
/// and that failure is itself a silent bail in [`run_stop`].
#[derive(Debug, Clone, Deserialize)]
pub struct StopInput {
    /// The harness's identifier for this session. Validated by
    /// [`valid_session_id`] before it ever becomes part of a filesystem path.
    #[serde(default)]
    pub session_id: String,
    /// The transcript file for this session, when the harness provides one.
    /// `None` for a harness that omits the field, sends `null` or points at
    /// a path this process cannot stat or read - all three fall back to the
    /// per-session stop counter rather than the substance threshold.
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    /// Set by the harness on a hook-caused continuation of the same turn:
    /// the loop guard both Claude Code and Codex attach to a Stop payload
    /// that resulted from an earlier hook decision. `true` means this call
    /// must never itself produce a second nudge.
    #[serde(default)]
    pub stop_hook_active: bool,
    /// The event name the harness attaches to the payload, when it does.
    /// This handler is wired to the Stop event only, so a name other than
    /// `Stop` is a defensive bail; an absent name is treated as fine, since
    /// not every harness stamps it.
    #[serde(default)]
    pub hook_event_name: Option<String>,
}

/// The persisted state for one session, at
/// `<state_dir>/hooks/<session_id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// The schema version.
    pub v: u32,
    /// How many Stop events have been recorded for this session so far,
    /// counting the one that produced this state.
    pub stops: u32,
    /// Whether this session has already been nudged. Once true, every later
    /// Stop for the same session stays silent regardless of transcript size.
    pub nudged: bool,
    /// When this file was last written.
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SessionState {
    /// The state a session starts from when no file exists yet, or the
    /// existing one could not be read or parsed. A corrupt state file is
    /// treated exactly like a missing one rather than aborting the hook: a
    /// worn nudge counter restarting at zero is a far smaller cost than a
    /// hook that starts failing loudly.
    fn fresh() -> SessionState {
        SessionState {
            v: STATE_VERSION,
            stops: 0,
            nudged: false,
            updated_at: chrono::Utc::now(),
        }
    }
}

/// How substantial a transcript looks, computed once by [`run_stop`] and
/// handed to [`decide`] so the decision itself stays a pure function of
/// already-gathered facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptStats {
    /// No path was given, or this process could not stat or read it. Falls
    /// back to the per-session stop counter.
    Unavailable,
    /// The transcript was measured and stayed below both thresholds.
    Insufficient,
    /// The transcript's size or line count reached the substance threshold.
    Substantial,
}

/// What a Stop call resolves to: print the nudge, or say nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopDecision {
    /// Exit 0, print nothing.
    Silent,
    /// Print the single-line block JSON reminder.
    Nudge,
}

/// Whether `id` is safe to use as a filename component: 1 to 128 ASCII
/// letters, digits, hyphens or underscores. This is the traversal defense
/// for [`state_path`] - no `/`, no `.`, no `..`, no empty string, nothing
/// outside plain ASCII - so a malformed or hostile session id can never walk
/// the state file outside `<state_dir>/hooks/`.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The pure decision core: given the parsed payload, whether any domain is
/// registered, whether the effective mode is read-only, the session's prior
/// state and how substantial the transcript looked, decide whether this call
/// earns a nudge. Every one of the bail conditions documented on the module
/// itself lives here except the three that must run before any state or
/// config IO happens (parse failure, an invalid session id and the overlay
/// load itself) - those short-circuit inside [`run_stop`] before this
/// function is ever reached.
pub fn decide(
    input: &StopInput,
    has_domains: bool,
    read_only: bool,
    state: &SessionState,
    transcript: TranscriptStats,
) -> StopDecision {
    if let Some(name) = input.hook_event_name.as_deref()
        && name != "Stop"
    {
        return StopDecision::Silent;
    }
    if input.stop_hook_active {
        return StopDecision::Silent;
    }
    if !has_domains || read_only {
        return StopDecision::Silent;
    }
    if state.nudged {
        return StopDecision::Silent;
    }
    match transcript {
        TranscriptStats::Substantial => StopDecision::Nudge,
        TranscriptStats::Insufficient => StopDecision::Silent,
        // No readable transcript: fall back to the per-session counter.
        // `state.stops` is the count going into this call, so a value of
        // `FALLBACK_STOPS - 1` (two prior Stops recorded) means this is the
        // third Stop overall, the one that fires.
        TranscriptStats::Unavailable => {
            if state.stops >= FALLBACK_STOPS - 1 {
                StopDecision::Nudge
            } else {
                StopDecision::Silent
            }
        }
    }
}

/// Whether this machine owes the human a consolidation sweep, and which
/// domains a human has written to since the last one.
///
/// Pure, and deliberately separate from [`decide`]: the capture nudge is
/// decided without ever consulting this, and only a call that already earned
/// that nudge asks here whether a maintenance paragraph rides along. `Some`
/// carries the pending domains, which is empty for an ask raised by the
/// weekly arm alone.
///
/// Due when the cooldown is clear and either arm is up:
///
/// - the weekly arm, [`EVOLVE_RUN_INTERVAL_DAYS`] since the last sweep, or
///   since this file was first seen when no sweep was ever recorded - so a
///   fresh install gets a quiet first week rather than an ask on day one;
/// - the pending arm, any domain a human has written to since the last
///   sweep. It fires inside the quiet week too: a human write is evidence
///   there is something to consolidate, which the calendar alone is not.
///
/// [`EVOLVE_NUDGE_COOLDOWN_HOURS`] gates both arms, so a machine with a
/// standing backlog asks once a day at most.
pub fn evolve_ask(m: &MaintenanceState, now: DateTime<Utc>) -> Option<Vec<String>> {
    if let Some(nudged) = m.last_nudge_at
        && now.signed_duration_since(nudged) < TimeDelta::hours(EVOLVE_NUDGE_COOLDOWN_HOURS)
    {
        return None;
    }
    let interval = TimeDelta::days(EVOLVE_RUN_INTERVAL_DAYS);
    let weekly_due = match m.last_run_at {
        Some(ran) => now.signed_duration_since(ran) >= interval,
        // No sweep ever recorded: the clock runs from the first time this
        // hook saw the machine. Absent (the very first call, which seeds it)
        // nothing is due yet.
        None => m
            .first_seen
            .is_some_and(|seen| now.signed_duration_since(seen) >= interval),
    };
    if weekly_due || !m.pending_domains.is_empty() {
        Some(m.pending_domains.clone())
    } else {
        None
    }
}

/// The full reason string this handler prints, composed from the capture
/// nudge and, when [`evolve_ask`] said the sweep is due, the maintenance
/// paragraph. The pending domains are named only when there are any: an ask
/// raised by the weekly arm alone has nothing to point at.
fn nudge_reason(evolve: Option<&[String]>) -> String {
    let Some(domains) = evolve else {
        return NUDGE_REASON.to_string();
    };
    if domains.is_empty() {
        format!("{NUDGE_REASON} {EVOLVE_NUDGE_REASON}")
    } else {
        format!(
            "{NUDGE_REASON} {EVOLVE_NUDGE_REASON} Focus domains: {}.",
            domains.join(", ")
        )
    }
}

/// Run the `hook stop` command: read the payload from stdin, decide, persist
/// state and print the nudge when earned. Never panics on ordinary bad
/// input and never returns an error - every failure mode this function can
/// reach degrades to silence, per the module's binding contract, so the
/// caller in `main.rs` always exits 0.
pub fn run_stop() {
    let mut raw = String::new();
    // A defensive cap: a Stop payload is a few hundred bytes, so a megabyte
    // is generous. A misbehaving harness feeding an endless stream gets cut
    // off instead of ballooning this process.
    if std::io::stdin()
        .take(1024 * 1024)
        .read_to_string(&mut raw)
        .is_err()
    {
        return;
    }
    let Ok(input) = serde_json::from_str::<StopInput>(&raw) else {
        return;
    };
    // The two cheap payload checks come before any filesystem or config
    // work: a miswired event or a hook-caused continuation is a pure no-op
    // that should neither load the overlay nor advance the stop counter
    // (continuations are not real stops).
    if input
        .hook_event_name
        .as_deref()
        .is_some_and(|name| name != "Stop")
    {
        return;
    }
    if input.stop_hook_active {
        return;
    }
    if !valid_session_id(&input.session_id) {
        return;
    }
    let Ok(loaded) = crystalline_service::overlay::load(None) else {
        return;
    };
    let has_domains = !loaded.effective.domains.is_empty();
    let read_only = loaded.effective.read_only();

    let Ok(path) = state_path(&input.session_id) else {
        return;
    };
    let state = read_state(&path).unwrap_or_else(SessionState::fresh);
    let transcript = transcript_stats(input.transcript_path.as_deref());

    let decision = decide(&input, has_domains, read_only, &state, transcript);

    let new_state = SessionState {
        v: STATE_VERSION,
        stops: state.stops.saturating_add(1),
        nudged: state.nudged || decision == StopDecision::Nudge,
        updated_at: chrono::Utc::now(),
    };
    // State is persisted before the nudge is printed: a crash or a killed
    // process between the two can only cost a missed nudge, never a repeat
    // one. A write failure is itself silent, the same as every other bail -
    // never risking a second nudge is worth more than reporting the error.
    let _ = write_state(&path, &new_state);

    // The per-machine throttle record, read on every call that gets this far
    // rather than only on the ones that nudge: seeding `first_seen` from the
    // first Stop hook this machine ever ran is what starts the fresh-install
    // quiet week, and a call that stays silent is still evidence the machine
    // exists. Seeding it with `now` cannot arm the weekly arm below - an age
    // of zero is inside every interval.
    let now = chrono::Utc::now();
    let mut maintenance_state = maintenance::load();
    let mut maintenance_dirty = false;
    if maintenance_state.first_seen.is_none() {
        maintenance_state.first_seen = Some(now);
        maintenance_dirty = true;
    }
    // The ride-along: only a call that already earned the capture nudge ever
    // asks, so the maintenance throttle can never be the reason this handler
    // breaks its silence.
    let evolve = if decision == StopDecision::Nudge {
        evolve_ask(&maintenance_state, now)
    } else {
        None
    };
    if evolve.is_some() {
        maintenance_state.last_nudge_at = Some(now);
        maintenance_dirty = true;
    }
    // Persisted before the print, the same crash-safety order the session
    // state follows: a crash in between can only cost a missed ask, never a
    // repeated one. The write goes through the same serialized, atomic
    // writer the daemon uses, and a failure is silent like every other bail.
    if maintenance_dirty {
        let _ = maintenance::save(&maintenance_state);
    }

    sweep_stale_state();

    if decision == StopDecision::Nudge {
        let payload =
            serde_json::json!({ "decision": "block", "reason": nudge_reason(evolve.as_deref()) });
        if let Ok(line) = serde_json::to_string(&payload) {
            println!("{line}");
        }
    }
}

/// The state file path for a session, `<state_dir>/hooks/<session_id>.json`.
/// Only ever called with an id [`valid_session_id`] has already accepted.
fn state_path(session_id: &str) -> Result<PathBuf, config::ConfigError> {
    Ok(config::state_dir()?
        .join("hooks")
        .join(format!("{session_id}.json")))
}

/// Read and parse a session's state file. `None` for a missing file, an
/// unreadable one or one that fails to parse - every case [`run_stop`]
/// treats identically, falling back to [`SessionState::fresh`].
fn read_state(path: &Path) -> Option<SessionState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Serialize and atomically write a session's state file, creating
/// `<state_dir>/hooks/` if it does not exist yet.
fn write_state(path: &Path, state: &SessionState) -> Result<(), ()> {
    let bytes = serde_json::to_vec(state).map_err(|_| ())?;
    config::save_bytes(path, &bytes).map_err(|_| ())
}

/// Measure a transcript without ever reading more than [`SUBSTANCE_BYTES`]:
/// `stat` first, and only read when the file's size alone falls short of the
/// byte bar. A `None` path, or one this process cannot stat or read, reads as
/// [`TranscriptStats::Unavailable`].
fn transcript_stats(path: Option<&Path>) -> TranscriptStats {
    let Some(path) = path else {
        return TranscriptStats::Unavailable;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return TranscriptStats::Unavailable;
    };
    if meta.len() >= SUBSTANCE_BYTES {
        return TranscriptStats::Substantial;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return TranscriptStats::Unavailable;
    };
    // Sized to the stat result (already known to be under the byte bar) and
    // capped at the bar itself in case the file grew between stat and read.
    let mut buf = vec![0u8; (meta.len().min(SUBSTANCE_BYTES)) as usize];
    let Ok(n) = file.read(&mut buf) else {
        return TranscriptStats::Unavailable;
    };
    let lines = buf[..n].iter().filter(|&&b| b == b'\n').count();
    if lines >= SUBSTANCE_LINES {
        TranscriptStats::Substantial
    } else {
        TranscriptStats::Insufficient
    }
}

/// Opportunistically remove session state files older than
/// [`STATE_STALE_SECS`]. Nothing schedules this separately; it rides along
/// on every `hook stop` call, so a long-lived install never accumulates one
/// file per session forever. A missing hooks directory or any per-entry IO
/// error is silently skipped - this is housekeeping, never load-bearing for
/// the decision just made. `maintenance.json` is exempt: it is per-machine
/// and long-lived, so a quiet week must not erase it.
fn sweep_stale_state() {
    let Ok(dir) = config::state_dir().map(|d| d.join("hooks")) else {
        return;
    };
    sweep_dir(&dir);
}

/// [`sweep_stale_state`] against an explicit directory, so the sweep's rules
/// are testable without the real state directory or a process-global
/// environment override.
fn sweep_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let Some(cutoff) =
        std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(STATE_STALE_SECS))
    else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // The maintenance throttle record shares this folder but is
        // per-machine and long-lived: sweeping it after a quiet week would
        // erase the throttle and re-arm the fresh-install grace period.
        if path.file_name().and_then(|n| n.to_str()) == Some(MAINTENANCE_FILE) {
            continue;
        }
        if let Ok(meta) = entry.metadata()
            && let Ok(modified) = meta.modified()
            && modified < cutoff
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> StopInput {
        StopInput {
            session_id: "abc-123".to_string(),
            transcript_path: None,
            stop_hook_active: false,
            hook_event_name: None,
        }
    }

    // --- valid_session_id ----------------------------------------------------

    #[test]
    fn valid_session_id_accepts_and_rejects_the_expected_shapes() {
        let cases: &[(&str, bool)] = &[
            ("abc-123_DEF", true),
            ("a", true),
            (&"a".repeat(128), true),
            ("", false),
            (&"a".repeat(129), false),
            ("has a space", false),
            ("has/slash", false),
            ("../traversal", false),
            ("dot.dot", false),
            ("emoji-🙂", false),
        ];
        for (id, expected) in cases {
            assert_eq!(
                valid_session_id(id),
                *expected,
                "session id {id:?} expected valid={expected}"
            );
        }
    }

    // --- decide ----------------------------------------------------------------

    #[test]
    fn a_substantial_transcript_fires_on_the_first_call() {
        let decision = decide(
            &input(),
            true,
            false,
            &SessionState::fresh(),
            TranscriptStats::Substantial,
        );
        assert_eq!(decision, StopDecision::Nudge);
    }

    #[test]
    fn an_insufficient_transcript_stays_silent() {
        let decision = decide(
            &input(),
            true,
            false,
            &SessionState::fresh(),
            TranscriptStats::Insufficient,
        );
        assert_eq!(decision, StopDecision::Silent);
    }

    #[test]
    fn an_event_name_other_than_stop_is_silent() {
        let mut i = input();
        i.hook_event_name = Some("SessionStart".to_string());
        let decision = decide(
            &i,
            true,
            false,
            &SessionState::fresh(),
            TranscriptStats::Substantial,
        );
        assert_eq!(decision, StopDecision::Silent);
    }

    #[test]
    fn an_absent_event_name_does_not_block_the_nudge() {
        let decision = decide(
            &input(),
            true,
            false,
            &SessionState::fresh(),
            TranscriptStats::Substantial,
        );
        assert_eq!(decision, StopDecision::Nudge);
    }

    #[test]
    fn stop_hook_active_is_silent() {
        let mut i = input();
        i.stop_hook_active = true;
        let decision = decide(
            &i,
            true,
            false,
            &SessionState::fresh(),
            TranscriptStats::Substantial,
        );
        assert_eq!(decision, StopDecision::Silent);
    }

    #[test]
    fn zero_domains_is_silent() {
        let decision = decide(
            &input(),
            false,
            false,
            &SessionState::fresh(),
            TranscriptStats::Substantial,
        );
        assert_eq!(decision, StopDecision::Silent);
    }

    #[test]
    fn read_only_is_silent() {
        let decision = decide(
            &input(),
            true,
            true,
            &SessionState::fresh(),
            TranscriptStats::Substantial,
        );
        assert_eq!(decision, StopDecision::Silent);
    }

    #[test]
    fn an_already_nudged_session_stays_silent_regardless_of_the_transcript() {
        let mut state = SessionState::fresh();
        state.nudged = true;
        let decision = decide(&input(), true, false, &state, TranscriptStats::Substantial);
        assert_eq!(decision, StopDecision::Silent);
    }

    #[test]
    fn the_fallback_counter_waits_for_the_third_stop() {
        let mut state = SessionState::fresh();
        for expected_stops in [0u32, 1] {
            state.stops = expected_stops;
            let decision = decide(&input(), true, false, &state, TranscriptStats::Unavailable);
            assert_eq!(
                decision,
                StopDecision::Silent,
                "stops={expected_stops} should stay silent"
            );
        }
        state.stops = 2;
        let decision = decide(&input(), true, false, &state, TranscriptStats::Unavailable);
        assert_eq!(
            decision,
            StopDecision::Nudge,
            "the third stop (stops=2 going in) should fire"
        );
    }

    // --- evolve_ask ------------------------------------------------------------

    /// The fixed instant every throttle case below measures against, so no
    /// test depends on the wall clock.
    fn now() -> chrono::DateTime<chrono::Utc> {
        "2026-08-17T12:00:00Z".parse().unwrap()
    }

    fn days_ago(days: i64) -> chrono::DateTime<chrono::Utc> {
        now() - chrono::TimeDelta::days(days)
    }

    fn hours_ago(hours: i64) -> chrono::DateTime<chrono::Utc> {
        now() - chrono::TimeDelta::hours(hours)
    }

    #[test]
    fn evolve_ask_fires_on_the_weekly_arm() {
        let state = MaintenanceState {
            last_run_at: Some(days_ago(8)),
            first_seen: Some(days_ago(30)),
            ..MaintenanceState::default()
        };
        assert_eq!(
            evolve_ask(&state, now()),
            Some(Vec::new()),
            "a sweep older than a week is due, with no domains to focus on"
        );
    }

    #[test]
    fn evolve_ask_stays_quiet_inside_the_week() {
        let state = MaintenanceState {
            last_run_at: Some(days_ago(2)),
            first_seen: Some(days_ago(30)),
            ..MaintenanceState::default()
        };
        assert_eq!(evolve_ask(&state, now()), None);
    }

    #[test]
    fn a_fresh_install_gets_a_quiet_first_week() {
        let fresh = MaintenanceState {
            first_seen: Some(days_ago(3)),
            ..MaintenanceState::default()
        };
        assert_eq!(
            evolve_ask(&fresh, now()),
            None,
            "an install three days old owes nothing yet"
        );

        let settled = MaintenanceState {
            first_seen: Some(days_ago(8)),
            ..MaintenanceState::default()
        };
        assert_eq!(
            evolve_ask(&settled, now()),
            Some(Vec::new()),
            "once the first week is up, a machine that never swept is due"
        );

        // The very first hook call ever, before `first_seen` is seeded: the
        // clock has not started, so nothing is due.
        assert_eq!(evolve_ask(&MaintenanceState::default(), now()), None);
    }

    #[test]
    fn pending_domains_fire_after_the_cooldown() {
        let state = MaintenanceState {
            pending_domains: vec!["a".to_string()],
            pending_since: Some(hours_ago(30)),
            last_nudge_at: Some(hours_ago(25)),
            first_seen: Some(days_ago(3)),
            ..MaintenanceState::default()
        };
        assert_eq!(
            evolve_ask(&state, now()),
            Some(vec!["a".to_string()]),
            "a human write is due even inside the fresh-install quiet week"
        );
    }

    #[test]
    fn the_cooldown_silences_both_arms() {
        let state = MaintenanceState {
            pending_domains: vec!["a".to_string()],
            pending_since: Some(hours_ago(3)),
            last_run_at: Some(days_ago(9)),
            last_nudge_at: Some(hours_ago(2)),
            first_seen: Some(days_ago(30)),
            ..MaintenanceState::default()
        };
        assert_eq!(
            evolve_ask(&state, now()),
            None,
            "a nudge two hours ago silences the weekly and the pending arm alike"
        );
    }

    // --- nudge_reason ----------------------------------------------------------

    #[test]
    fn the_reason_names_the_focus_domains() {
        let focused = nudge_reason(Some(&["playground".to_string(), "ops".to_string()]));
        assert!(
            focused.starts_with(NUDGE_REASON),
            "the capture nudge stays first and whole: {focused}"
        );
        assert!(focused.contains(EVOLVE_NUDGE_REASON));
        assert!(
            focused.contains("Focus domains: playground, ops."),
            "the pending domains are named: {focused}"
        );

        let weekly_only = nudge_reason(Some(&[]));
        assert!(weekly_only.contains(EVOLVE_NUDGE_REASON));
        assert!(
            !weekly_only.contains("Focus domains"),
            "a weekly-arm ask has nothing to focus on: {weekly_only}"
        );

        assert_eq!(
            nudge_reason(None),
            NUDGE_REASON,
            "no ask means the capture nudge alone, byte for byte"
        );
    }

    // --- sweep -----------------------------------------------------------------

    #[test]
    fn the_stale_sweep_spares_the_maintenance_file() {
        let dir = tempfile::tempdir().unwrap();
        let stale = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(STATE_STALE_SECS + 3600))
            .unwrap();
        for name in ["old-session.json", MAINTENANCE_FILE] {
            let path = dir.path().join(name);
            std::fs::write(&path, b"{}").unwrap();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(stale).unwrap();
        }

        sweep_dir(dir.path());

        assert!(
            !dir.path().join("old-session.json").exists(),
            "a week-old session file is still swept"
        );
        assert!(
            dir.path().join(MAINTENANCE_FILE).exists(),
            "the maintenance throttle record is long-lived and must survive a quiet week"
        );
    }
}
