//! The bound on password guessing at `POST /auth/login`.
//!
//! A per-account escalating delay, never a refusal below the ceiling. Keyed on
//! the account name because that is the only key available: `PeerAddr` behind
//! a reverse proxy is the proxy, and this surface deliberately refuses to
//! trust `X-Forwarded-For` (a forwarding header may only take locality away,
//! never grant it). A delay rather than a lockout because a per-name refusal
//! hands an attacker a lever against a known account, which is the objection
//! `auth::authenticate`'s own doc comment raises and which still stands.
//!
//! **Keyed on the submitted name whether or not an account by that name
//! exists.** This is the load-bearing detail. `auth::authenticate` spends
//! exactly one argon2 verification per attempt so that a wrong name and a
//! wrong password cost the same; if only real accounts escalated here, a wrong
//! name would stay fast while a wrong password got slow, and that oracle would
//! be reopened one layer up.
//!
//! Modelled on the registration limiter in [`super::oauth`], including its
//! testability rule: `now` is a parameter rather than read inside, so the
//! whole schedule can be driven through its window without a test that sleeps.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, SemaphorePermit};

/// How long a name with no new failure keeps its count.
pub const FORGET_AFTER: Duration = Duration::from_secs(15 * 60);

/// How many names are tracked at once.
///
/// The keys are whatever callers submitted, so this is a bound on
/// attacker-supplied input rather than on the number of accounts. Past it the
/// oldest entry goes.
///
/// **The residual, accepted when this was designed:** an attacker can evict
/// their own counter by spraying this many other names between attempts. That
/// fails *open* - the throttle stops protecting a name, it never starts
/// denying one - which is the right direction for a front door, and the
/// traffic it costs is what the failed-login log event records. A fixed
/// bucket table with no eviction was considered and rejected because it fails
/// the other way: a low, constant request rate would hold every bucket at the
/// ceiling and make every legitimate sign-in on the instance pay the full
/// delay.
pub const MAX_TRACKED: usize = 100_000;

/// How many requests may be napping at once.
///
/// A napping request holds a task and an open connection, and the ceiling
/// above does not bound how many nap: an attacker cycling fresh names to four
/// or five failures each never reaches the refusal. Past this cap a request
/// gets the refusal immediately instead of a nap.
const MAX_SLEEPERS: usize = 64;

/// The first delay past the free attempts. Doubles from here.
const BASE_DELAY: Duration = Duration::from_secs(1);

/// What to do with an arriving attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Proceed, after napping this long. Often zero.
    Delay(Duration),
    /// Refuse now, and come back in this many seconds. Never zero.
    Refuse(u64),
}

/// One name's run of consecutive failures, and when the last of them arrived.
#[derive(Debug, Clone, Copy)]
struct Failures {
    count: u32,
    last: Instant,
}

/// The table of consecutive failures per submitted name.
#[derive(Debug)]
pub struct LoginThrottle {
    names: Mutex<HashMap<String, Failures>>,
    sleepers: Semaphore,
    free_attempts: u32,
    max_delay: Duration,
}

impl LoginThrottle {
    /// A throttle that lets `free_attempts` consecutive failures through
    /// undelayed and grows the delay to at most `max_delay`. A zero
    /// `max_delay` is the configured "off".
    pub fn new(free_attempts: u32, max_delay: Duration) -> LoginThrottle {
        LoginThrottle {
            names: Mutex::new(HashMap::new()),
            sleepers: Semaphore::new(MAX_SLEEPERS),
            free_attempts,
            max_delay,
        }
    }

