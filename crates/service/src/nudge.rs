//! The two asks a write receipt carries: share what the team has not seen yet,
//! and run the maintenance sweep when it is due.
//!
//! The Stop hook already gives a person both of these at the end of a session
//! (`crates/cli/src/hook.rs`). An agent working over MCP never sees a Stop
//! hook - it may be talking to this daemon from another machine, another
//! harness or no harness at all - so the same two asks ride along on the one
//! surface every agent does meet: the receipt it gets back from a write.
//!
//! Three rules shape what is emitted, and all three exist so a trailer never
//! becomes noise an agent learns to skip:
//!
//! - **at most one line per receipt.** Share first, because unshared work is
//!   somebody else's problem until it is shared; the maintenance ask gets its
//!   chance only on a receipt the share ask did not take;
//! - **once per cooldown per identity** ([`crate::maintenance::mcp_nudge_due`]),
//!   so an agent capturing twenty engrams in a row meets each ask once, and two
//!   agents sharing one daemon each meet it on their own clock;
//! - **recorded when emitted, never when merely computed.** A receipt that says
//!   nothing spends nothing, so the ask still lands on the first write after
//!   there is something to say.
//!
//! The share wording is the hook's own, byte for byte
//! ([`MCP_SHARE_NUDGE_REASON`]): `crates/service` cannot depend on
//! `crates/cli`, so the sentence is copied rather than imported, and a test in
//! the CLI's own suite pins the copy against the original.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use crystalline_core::config::GlobalConfig;

use crate::engine::Engine;
use crate::maintenance::{self, MaintenanceState, NudgeKind, mcp_nudge_due};

/// What every line this module emits starts with, so an agent can tell the
/// knowledge base's own voice from the receipt it is attached to.
///
/// [`MCP_EVOLVE_NUDGE`] spells the same five characters out rather than
/// composing them, because the brief pins that constant as one literal. The two
/// therefore move together or not at all: a prefix changed here alone would
/// leave the maintenance ask on the old one, and the test that counts asks
/// counts this prefix.
pub const MCP_NUDGE_PREFIX: &str = "[crystalline] ";

/// The tail of the sharing ask, a byte-for-byte copy of the Stop hook's
/// `SHARE_NUDGE_REASON` (`crates/cli/src/hook.rs`).
///
/// Copied rather than imported because the dependency direction forbids the
/// import: `crates/service` sits below `crates/cli`, and the hook is the CLI's.
/// The two are pinned equal by `the_mcp_share_nudge_mirrors_the_hook_byte_for_byte`
/// in the CLI's own test suite, so a reword on either side fails there rather
/// than drifting into two dialects of one ask.
///
/// Number-neutral, like the original: [`share_nudge_line`] agrees with itself
/// in number and then hands off to this sentence, which refers back to the
/// whole delta as "that work".
pub const MCP_SHARE_NUDGE_REASON: &str = "If that work is done, propose sharing it with share_changes so the domain owner can review it and the team's archive stays current - and wait for a yes.";

/// The maintenance ask, whole. One line rather than the hook's paragraph: a
/// receipt is read mid-task, so it names the tool, the two authority classes
/// and nothing else.
///
/// It opens with [`MCP_NUDGE_PREFIX`]'s five characters, written out rather
/// than composed because the brief pins this literal. Change one and change the
/// other.
pub const MCP_EVOLVE_NUDGE: &str = "[crystalline] Knowledge maintenance is due: call evolve_engrams and work the queue - mechanical findings directly, judgment findings one at a time with a yes.";

/// How long a machine may go without a consolidation sweep before the
/// maintenance ask arms itself: one week, measured from the last recorded
/// sweep or, when none was ever recorded, from the day the Stop hook first saw
/// this machine.
///
/// The Stop hook's `EVOLVE_RUN_INTERVAL_DAYS` under another name, for the
/// reason [`MCP_SHARE_NUDGE_REASON`] is copied: the constant is the CLI's and
/// the dependency direction forbids the import. Both arms below are the arms
/// that hook reads, so the two surfaces ask on the same evidence.
const EVOLVE_RUN_INTERVAL_DAYS: i64 = 7;

/// What stands in for the identity when a call carries no account: a stdio
/// session, or an HTTP instance that asks nobody to authenticate.
///
/// One key per machine rather than one per session, so a harness that opens a
/// fresh session per task does not meet the same ask on every one of them.
///
/// Reserved, therefore: an account literally named `instance` shares this
/// cadence, which costs it at worst a silent receipt somebody else's session
/// earned. Nothing here is authorization, so that is the whole of the
/// consequence.
pub const INSTANCE_KEY: &str = "instance";

