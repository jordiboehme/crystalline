//! When a home instance's Reflection is due for its backup commit: at once
//! for a settled change, after 60 idle seconds for a room's edits. Pure
//! scheduling; the commit itself is `crystalline_remote::reflections_backup`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a room's edits must rest before they are one settled change.
pub const SETTLE_IDLE: Duration = Duration::from_secs(60);

/// How long a failed backup waits before the next attempt.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

/// The longest wait between two attempts: each failure in a row doubles
/// [`RETRY_AFTER`] up to this, so a lasting refusal does not ask the forge
/// every minute.
pub const RETRY_MAX: Duration = Duration::from_secs(3600);

/// How long a backup that held a page back waits before it looks again
/// with no new change: who may read a domain can change without anything in
/// the Reflection being marked.
pub const HELD_RECHECK: Duration = Duration::from_secs(600);

/// How often a backup that waits for a GitHub connection says so in the log.
const QUIET_LOG_INTERVAL: Duration = Duration::from_secs(3600);

/// Why a Reflection is marked for backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupCause {
    /// A room saved, or the sync saw a change: settles after [`SETTLE_IDLE`].
    Edit,
    /// An agent write, a comment, a settle, a move, a delete, a room closed.
    Settled,
}

/// One domain's pending backup.
#[derive(Debug, Clone, Copy)]
pub struct BackupMark {
    /// When the domain was last marked.
    pub last_edit: Instant,
    /// Whether a settled change is among the marked ones.
    pub settled: bool,
    /// After a failed attempt, the earliest moment of the next one.
    pub retry_after: Option<Instant>,
}

/// Whether a mark is due at `now`.
pub fn due(mark: &BackupMark, now: Instant) -> bool {
    if mark.retry_after.is_some_and(|at| now < at) {
        return false;
    }
    mark.settled || now.saturating_duration_since(mark.last_edit) >= SETTLE_IDLE
}

/// Every domain's pending backup.
#[derive(Debug, Default)]
pub struct BackupMarks {
    marks: Mutex<HashMap<String, BackupMark>>,
    /// Failed attempts in a row, per domain; gone once one lands.
    failures: Mutex<HashMap<String, u32>>,
    /// When the wait for a GitHub connection was last logged.
    quiet_logged: Mutex<Option<Instant>>,
    /// The domains whose last backup held a page back and that wait
    /// [`HELD_RECHECK`] to look again; a new mark ends the wait.
    holding: Mutex<std::collections::HashSet<String>>,
}

impl BackupMarks {
    /// Mark `domain` for backup.
    pub fn mark(&self, domain: &str, cause: BackupCause, now: Instant) {
        let mut marks = self.marks.lock().expect("backup marks");
        let mark = marks.entry(domain.to_string()).or_insert(BackupMark {
            last_edit: now,
            settled: false,
            retry_after: None,
        });
        mark.last_edit = now;
        mark.settled |= cause == BackupCause::Settled;
        // A change after a hold is backed up as any other: the wait to look
        // again was only for when nothing changes.
        if self.holding.lock().expect("backup holds").remove(domain) {
            mark.retry_after = None;
        }
    }

    /// The backup of `domain` landed at `now` and held a page back: keep it
    /// marked and look again after [`HELD_RECHECK`], or at once on a new
    /// mark, so the page goes out once nothing holds it any more. A change
    /// marked while the backup ran keeps its own mark and goes at once.
    pub fn hold(&self, domain: &str, now: Instant) {
        let mut marks = self.marks.lock().expect("backup marks");
        if marks.contains_key(domain) {
            return;
        }
        marks.insert(
            domain.to_string(),
            BackupMark {
                last_edit: now,
                settled: true,
                retry_after: Some(now + HELD_RECHECK),
            },
        );
        drop(marks);
        self.holding
            .lock()
            .expect("backup holds")
            .insert(domain.to_string());
    }

    /// The domains due at `now`, sorted.
    pub fn due_domains(&self, now: Instant) -> Vec<String> {
        let marks = self.marks.lock().expect("backup marks");
        let mut out: Vec<String> = marks
            .iter()
            .filter(|(_, m)| due(m, now))
            .map(|(d, _)| d.clone())
            .collect();
        out.sort();
        out
    }

    /// Forget `domain`'s mark: it is not a home Reflection here.
    pub fn clear(&self, domain: &str) {
        self.holding.lock().expect("backup holds").remove(domain);
        self.marks.lock().expect("backup marks").remove(domain);
        self.failures
            .lock()
            .expect("backup failures")
            .remove(domain);
    }

    /// The backup of `domain` that the tick started at `seen` landed (or had
    /// nothing to do): forget the mark, unless a change was marked after the
    /// tick read it - that one is not in the commit and stays marked.
    pub fn clear_if_unchanged(&self, domain: &str, seen: Instant) {
        self.holding.lock().expect("backup holds").remove(domain);
        self.failures
            .lock()
            .expect("backup failures")
            .remove(domain);
        let mut marks = self.marks.lock().expect("backup marks");
        match marks.get_mut(domain) {
            Some(m) if m.last_edit <= seen => {
                marks.remove(domain);
            }
            // A change marked during the backup: the next tick takes it,
            // with no wait left over from earlier failures.
            Some(m) => m.retry_after = None,
            None => {}
        }
    }