    /// What an attempt for `key` arriving at `now` should cost.
    pub fn check(&self, key: &str, now: Instant) -> Verdict {
        // A zero ceiling is the configured "off". Answering before anything
        // else is read keeps that from turning into "refuse everything": the
        // first delay past the free attempts is one second, which would
        // otherwise exceed a zero ceiling and refuse.
        if self.max_delay.is_zero() {
            return Verdict::Delay(Duration::ZERO);
        }
        // A poisoned lock is taken anyway: refusing or delaying every login
        // for the life of the process would be a worse outcome than the panic
        // that poisoned it.
        let mut names = self.names.lock().unwrap_or_else(|e| e.into_inner());
        // A name nobody is tracking, and a name whose window has passed, both
        // carry a count of zero and go through the schedule like any other
        // rather than short-circuiting to no delay. With free attempts left
        // the two are the same answer; with `free_attempts` at zero, where
        // the setting means the very first attempt already pays, they are
        // not, and a short circuit here would skip the base second entirely.
        let seen = match names.get(key).copied() {
            Some(seen) if now.duration_since(seen.last) >= FORGET_AFTER => {
                names.remove(key);
                None
            }
            other => other,
        };
        let count = seen.map(|seen| seen.count).unwrap_or(0);
        match self.delay_for(count) {
            Some(delay) => Verdict::Delay(delay),
            None => {
                // Unreachable at a count of zero, since the ceiling is whole
                // seconds and a zero one answered above, but the wait is
                // computed without assuming that.
                let left = seen
                    .map(|seen| FORGET_AFTER.saturating_sub(now.duration_since(seen.last)))
                    .unwrap_or(FORGET_AFTER);
                Verdict::Refuse(left.as_secs().max(1))
            }
        }
    }

    /// The nap an attempt arriving on top of `count` consecutive failures has
    /// earned, or `None` past the ceiling, where the answer is a refusal
    /// instead.
    ///
    /// `count` is what is already on the table, so the attempt being weighed
    /// is the `count + 1`th: with three free attempts, a count of three is the
    /// fourth attempt and the first that pays.
    fn delay_for(&self, count: u32) -> Option<Duration> {
        if count < self.free_attempts {
            return Some(Duration::ZERO);
        }
        // Checked rather than bare. The early return above already means
        // `count >= self.free_attempts`, so the subtraction cannot underflow
        // today, but that argument lives a few lines from the arithmetic.
        let steps = count.checked_sub(self.free_attempts)?;
        // A count large enough to overflow the shift or the multiplication is
        // far past the ceiling anyway, and it must refuse rather than wrap
        // around into a short nap.
        let factor = 1u32.checked_shl(steps)?;
        let delay = BASE_DELAY.checked_mul(factor)?;
        (delay <= self.max_delay).then_some(delay)
    }

    /// Count one failure for `key`.
    pub fn record_failure(&self, key: &str, now: Instant) {
        let mut names = self.names.lock().unwrap_or_else(|e| e.into_inner());
        // The sweep runs only when the table is full, so the common path stays
        // constant time. A `retain` on every failed login would be a hundred
        // thousand comparisons per attempt.
        if names.len() >= MAX_TRACKED {
            names.retain(|_, seen| now.duration_since(seen.last) < FORGET_AFTER);
            if names.len() >= MAX_TRACKED
                && let Some(oldest) = names
                    .iter()
                    .min_by_key(|(_, seen)| seen.last)
                    .map(|(name, _)| name.clone())
            {
                names.remove(&oldest);
            }
        }
        let seen = names.entry(key.to_string()).or_insert(Failures {
            count: 0,
            last: now,
        });
        // A failure arriving after the window is the first of a new run, not
        // the next of an old one.
        if now.duration_since(seen.last) >= FORGET_AFTER {
            seen.count = 0;
        }
        seen.count = seen.count.saturating_add(1);
        seen.last = now;
    }

    /// Forget `key` entirely, which is what a successful sign-in does.
    pub fn record_success(&self, key: &str) {
        self.names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// A permit to nap, or `None` when too many already are.
    pub fn try_take_sleeper(&self) -> Option<SemaphorePermit<'_>> {
        self.sleepers.try_acquire().ok()
    }

    /// How many consecutive failures `key` currently carries. Zero when it is
    /// not tracked. Read by the failed-login log event, which reports it so an
    /// operator can tell a typo from a run.
    pub fn failures(&self, key: &str) -> u32 {
        self.names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .map(|seen| seen.count)
            .unwrap_or(0)
    }