/// Whose cadence a call runs on: the account it authenticated as, or the
/// instance when it has none.
pub(crate) fn identity_key(identity: Option<&str>) -> &str {
    identity.unwrap_or(INSTANCE_KEY)
}

/// The sharing ask for `count` unshared changes across `domains`.
///
/// One domain is named, because naming it is what lets an agent act without
/// asking which; several are counted, because a list of names would push the
/// instruction out of sight. The counts agree with themselves in number, so a
/// single change never reads as "1 changes". Both rules are the Stop hook's
/// `share_line`, which is the line this one has to read like.
///
/// Callers hand it a delta they have already found to be non-empty; it renders
/// what it is given rather than deciding whether there is anything to say.
pub fn share_nudge_line(count: u64, domains: &[String]) -> String {
    let what = if count == 1 {
        "1 change".to_string()
    } else {
        format!("{count} changes")
    };
    let verb = if count == 1 { "is" } else { "are" };
    let where_ = if domains.len() == 1 {
        format!("in the team domain {}", domains[0])
    } else {
        format!("across {} team domains", domains.len())
    };
    format!("{MCP_NUDGE_PREFIX}{what} {where_} {verb} not yet shared. {MCP_SHARE_NUDGE_REASON}")
}

/// How long one team domain's walked answer is trusted before the walk is paid
/// again: a minute.
///
/// Short deliberately, and much shorter than the ask's own cooldown. The
/// unshared work this arm looks for is CREATED by the very writes that carry
/// these receipts, so a memo as long as the cooldown would hide an agent's own
/// work from it for hours. A minute bounds the cost - twenty captures in a row
/// pay for one walk rather than twenty - while keeping the ask within a minute
/// of the work that earned it.
const SHARE_MEMO_TTL: Duration = Duration::from_secs(60);