    /// The backup of `domain` failed at `now`: keep the mark and wait,
    /// [`RETRY_AFTER`] after the first failure and twice as long after each
    /// further one in a row, up to [`RETRY_MAX`]. Answers how many attempts
    /// in a row have failed, so the caller logs the first one loudly.
    pub fn failed(&self, domain: &str, now: Instant) -> u32 {
        let mut failures = self.failures.lock().expect("backup failures");
        let count = failures.entry(domain.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        let wait = RETRY_AFTER
            .saturating_mul(1u32 << (*count - 1).min(16))
            .min(RETRY_MAX);
        if let Some(mark) = self.marks.lock().expect("backup marks").get_mut(domain) {
            mark.retry_after = Some(now + wait);
        }
        *count
    }

    /// The domain `old` is now called `new`: its pending mark goes along,
    /// merged with any mark `new` already carries.
    pub fn rename(&self, old: &str, new: &str) {
        self.failures.lock().expect("backup failures").remove(old);
        self.holding.lock().expect("backup holds").remove(old);
        let mut marks = self.marks.lock().expect("backup marks");
        let Some(moved) = marks.remove(old) else {
            return;
        };
        let merged = match marks.get(new) {
            Some(there) => BackupMark {
                last_edit: there.last_edit.max(moved.last_edit),
                settled: there.settled || moved.settled,
                retry_after: None,
            },
            None => BackupMark {
                retry_after: None,
                ..moved
            },
        };
        marks.insert(new.to_string(), merged);
    }

    /// Whether the backup that waits for a GitHub connection should say so
    /// at `now`: the first time, then at most once an hour.
    pub fn should_log_waiting(&self, now: Instant) -> bool {
        let mut last = self.quiet_logged.lock().expect("backup log");
        let due = last.is_none_or(|at| now.saturating_duration_since(at) >= QUIET_LOG_INTERVAL);
        if due {
            *last = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settled_mark_is_due_at_once_and_an_edit_after_sixty_idle_seconds() {
        let t0 = Instant::now();
        let edit = BackupMark {
            last_edit: t0,
            settled: false,
            retry_after: None,
        };
        assert!(!due(&edit, t0 + Duration::from_secs(59)));
        assert!(due(&edit, t0 + Duration::from_secs(60)));
        let settled = BackupMark {
            settled: true,
            ..edit
        };
        assert!(due(&settled, t0));
        let waiting = BackupMark {
            retry_after: Some(t0 + Duration::from_secs(60)),
            ..settled
        };
        assert!(
            !due(&waiting, t0 + Duration::from_secs(10)),
            "a failed attempt waits"
        );
        assert!(due(&waiting, t0 + Duration::from_secs(61)));
    }

    #[test]
    fn a_new_edit_restarts_the_idle_clock_and_a_settle_wins() {
        let marks = BackupMarks::default();
        let t0 = Instant::now();
        marks.mark("eng", BackupCause::Edit, t0);
        marks.mark("eng", BackupCause::Edit, t0 + Duration::from_secs(50));
        assert!(
            marks.due_domains(t0 + Duration::from_secs(70)).is_empty(),
            "idle only since 50s"
        );
        assert_eq!(
            marks.due_domains(t0 + Duration::from_secs(111)),
            vec!["eng".to_string()]
        );
        marks.mark("ops", BackupCause::Settled, t0);
        assert!(marks.due_domains(t0).contains(&"ops".to_string()));
        // A settle after an edit wins: the domain is due at once.
        let t1 = t0 + Duration::from_secs(200);
        marks.mark("eng", BackupCause::Edit, t1);
        assert!(
            !marks.due_domains(t1).contains(&"eng".to_string()),
            "an edit alone waits"
        );
        marks.mark("eng", BackupCause::Settled, t1);
        assert!(
            marks.due_domains(t1).contains(&"eng".to_string()),
            "the settle wins"
        );
    }

    #[test]
    fn failures_in_a_row_wait_longer_and_a_landed_backup_starts_over() {
        let marks = BackupMarks::default();
        let t0 = Instant::now();
        marks.mark("eng", BackupCause::Settled, t0);
        assert_eq!(marks.failed("eng", t0), 1);
        assert!(
            marks
                .due_domains(t0 + Duration::from_secs(61))
                .contains(&"eng".to_string())
        );
        assert_eq!(marks.failed("eng", t0), 2);
        assert!(
            marks.due_domains(t0 + Duration::from_secs(61)).is_empty(),
            "two minutes now"
        );
        assert!(
            marks
                .due_domains(t0 + Duration::from_secs(121))
                .contains(&"eng".to_string())
        );
        for _ in 0..20 {
            marks.failed("eng", t0);
        }
        assert!(
            marks
                .due_domains(t0 + RETRY_MAX - Duration::from_secs(1))
                .is_empty()
        );
        assert!(
            marks
                .due_domains(t0 + RETRY_MAX)
                .contains(&"eng".to_string()),
            "never past the cap"
        );
        marks.clear_if_unchanged("eng", t0);
        marks.mark("eng", BackupCause::Settled, t0);
        assert_eq!(
            marks.failed("eng", t0),
            1,
            "a landed backup resets the count"
        );
    }

    #[test]
    fn a_rename_carries_the_mark_to_the_new_name() {
        let marks = BackupMarks::default();
        let t0 = Instant::now();
        marks.mark("eng", BackupCause::Settled, t0);
        marks.rename("eng", "platform");
        assert_eq!(marks.due_domains(t0), vec!["platform".to_string()]);
        marks.rename("gone", "other");
        assert_eq!(marks.due_domains(t0), vec!["platform".to_string()]);
    }
}
