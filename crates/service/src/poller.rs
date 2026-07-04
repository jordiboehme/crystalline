//! Pure scheduling and observable-state plumbing for the background origin
//! poller (`crate::daemon::run_origin_poller`), factored out of the engine
//! and the daemon so the due/not-due, interval and jitter decisions are unit
//! tested with no tokio runtime, no engine and no provider at all, the same
//! separation [`crate::origin`] gives the origin engine methods.
//!
//! Nothing here talks to GitHub, the filesystem or the engine.
//! `Engine::origin_poll_tick` composes these functions with `origin_update`
//! (never reimplementing the pull itself), and [`OriginPollerState`] is the
//! shared, network-free record `Engine::status_report`'s `origins` block
//! reads from.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

/// The background poll interval when neither a domain nor the global
/// `github.poll_secs` setting configures one.
const DEFAULT_POLL_SECS: u64 = 300;

/// The lowest accepted poll interval, regardless of configuration: protects
/// a misconfigured `poll_secs` (global or per-domain) from hammering GitHub.
const MIN_POLL_SECS: u64 = 60;

/// How long a "no GitHub token yet" debug line is suppressed for after it is
/// logged once, so a poller waiting on a `connect` never spams the log.
const NO_TOKEN_LOG_INTERVAL: Duration = Duration::from_secs(3600);

/// The maximum jitter fraction applied to a poll interval, plus or minus.
const JITTER_FRACTION: f64 = 0.10;

/// The effective poll interval for one domain: its own `poll_secs` override,
/// else the global `github.poll_secs` setting, else [`DEFAULT_POLL_SECS`],
/// floored at [`MIN_POLL_SECS`] no matter how the value was configured.
pub(crate) fn effective_interval_secs(
    domain_poll_secs: Option<u64>,
    github_poll_secs: Option<u64>,
) -> u64 {
    domain_poll_secs
        .or(github_poll_secs)
        .unwrap_or(DEFAULT_POLL_SECS)
        .max(MIN_POLL_SECS)
}

/// A deterministic fraction in `[-JITTER_FRACTION, JITTER_FRACTION]` for
/// `domain` at `tick`: the same inputs always draw the same fraction, but a
/// different `tick` (see [`OriginPollerState::next_tick`]) redraws it, so
/// repeated reschedules of the same domain do not all land on the same
/// offset. Hashing is unpredictable enough for spreading poll load across
/// many domains, which is all jitter is for here, so no random number
/// generator dependency is needed.
fn jitter_fraction(domain: &str, tick: u64) -> f64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    domain.hash(&mut hasher);
    tick.hash(&mut hasher);
    let bits = (hasher.finish() >> 40) as u32; // 24 bits of entropy.
    let normalized = bits as f64 / (1u32 << 24) as f64; // [0, 1).
    (normalized * 2.0 - 1.0) * JITTER_FRACTION
}

/// `base_secs` adjusted by up to [`JITTER_FRACTION`], deterministic for a
/// given `domain` and `tick` (see [`jitter_fraction`]), never below one
/// second.
pub(crate) fn jittered_interval(base_secs: u64, domain: &str, tick: u64) -> Duration {
    let fraction = jitter_fraction(domain, tick);
    let seconds = (base_secs as f64) * (1.0 + fraction);
    Duration::from_secs_f64(seconds.max(1.0))
}

/// Whether a domain scheduled for `next_due` is due at `now`. `None` (never
/// scheduled) is always due: a freshly enabled or freshly added domain polls
/// on the poller's very next tick rather than waiting out a full interval.
pub(crate) fn is_due(next_due: Option<Instant>, now: Instant) -> bool {
    match next_due {
        None => true,
        Some(due) => now >= due,
    }
}

/// One completed poll's outcome, kept only for `status_report`'s `origins`
/// block; never consulted by the poller's own due/not-due decision.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DomainPollOutcome {
    /// The origin had nothing new.
    UpToDate,
    /// The origin had new content: `applied` files were written or deleted
    /// and `conflicts` new conflicts were recorded (zero when the pull
    /// merged cleanly).
    Applied {
        /// How many files were written or deleted from upstream.
        applied: usize,
        /// How many new conflicts this poll recorded.
        conflicts: usize,
    },
    /// The poll attempt failed; `message` is the error's product-vocabulary
    /// text, verbatim.
    Error(String),
}