    /// How many names are tracked. The cap above is the only thing that reads
    /// it outside a test, and it reads its own map, so this exists for the
    /// test that pins the bound.
    #[cfg(test)]
    pub fn tracked(&self) -> usize {
        self.names.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn throttle() -> LoginThrottle {
        LoginThrottle::new(3, Duration::from_secs(8))
    }

    /// **The schedule walks the free attempts, then doubles, then refuses.**
    #[test]
    fn the_delay_doubles_past_the_free_attempts_and_then_refuses() {
        let t = throttle();
        let now = Instant::now();
        for _ in 0..3 {
            assert_eq!(t.check("ada", now), Verdict::Delay(Duration::ZERO));
            t.record_failure("ada", now);
        }
        for expected in [1, 2, 4, 8] {
            assert_eq!(
                t.check("ada", now),
                Verdict::Delay(Duration::from_secs(expected)),
                "after the free attempts the delay doubles"
            );
            t.record_failure("ada", now);
        }
        assert!(
            matches!(t.check("ada", now), Verdict::Refuse(_)),
            "past the ceiling the answer is a refusal, not a longer nap"
        );
    }

    /// **Zero free attempts charges from the very first try**, which is what
    /// that setting means, and the base second is reachable rather than
    /// skipped: an untracked name is a count of zero, not a special case.
    #[test]
    fn a_zero_free_attempt_setting_charges_from_the_first_attempt() {
        let t = LoginThrottle::new(0, Duration::from_secs(8));
        let now = Instant::now();
        assert_eq!(t.check("ada", now), Verdict::Delay(BASE_DELAY));
        t.record_failure("ada", now);
        assert_eq!(t.check("ada", now), Verdict::Delay(Duration::from_secs(2)));
    }

    /// **A refusal never says zero.** A `Retry-After: 0` invites an immediate
    /// retry into the same refusal.
    #[test]
    fn a_refusal_never_says_come_back_in_zero_seconds() {
        let t = throttle();
        let now = Instant::now();
        for _ in 0..8 {
            t.record_failure("ada", now);
        }
        let Verdict::Refuse(seconds) = t.check("ada", now) else {
            panic!("the eighth failure refuses");
        };
        assert!(seconds >= 1);
    }

    /// **A success clears the slate**, so somebody who mistyped four times and
    /// then got it right is not still paying on their next sign-in.
    #[test]
    fn a_success_forgets_the_failures() {
        let t = throttle();
        let now = Instant::now();
        for _ in 0..5 {
            t.record_failure("ada", now);
        }
        t.record_success("ada");
        assert_eq!(t.check("ada", now), Verdict::Delay(Duration::ZERO));
    }

    /// **An entry nobody touched inside the window is forgotten.**
    #[test]
    fn an_old_entry_is_forgotten() {
        let t = throttle();
        let start = Instant::now();
        for _ in 0..6 {
            t.record_failure("ada", start);
        }
        assert!(matches!(t.check("ada", start), Verdict::Delay(d) if !d.is_zero()));
        assert_eq!(
            t.check("ada", start + FORGET_AFTER),
            Verdict::Delay(Duration::ZERO),
            "the window has passed, so the count is gone"
        );
    }

    /// **A zero ceiling is off**, which is what the config key's 0 means. The
    /// easy mistake is to let the first delay past the free attempts exceed a
    /// zero ceiling and turn "off" into "refuse everything".
    #[test]
    fn a_zero_ceiling_turns_the_throttle_off() {
        let t = LoginThrottle::new(3, Duration::ZERO);
        let now = Instant::now();
        for _ in 0..50 {
            assert_eq!(t.check("ada", now), Verdict::Delay(Duration::ZERO));
            t.record_failure("ada", now);
        }
    }

    /// **The table never grows past its cap**, because its keys are whatever
    /// a caller submitted. A handful of names past the cap rather than
    /// hundreds: every one of them sweeps a full table, so the overshoot is
    /// paid for in a debug build and five names prove the same bound.
    #[test]
    fn the_table_is_bounded_by_its_cap() {
        let t = throttle();
        let now = Instant::now();
        for i in 0..(MAX_TRACKED + 5) {
            t.record_failure(&format!("name-{i}"), now);
        }
        assert!(t.tracked() <= MAX_TRACKED, "saw {}", t.tracked());
    }
}
