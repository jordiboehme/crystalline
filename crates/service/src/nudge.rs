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

use chrono::{DateTime, TimeDelta, Utc};

use crate::engine::Engine;
use crate::maintenance::{self, MaintenanceState, NudgeKind, mcp_nudge_due};

/// What every line this module emits starts with, so an agent can tell the
/// knowledge base's own voice from the receipt it is attached to.
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
/// the throttle is a small JSON read and is asked FIRST, a domain with no
/// origin is never looked at, and a reviewing domain is answered from index
/// rows rather than by staging a share (which pulls, over the network, and has
/// no business inside a write receipt).
pub async fn write_verb_trailer(engine: &Engine, identity: Option<&str>) -> Option<String> {
    let state_dir = engine.journal_state_dir().ok()?;
    let key = identity_key(identity);
    let now = Utc::now();
    let state = maintenance::load_under(&state_dir);

    if mcp_nudge_due(&state, NudgeKind::Share, key, now) {
        let (count, domains) = unshared_team_work(engine, identity).await;
        if count > 0 && !domains.is_empty() {
            record(&state_dir, NudgeKind::Share, key, now);
            return Some(share_nudge_line(count, &domains));
        }
    }

    if mcp_nudge_due(&state, NudgeKind::Evolve, key, now) && evolve_due(engine, &state, now) {
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
async fn unshared_team_work(engine: &Engine, identity: Option<&str>) -> (u64, Vec<String>) {
    let actor = identity.unwrap_or(crate::engine::OWNER_IDENTITY_NAME);
    let config = engine.config();
    let mut changes = 0u64;
    let mut names = Vec::new();
    for (name, entry) in &config.domains {
        if entry.origin.is_none() {
            continue;
        }
        let count = if entry.is_overlay() {
            engine
                .overlay_counts_by_actor(name)
                .await
                .unwrap_or_default()
                .iter()
                .find(|(who, _)| who == actor)
                .map(|(_, held)| *held)
                .unwrap_or(0)
        } else {
            engine.unshared_change_count(name).unwrap_or(0)
        };
        if count == 0 {
            continue;
        }
        changes += count;
        names.push(name.clone());
    }
    (changes, names)
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
fn evolve_due(engine: &Engine, state: &MaintenanceState, now: DateTime<Utc>) -> bool {
    let config = engine.config();
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