/// One domain's poll schedule and most recent result.
#[derive(Clone, Debug, Default)]
struct DomainPollState {
    /// The scheduling clock's next-due instant, used only for the poller's
    /// own is-due check.
    next_due: Option<Instant>,
    /// `next_due`'s wall-clock mirror, for `status_report`'s offline
    /// `origins` block (an `Instant` carries no epoch, so it cannot be
    /// serialized on its own).
    next_due_at: Option<DateTime<Utc>>,
    /// The most recently completed poll's outcome, `None` before the first
    /// completed poll.
    last_result: Option<DomainPollOutcome>,
}

/// The origin poller's observable state: every domain's schedule and most
/// recent result, plus the poller's one shared rate-limit pause. Read by
/// `Engine::status_report`'s offline `origins` block and written only by the
/// poller's own tick; every accessor is a plain mutex lock, never IO or a
/// network call, mirroring the `origin_locks` map already on `Engine`.
#[derive(Default)]
pub(crate) struct OriginPollerState {
    domains: Mutex<HashMap<String, DomainPollState>>,
    rate_limited_until: Mutex<Option<DateTime<Utc>>>,
    tick_counter: AtomicU64,
    no_token_log_at: Mutex<Option<Instant>>,
}

impl OriginPollerState {
    /// A fresh tick counter value, for redrawing a domain's jitter on every
    /// reschedule (see [`jitter_fraction`]).
    pub(crate) fn next_tick(&self) -> u64 {
        self.tick_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Whether `domain` is due at `now`: never scheduled before (always due)
    /// or its recorded `next_due` has passed.
    pub(crate) fn is_due(&self, domain: &str, now: Instant) -> bool {
        let due = self
            .domains
            .lock()
            .unwrap()
            .get(domain)
            .and_then(|d| d.next_due);
        is_due(due, now)
    }

    /// Records `domain`'s next-due instant, in both clocks together so they
    /// never drift apart.
    pub(crate) fn schedule(&self, domain: &str, next_due: Instant, next_due_at: DateTime<Utc>) {
        let mut domains = self.domains.lock().unwrap();
        let entry = domains.entry(domain.to_string()).or_default();
        entry.next_due = Some(next_due);
        entry.next_due_at = Some(next_due_at);
    }

    /// Records `domain`'s most recently completed poll outcome.
    pub(crate) fn record_result(&self, domain: &str, outcome: DomainPollOutcome) {
        let mut domains = self.domains.lock().unwrap();
        domains.entry(domain.to_string()).or_default().last_result = Some(outcome);
    }

    /// `domain`'s next-due wall-clock instant, for the offline status block.
    pub(crate) fn next_due_at(&self, domain: &str) -> Option<DateTime<Utc>> {
        self.domains
            .lock()
            .unwrap()
            .get(domain)
            .and_then(|d| d.next_due_at)
    }

    /// `domain`'s most recently completed poll outcome, for the offline
    /// status block.
    pub(crate) fn last_result(&self, domain: &str) -> Option<DomainPollOutcome> {
        self.domains
            .lock()
            .unwrap()
            .get(domain)
            .and_then(|d| d.last_result.clone())
    }

    /// Pauses every domain's polling until `until`: GitHub rate limits are
    /// per-token, so one domain hitting the limit means every domain is
    /// paused, not just the one that tripped it. `None` clears the pause.
    pub(crate) fn set_rate_limited_until(&self, until: Option<DateTime<Utc>>) {
        *self.rate_limited_until.lock().unwrap() = until;
    }

    /// The shared rate-limit pause deadline, if the poller is currently
    /// waiting one out.
    pub(crate) fn rate_limited_until(&self) -> Option<DateTime<Utc>> {
        *self.rate_limited_until.lock().unwrap()
    }

    /// Whether a "no GitHub token yet" debug line should be logged right
    /// now: the first time this is asked, and at most once every
    /// [`NO_TOKEN_LOG_INTERVAL`] afterward, so a poller left waiting for a
    /// `connect` never spams the log.
    pub(crate) fn should_log_no_token(&self, now: Instant) -> bool {
        let mut last = self.no_token_log_at.lock().unwrap();
        let should_log = match *last {
            Some(t) => now.checked_duration_since(t).unwrap_or_default() >= NO_TOKEN_LOG_INTERVAL,
            None => true,
        };
        if should_log {
            *last = Some(now);
        }
        should_log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- effective_interval_secs -------------------------------------------

    #[test]
    fn effective_interval_secs_prefers_the_domain_override() {
        assert_eq!(effective_interval_secs(Some(120), Some(600)), 120);
    }

    #[test]
    fn effective_interval_secs_falls_back_to_the_global_setting() {
        assert_eq!(effective_interval_secs(None, Some(600)), 600);
    }

    #[test]
    fn effective_interval_secs_defaults_to_300_when_nothing_is_configured() {
        assert_eq!(effective_interval_secs(None, None), 300);
    }

    #[test]
    fn effective_interval_secs_floors_a_too_small_domain_override() {
        assert_eq!(effective_interval_secs(Some(5), None), 60);
    }

    #[test]
    fn effective_interval_secs_floors_a_too_small_global_setting() {
        assert_eq!(effective_interval_secs(None, Some(1)), 60);
    }

    // --- jittered_interval ---------------------------------------------------

    #[test]
    fn jittered_interval_is_deterministic_for_the_same_inputs() {
        let a = jittered_interval(300, "brand", 7);
        let b = jittered_interval(300, "brand", 7);
        assert_eq!(a, b);
    }

    #[test]
    fn jittered_interval_stays_within_ten_percent_of_the_base() {
        for tick in 0..200u64 {
            let d = jittered_interval(300, "brand", tick);
            let secs = d.as_secs_f64();
            assert!((270.0..=330.0).contains(&secs), "tick {tick}: {secs}");
        }
    }

    #[test]
    fn jittered_interval_varies_with_the_tick_counter() {
        let samples: std::collections::HashSet<_> = (0..20u64)
            .map(|tick| jittered_interval(300, "brand", tick).as_millis())
            .collect();
        assert!(samples.len() > 1, "jitter never changed across 20 ticks");
    }

    #[test]
    fn jittered_interval_varies_with_the_domain_name() {
        let a = jittered_interval(300, "brand", 1);
        let b = jittered_interval(300, "engineering", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn jittered_interval_never_drops_the_floor_below_one_second() {
        let d = jittered_interval(1, "brand", 1);
        assert!(d.as_secs_f64() >= 1.0);
    }

    // --- is_due ---------------------------------------------------------------

    #[test]
    fn is_due_when_never_scheduled() {
        assert!(is_due(None, Instant::now()));
    }

    #[test]
    fn is_due_when_the_deadline_has_passed() {
        let now = Instant::now();
        let due = now - Duration::from_secs(1);
        assert!(is_due(Some(due), now));
    }

    #[test]
    fn is_not_due_before_the_deadline() {
        let now = Instant::now();
        let due = now + Duration::from_secs(60);
        assert!(!is_due(Some(due), now));
    }

    // --- OriginPollerState ------------------------------------------------

    #[test]
    fn state_reports_a_domain_never_scheduled_as_due() {
        let state = OriginPollerState::default();
        assert!(state.is_due("brand", Instant::now()));
    }

    #[test]
    fn state_schedule_round_trips_next_due_at_and_gates_is_due() {
        let state = OriginPollerState::default();
        let now = Instant::now();
        let wall = Utc::now();
        state.schedule(
            "brand",
            now + Duration::from_secs(60),
            wall + chrono::Duration::seconds(60),
        );
        assert!(!state.is_due("brand", now));
        assert!(state.is_due("brand", now + Duration::from_secs(61)));
        assert_eq!(
            state.next_due_at("brand"),
            Some(wall + chrono::Duration::seconds(60))
        );
    }

    #[test]
    fn state_record_result_round_trips() {
        let state = OriginPollerState::default();
        assert_eq!(state.last_result("brand"), None);
        state.record_result("brand", DomainPollOutcome::UpToDate);
        assert_eq!(
            state.last_result("brand"),
            Some(DomainPollOutcome::UpToDate)
        );
    }

    #[test]
    fn state_rate_limit_pause_round_trips_and_clears() {
        let state = OriginPollerState::default();
        assert_eq!(state.rate_limited_until(), None);
        let until = Utc::now() + chrono::Duration::minutes(5);
        state.set_rate_limited_until(Some(until));
        assert_eq!(state.rate_limited_until(), Some(until));
        state.set_rate_limited_until(None);
        assert_eq!(state.rate_limited_until(), None);
    }

    #[test]
    fn tick_counter_increments_on_every_call() {
        let state = OriginPollerState::default();
        let a = state.next_tick();
        let b = state.next_tick();
        assert_ne!(a, b);
    }

    #[test]
    fn should_log_no_token_is_true_the_first_time_then_suppressed() {
        let state = OriginPollerState::default();
        let now = Instant::now();
        assert!(state.should_log_no_token(now));
        assert!(!state.should_log_no_token(now + Duration::from_secs(10)));
        assert!(!state.should_log_no_token(now + Duration::from_secs(3599)));
    }

    #[test]
    fn should_log_no_token_allows_another_line_after_an_hour() {
        let state = OriginPollerState::default();
        let now = Instant::now();
        assert!(state.should_log_no_token(now));
        assert!(state.should_log_no_token(now + Duration::from_secs(3601)));
    }
}