/// What the share arm last found for one team domain's folder, and when.
///
/// Keyed by the folder rather than the domain name: the name is per-install
/// configuration and two engines in one process (which is what the test suite
/// is) can register the same name for different folders, while a folder is the
/// thing actually walked.
///
/// Process-local and deliberately not persisted: it is a cost bound, not a
/// decision record. Losing it on restart costs one walk.
static SHARE_MEMO: LazyLock<Mutex<HashMap<String, (Instant, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Take [`SHARE_MEMO`], ignoring poisoning: a panicking reader leaves a map
/// that is still a map, and a cost bound is no reason to bring a daemon down.
fn memo() -> std::sync::MutexGuard<'static, HashMap<String, (Instant, u64)>> {
    SHARE_MEMO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How many times the share arm has gone past the memo to an actual walk, for
/// the test that pins the memo. A test seam: neither the counter nor its
/// increment is compiled into a released binary.
#[cfg(any(test, feature = "testing"))]
static SHARE_WALKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many walks this process has paid for since it started.
#[cfg(any(test, feature = "testing"))]
pub fn share_walks() -> u64 {
    SHARE_WALKS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Count one paid walk. A no-op in a released binary.
#[cfg(any(test, feature = "testing"))]
fn count_walk() {
    SHARE_WALKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Count one paid walk. A no-op in a released binary.
#[cfg(not(any(test, feature = "testing")))]
fn count_walk() {}

/// The one line a write receipt carries, or `None` when nothing is due.
///
/// `identity` is the account the call authenticated as, `None` for a stdio
/// session and for an instance with MCP authentication off. It answers two
/// questions at once, and they are not the same question:
///
/// - **whose cadence this is** (`identity_key`), where `None` keys on the
///   instance;
/// - **whose drafts to count** in a domain that reviews changes before they
///   land, where `None` means the machine owner
///   ([`crate::engine::OWNER_IDENTITY_NAME`]), which is what
///   [`crate::scope::overlay_actor`] answers for the local surfaces. The one
///   scope where the two disagree is an unauthenticated HTTP caller, and that
///   caller is refused by a reviewing domain's write gate long before it can
///   reach a receipt.
///
/// Cost is bounded on purpose, because this runs on every successful write:
/// both throttles are read FIRST out of one small JSON file and a receipt with
/// neither ask open leaves without reading anything else at all - not even the
/// configuration, which is a whole-snapshot clone. Past that, a domain with no
/// origin is never looked at, a reviewing domain is answered from index rows
/// rather than by staging a share (which pulls, over the network, and has no
/// business inside a write receipt), and the one walk that is left is memoized
/// per folder for a minute (`SHARE_MEMO_TTL`) and run off the runtime's threads.
///
/// **The read and the record are not one atomic step**, and deliberately not:
/// two writes from one identity arriving at once both read "due" and both emit,
/// so a cooldown window can carry two trailers. An agent serializes its own
/// tool calls, so the case is rare; the cost when it happens is one extra
/// sentence; and the alternative is a lock on the path of every write, which is
/// a real cost paid against a cosmetic one.
pub async fn write_verb_trailer(engine: &Engine, identity: Option<&str>) -> Option<String> {
    let state_dir = engine.journal_state_dir().ok()?;
    let key = identity_key(identity);
    let now = Utc::now();
    let state = maintenance::load_under(&state_dir);

    let share_open = mcp_nudge_due(&state, NudgeKind::Share, key, now);
    let evolve_open = mcp_nudge_due(&state, NudgeKind::Evolve, key, now);
    if !share_open && !evolve_open {
        return None;
    }
    // One snapshot for whichever arms are still open, rather than one per arm:
    // `Engine::config` clones the whole configuration.
    let config = engine.config();

    if share_open {
        let (count, domains) = unshared_team_work(engine, &config, identity).await;
        if count > 0 && !domains.is_empty() {
            record(&state_dir, NudgeKind::Share, key, now);
            return Some(share_nudge_line(count, &domains));
        }
    }

    if evolve_open && evolve_due(&config, &state, now) {
        record(&state_dir, NudgeKind::Evolve, key, now);
        return Some(MCP_EVOLVE_NUDGE.to_string());
    }

    None
}

/// Stamp an emitted nudge against the state directory this daemon reads, which
/// is the machine's own in production and a test's temporary one under test.
fn record(state_dir: &std::path::Path, kind: NudgeKind, identity: &str, now: DateTime<Utc>) {
    if let Err(e) =
        maintenance::record_mcp_nudge_at(&maintenance::path_under(state_dir), kind, identity, now)
    {
        // Swallowed at debug like every other writer of this file: the write
        // that earned the receipt has already landed, and a throttle record
        // that could not be stamped costs one repeated ask.
        tracing::debug!("maintenance state not stamped with the mcp nudge: {e}");
    }
}

/// What every team domain holds that the team has not seen, as
/// `(changes, the domains holding them)` in registration order.
///
/// Two dimensions, one per mode, because "not yet shared" means two different
/// things:
///
/// - a domain that takes changes directly owes its origin whatever its working
///   tree holds that the base snapshot does not, which is what `origin status`
///   counts offline and what the Stop hook asks about;
/// - a domain that reviews changes first owes nothing from its folder - every
///   legitimate change is in somebody's draft overlay - so what is unshared is
///   the ACTING identity's own draft delta, and nobody else's.
///
/// A domain with no origin is skipped before anything is read: there is
/// nowhere for its work to be shared to, and skipping it is also what keeps an
/// install with no team domains from paying for this at all. A domain whose
/// origin is mid-operation is skipped too, without waiting for it: an ask is
/// worth no part of the latency of a write that has already landed.
async fn unshared_team_work(
    engine: &Engine,
    config: &GlobalConfig,
    identity: Option<&str>,
) -> (u64, Vec<String>) {
    let actor = identity.unwrap_or(crate::engine::OWNER_IDENTITY_NAME);
    let mut changes = 0u64;
    let mut names = Vec::new();
    for (name, entry) in &config.domains {
        if entry.origin.is_none() {
            continue;
        }
        let count = if entry.is_overlay() {
            // An index read, per actor, so there is nothing here to memoize:
            // the answer is different for every caller and cheap for all of
            // them.
            engine
                .overlay_counts_by_actor(name)
                .await
                .unwrap_or_default()
                .iter()
                .find(|(who, _)| who == actor)
                .map(|(_, held)| *held)
                .unwrap_or(0)
        } else {
            match entry.file_path() {
                Some(root) => walked_unshared(engine, name, &root).await,
                None => 0,
            }
        };
        if count == 0 {
            continue;
        }
        changes += count;
        names.push(name.clone());
    }
    (changes, names)
}

/// One direct-mode team domain's unshared count, from the memo when it holds a
/// fresh answer and from the walk otherwise.
///
/// Only an answer is memoized. A walk that could not run - an origin operation
/// in flight, an unreadable tree - leaves the memo as it was, so a contended
/// moment is retried on the next receipt rather than cached as "owes nothing"
/// for a minute.
async fn walked_unshared(engine: &Engine, name: &str, root: &Path) -> u64 {
    let key = root.display().to_string();
    if let Some((at, count)) = memo().get(&key)
        && at.elapsed() < SHARE_MEMO_TTL
    {
        return *count;
    }
    count_walk();
    match engine.unshared_change_count(name).await {
        Some(count) => {
            memo().insert(key, (Instant::now(), count));
            count
        }
        None => 0,
    }
}

/// Whether the maintenance sweep is due, on the two arms the Stop hook reads:
/// a week since the last recorded sweep, or any domain a human has written to
/// since one ran.
///
/// The pending arm is narrowed to domains this install still registers, for
/// the reason the hook narrows it: a domain that went pending and was then
/// unregistered can never be swept out of the list again, and a ghost must not
/// arm an ask nobody can settle.
///
/// The weekly arm needs a clock to measure against, and on an install that has
/// never run a Stop hook there is none - `first_seen` is the hook's own stamp.
/// Such an install is asked by the pending arm alone, which is the arm with
/// evidence behind it.
fn evolve_due(config: &GlobalConfig, state: &MaintenanceState, now: DateTime<Utc>) -> bool {
    let pending = state
        .pending_domains
        .iter()
        .any(|domain| config.domains.contains_key(domain));
    let interval = TimeDelta::days(EVOLVE_RUN_INTERVAL_DAYS);
    let weekly = match state.last_run_at {
        Some(ran) => now.signed_duration_since(ran) >= interval,
        None => state
            .first_seen
            .is_some_and(|seen| now.signed_duration_since(seen) >= interval),
    };
    weekly || pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maintenance::{self, NudgeKind, mcp_nudge_due};

    /// A session with no account of its own - stdio, or an instance that asks
    /// nobody to authenticate - is still one cadence: it keys on the instance,
    /// and a named account's quiet hours are not spent by it.
    #[test]
    fn an_anonymous_nudge_uses_the_instance_key() {
        let now: DateTime<Utc> = "2026-09-16T09:00:00Z".parse().unwrap();
        assert_eq!(identity_key(None), INSTANCE_KEY);
        assert_eq!(identity_key(Some("ada")), "ada");

        let dir = tempfile::tempdir().unwrap();
        let path = maintenance::path_under(dir.path());
        maintenance::record_mcp_nudge_at(&path, NudgeKind::Evolve, identity_key(None), now)
            .unwrap();

        let state = maintenance::load_under(dir.path());
        assert!(
            !mcp_nudge_due(&state, NudgeKind::Evolve, identity_key(None), now),
            "the instance's own cadence is throttled"
        );
        assert!(
            mcp_nudge_due(&state, NudgeKind::Evolve, "ada", now),
            "and an account keeps its own"
        );
    }

    /// **The share arm's tree walk never runs on a runtime thread, and never
    /// waits for the origin lock.**
    ///
    /// Both halves are structural rather than observable: a walk that hashes
    /// every file in a domain root would occupy a tokio worker for as long as
    /// it takes, and the deterministic way to show contention is to hold the
    /// lock from a fixture, which pins the implementation rather than the
    /// behaviour. So the shape itself is pinned, on this repo's own precedent
    /// for a source-scanning guard, and the caller's `.await` is pinned by the
    /// compiler.
    #[test]
    fn the_share_walk_is_handed_to_a_blocking_thread() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("engine.rs"),
        )
        .expect("the engine's own source");
        let body = source
            .split_once("pub(crate) async fn unshared_change_count")
            .expect("the receipt path's counter")
            .1
            .split_once("\n    }\n")
            .expect("its body ends at the method's closing brace")
            .0;
        assert!(
            body.contains("spawn_blocking"),
            "the walk is handed to a blocking thread, never run on the runtime's: {body}"
        );
        assert!(
            body.contains("try_lock_owned"),
            "and the origin lock is taken without waiting, in a guard the \
             blocking task can own: {body}"
        );
    }

    /// The line agrees with itself in number, names one domain and counts
    /// several, and carries the hook's sentence unchanged.
    #[test]
    fn the_share_line_reads_like_the_hooks() {
        let one = share_nudge_line(1, &["kb".to_string()]);
        assert_eq!(
            one,
            format!(
                "[crystalline] 1 change in the team domain kb is not yet shared. \
                 {MCP_SHARE_NUDGE_REASON}"
            )
        );
        let many = share_nudge_line(4, &["kb".to_string(), "ops".to_string()]);
        assert_eq!(
            many,
            format!(
                "[crystalline] 4 changes across 2 team domains are not yet shared. \
                 {MCP_SHARE_NUDGE_REASON}"
            )
        );
    }
}
