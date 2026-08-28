//! Pure helpers for GitHub-origin collaboration, factored out of
//! [`crate::engine::Engine`] so its `origin_add`, `origin_update`,
//! `origin_status`, `origin_share`, `origin_share_preview`, `origin_withdraw`
//! and `origin_resolve` methods stay orchestration-only: everything here is a
//! free function over plain data, with no access to `Engine`'s private state,
//! mirroring how [`crate::settings`] operates on
//! [`crystalline_core::config::GlobalConfig`] rather than reaching into the
//! engine itself.
//!
//! Nothing here talks to GitHub, the filesystem or the token store; that is
//! `crystalline_remote::ops` and `crystalline_remote::token`'s job. This
//! module only shapes the inputs (a default domain name, a default folder, a
//! token-store host key, a validated conflict resolution) and the outputs
//! (aggregate JSON) around those calls.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use crystalline_remote::RemoteError;
use crystalline_remote::changes::LocalChange;
use crystalline_remote::ops::{self, OriginStatusReport, ProposeOutcome, PullReport};
use crystalline_remote::state::{OriginState, ProposalStatus};
use serde_json::{Value, json};

use crate::engine::EngineError;
use crate::poller::DomainPollOutcome;

/// The domain name `origin_add` uses when the caller does not supply one: the
/// repository's own name segment (the part after the last `/`), run through
/// the same slug rules a permalink uses. Falls back to `domain` when the
/// segment slugifies to nothing (an unlikely but possible edge case, for
/// example a repo name made only of punctuation).
pub(crate) fn default_domain_name(repo: &str) -> String {
    let segment = repo.rsplit('/').next().unwrap_or(repo);
    let slug = crystalline_core::slugify(segment);
    if slug.is_empty() {
        "domain".to_string()
    } else {
        slug
    }
}

/// The domain folder a domain-creating call uses when the caller does not
/// supply one: `<root>/<domain>`, where `root` is the configured domains root
/// (`GlobalConfig::domains_root`, `~/Documents/Crystalline` by default). Kept a
/// free function over the already-resolved root so both `origin_add`, the
/// local-domain path and the standalone CLI's `domain add` (which has no
/// engine to call into) share one placement rule. Public (re-exported from
/// the crate root) for that last caller.
pub fn default_domain_folder(root: &Path, domain: &str) -> PathBuf {
    root.join(domain)
}

/// Parses an origin spec of the form `owner/repo[/subpath...]` into
/// `(owner/repo, subpath)`: the first segment is the owner, the second the
/// repository name and everything after the second `/` (if any) is the
/// subpath within the repository the team domain roots at. The error text is
/// deliberately subject-free ("must look like ...") so a caller can prefix its
/// own framing: `--origin` for the CLI flag, the offending variable name for
/// the environment overlay.
pub fn parse_origin_spec(spec: &str) -> Result<(String, Option<String>), String> {
    let mut parts = spec.splitn(3, '/');
    let owner = parts.next().filter(|s| !s.is_empty());
    let repo = parts.next().filter(|s| !s.is_empty());
    let (owner, repo) = match (owner, repo) {
        (Some(o), Some(r)) => (o, r),
        _ => {
            return Err(format!(
                "must look like owner/repo or owner/repo/subpath, got '{spec}'"
            ));
        }
    };
    let subpath = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    Ok((format!("{owner}/{repo}"), subpath))
}

/// Derives the token-store host key from `github.api_url`: `None` (the
/// GitHub.com account) when the api url is absent or is the default
/// `https://api.github.com`, or the bare Enterprise Server host otherwise.
///
/// Mirrors exactly the derivation `crystalline connect github --host <HOST>`
/// uses to decide where to save a token (`https://HOST/api/v3` as the api
/// url, then this same stripping), so a token saved for a given host is found
/// again by an engine operation reading `github.api_url` back from config.
pub(crate) fn token_host(api_url: Option<&str>) -> Option<String> {
    let auth_base = crystalline_remote::github::auth::auth_base(api_url);
    let bare = auth_base
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    if bare == "github.com" {
        None
    } else {
        Some(bare.to_string())
    }
}

/// Shapes one domain's [`PullReport`] into `origin_update`'s per-domain
/// aggregate entry: `{ domain, up_to_date, applied, merged, conflicts,
/// proposals, skipped_large, re_baselined }`. `proposals` is the caller's
/// already-joined view of `report.proposals` (see
/// [`proposal_transitions_json`]), each entry carrying the proposal's url and
/// title alongside its number and new status, rather than the bare
/// `(number, status)` pair `PullReport` itself carries.
pub(crate) fn pull_report_json(domain: &str, report: &PullReport, proposals: Vec<Value>) -> Value {
    json!({
        "domain": domain,
        "up_to_date": report.up_to_date,
        "applied": report.applied,
        "merged": report.merged,
        "conflicts": report.conflicts,
        "proposals": proposals,
        "skipped_large": report.skipped_large,
        "re_baselined": report.re_baselined,
    })
}

/// Joins `origin_update`'s proposal transitions (each a bare `(number,
/// status)` pair from [`PullReport::proposals`]) against `state`'s own
/// records to attach the url and title a human needs to actually open the
/// proposal: a still-open or declined transition is found in
/// `state.proposals`, a just-merged one has already moved to
/// `state.history` by the time `ops::pull` returns and saves. `state` is
/// `None` when the post-pull state could not be reloaded at all; a
/// transition with no match anywhere (should not happen, but is not fatal)
/// degrades to number and status only, with `url` and `title` left `null`,
/// never an error.
pub(crate) fn proposal_transitions_json(
    transitions: &[(u64, ProposalStatus)],
    state: Option<&OriginState>,
) -> Vec<Value> {
    transitions
        .iter()
        .map(|(number, status)| {
            let found = state.and_then(|s| {
                s.proposals
                    .iter()
                    .chain(s.history.iter())
                    .find(|p| p.number == *number)
            });
            json!({
                "number": number,
                "status": status,
                "url": found.map(|p| p.url.clone()),
                "title": found.map(|p| p.title.clone()),
            })
        })
        .collect()
}

/// Shapes one domain's [`OriginStatusReport`] into `origin_status`'s
/// per-domain entry: `{ domain, repo, branch, base_commit, behind,
/// local_changes, skipped_large, open_proposals, declined_proposals,
/// conflicts, last_checked, probe_error, stack_number, stack_wedged,
/// repair_pending, stack_link_pending }`. `probe_error` carries the live
/// probe's own error message, verbatim, when the probe failed for a
/// transport reason (offline, rate limited, an expired connection) and the
/// report was produced by retrying with no probe at all; `null` when the
/// probe succeeded or was never attempted (no connection).
///
/// Each open proposal travels as its full record (review feedback included)
/// decorated with `amended_upstream`: true when the live branch head no
/// longer matches the recorded one, so the caller can say a reviewer moved
/// the branch without a second round trip. It is false for every proposal
/// when the live list was not consulted at all (offline, or a failed list).
///
/// The four stack keys name where the domain's chain stands: `stack_number`
/// (the chain's number on the forge, `null` when nothing is stacked),
/// `stack_wedged` (Declined layers still carrying open layers above them, an
/// empty list when the chain is sound), and the two debts a caller can act on
/// by sharing or checking status again, `repair_pending` and
/// `stack_link_pending`. All four are always present, quiet rather than
/// absent off the stacked path, so one reader handles either path.
pub(crate) fn status_report_json(
    domain: &str,
    report: &OriginStatusReport,
    probe_error: Option<String>,
) -> Value {
    let open: Vec<Value> = report
        .open_proposals
        .iter()
        .map(|p| {
            let mut v = serde_json::to_value(p).expect("a proposal serializes");
            v["amended_upstream"] = json!(report.amended_upstream.contains(&p.number));
            v
        })
        .collect();
    json!({
        "domain": domain,
        "repo": report.repo,
        "branch": report.branch,
        "base_commit": report.base_commit,
        "behind": report.behind,
        "local_changes": report.local_changes,
        "skipped_large": report.skipped_large,
        "open_proposals": open,
        "declined_proposals": report.declined_proposals,
        "conflicts": report.conflicts,
        "last_checked": report.last_checked,
        "probe_error": probe_error,
        "stack_number": report.stack_number,
        "stack_wedged": report.stack_wedged,
        "repair_pending": report.repair_pending,
        "stack_link_pending": report.stack_link_pending,
    })
}

/// Whether `err` is the kind of error a live probe raises when the network
/// or the GitHub connection itself is the problem, rather than the domain's
/// own local state: [`RemoteError::Offline`], [`RemoteError::RateLimited`]
/// and [`RemoteError::AuthExpired`]. These are exactly the outcomes
/// `Provider::branch_head` can raise that have nothing to do with the
/// repository or domain being probed, so `origin_status` retries the same
/// domain with no provider at all rather than failing it outright, matching
/// the binding constraint that `origin_status` never hard-fails offline.
pub(crate) fn is_probe_transport_error(err: &RemoteError) -> bool {
    matches!(
        err,
        RemoteError::Offline | RemoteError::RateLimited { .. } | RemoteError::AuthExpired
    )
}

/// Shapes one domain's offline [`OriginStatusReport`] together with the
/// poller's own schedule and last result into `status_report`'s `origins`
/// block entry: `{ domain, repo, branch, last_checked, last_result,
/// next_due, open_proposals, declined_proposals, conflicts, local_changes,
/// stack_number, stack_wedged, repair_pending, stack_link_pending }`. The
/// four stack keys carry the same meaning they do in [`status_report_json`],
/// and are the one place the glance keeps whole lists rather than counts:
/// a wedged layer is named by number because that number is what a caller
/// withdraws or shares against. `next_due` and `last_result` are `null` for
/// a domain the poller has not scheduled or completed a tick for yet: a
/// freshly enabled or freshly added domain, or any domain when no daemon runs
/// the poller at all.
/// Unlike [`status_report_json`] (which embeds the full open and declined
/// proposal records for `origin_status`'s detailed view), this counts them:
/// the status overview stays a glance rather than a second copy of
/// `origin_status`.
pub(crate) fn origin_poll_status_json(
    domain: &str,
    report: &OriginStatusReport,
    next_due: Option<DateTime<Utc>>,
    last_result: Option<&DomainPollOutcome>,
) -> Value {
    json!({
        "domain": domain,
        "repo": report.repo,
        "branch": report.branch,
        "last_checked": report.last_checked,
        "last_result": last_result.map(poll_outcome_json),
        "next_due": next_due,
        "open_proposals": report.open_proposals.len(),
        "declined_proposals": report.declined_proposals.len(),
        "conflicts": report.conflicts.len(),
        "local_changes": report.local_changes,
        "stack_number": report.stack_number,
        "stack_wedged": report.stack_wedged,
        "repair_pending": report.repair_pending,
        "stack_link_pending": report.stack_link_pending,
    })
}

/// Shapes one [`DomainPollOutcome`] for `origin_poll_status_json`: `{
/// outcome: "up_to_date" }`, `{ outcome: "applied", applied, conflicts }` or
/// `{ outcome: "error", error }`.
fn poll_outcome_json(outcome: &DomainPollOutcome) -> Value {
    match outcome {
        DomainPollOutcome::UpToDate => json!({ "outcome": "up_to_date" }),
        DomainPollOutcome::Applied { applied, conflicts } => json!({
            "outcome": "applied",
            "applied": applied,
            "conflicts": conflicts,
        }),
        DomainPollOutcome::Error(message) => json!({
            "outcome": "error",
            "error": message,
        }),
    }
}

/// Shapes [`ops::propose`]'s outcome into `origin_share`'s JSON: `{ outcome:
/// "proposed", url, number, branch, added, updated, deleted, skipped_large,
/// summary, stack_number, stack_position }` when a pull request was opened
/// (a new layer on a chain included), `{ outcome: "updated", proposal }` when
/// an open proposal was amended in place, `{ outcome: "nothing_to_share",
/// skipped_large }` when the team already has everything the domain knows, or
/// `{ outcome: "proposal_diverged", proposal, guidance }` when a reviewer
/// moved the proposal branch and nothing was written.
///
/// `stack_number` names the chain this proposal belongs to on the forge and
/// `stack_position` is `[layer, open layers]` with a 1-based layer, so a
/// caller can say "layer 2 of 2 on stack #42" without a second call. Both are
/// `null` off the stacked path - an unstacked forge, a lone proposal - rather
/// than absent, so one shape reads either way.
///
/// The two are not null together on the stacked path: `stack_position` is
/// always set there, while `stack_number` is `null` when the call that links
/// the chain on the forge failed - every layer exists, they are simply not
/// grouped yet, and `stack_link_pending` in the status surfaces carries that
/// debt until a share or a probing status settles it. So a renderer keys off
/// `stack_position` to decide whether it is looking at a layer at all, and
/// names the stack number only when it has one.
///
/// The further outcome a caller may see, `conflicts_pending`, is not shaped
/// here: `Engine::origin_share` builds it directly from the reloaded conflict
/// list when `ops::propose` itself refuses, since
/// `RemoteError::ConflictsPending` alone carries only a count.
pub(crate) fn propose_outcome_json(outcome: &ProposeOutcome) -> Value {
    match outcome {
        ProposeOutcome::Proposed(report) => json!({
            "outcome": "proposed",
            "url": report.url,
            "number": report.number,
            "branch": report.branch,
            "added": report.added,
            "updated": report.updated,
            "deleted": report.deleted,
            "skipped_large": report.skipped_large,
            "summary": report.summary,
            "stack_number": report.stack_number,
            "stack_position": report.stack_position,
        }),
        ProposeOutcome::Updated(report) => json!({
            "outcome": "updated",
            "proposal": {
                "url": report.url,
                "number": report.number,
                "branch": report.branch,
                "added": report.added,
                "updated": report.updated,
                "deleted": report.deleted,
                "skipped_large": report.skipped_large,
                "summary": report.summary,
                "stack_number": report.stack_number,
                "stack_position": report.stack_position,
            },
        }),
        ProposeOutcome::NothingToShare { skipped_large } => json!({
            "outcome": "nothing_to_share",
            "skipped_large": skipped_large,
        }),
        ProposeOutcome::ProposalDiverged {
            number,
            url,
            branch,
        } => json!({
            "outcome": "proposal_diverged",
            "proposal": { "number": number, "url": url, "branch": branch },
            "guidance": DIVERGED_GUIDANCE,
        }),
    }
}

/// Shapes [`ops::propose_preview`]'s plan for `origin_share_preview` and the
/// REST changes route: always `effective_title` and `changes` (one
/// `{ path, kind }` entry per detected local change), plus the fields the
/// planned action itself carries - `number` and `url` for an update,
/// `top_number` and `top_title` for a `stack` (a new layer on an open chain),
/// `number`, `url` and `layers_above` for an `amend`, all three of `number`,
/// `url` and `branch` for a diverged proposal, `count` for pending conflicts
/// and nothing extra for a create or a no-op.
pub(crate) fn share_plan_json(plan: &ops::SharePlan) -> Value {
    let changes: Vec<Value> = plan
        .changes
        .changes
        .iter()
        .map(|c| {
            let kind = match c {
                LocalChange::Added { .. } => "added",
                LocalChange::Modified { .. } => "modified",
                LocalChange::Deleted { .. } => "deleted",
            };
            json!({ "path": c.path(), "kind": kind })
        })
        .collect();
    let mut v = json!({
        "effective_title": plan.effective_title,
        "changes": changes,
    });
    match &plan.action {
        ops::PlannedAction::Create => v["action"] = json!("create"),
        ops::PlannedAction::Update { number, url } => {
            v["action"] = json!("update");
            v["number"] = json!(number);
            v["url"] = json!(url);
        }
        ops::PlannedAction::StackOnTop {
            top_number,
            top_title,
        } => {
            v["action"] = json!("stack");
            v["top_number"] = json!(top_number);
            v["top_title"] = json!(top_title);
        }
        ops::PlannedAction::Amend {
            number,
            url,
            layers_above,
        } => {
            v["action"] = json!("amend");
            v["number"] = json!(number);
            v["url"] = json!(url);
            v["layers_above"] = json!(layers_above);
        }
        ops::PlannedAction::NothingToShare => v["action"] = json!("nothing_to_share"),
        ops::PlannedAction::ConflictsPending { count } => {
            v["action"] = json!("conflicts_pending");
            v["count"] = json!(count);
        }
        ops::PlannedAction::ProposalDiverged {
            number,
            url,
            branch,
        } => {
            v["action"] = json!("proposal_diverged");
            v["number"] = json!(number);
            v["url"] = json!(url);
            v["branch"] = json!(branch);
        }
    }
    v
}

/// Resolves the layer a withdrawal would take out and shapes it for
/// `origin_withdraw_preview`: `{ number, title, url, layers_above,
/// only_layer, reverting }`.
///
/// **Target resolution mirrors [`ops::withdraw`]'s, read off one offline
/// [`OriginStatusReport`] instead of origin state.** A named number is looked
/// for among the open layers and then the declined records - a declined
/// proposal can be withdrawn too, which tidies its record away - and a number
/// that is neither is [`RemoteError::ProposalNotFound`]. With no number the
/// target is the single open proposal, or, on a chain this machine knows as a
/// stack, the TOP open layer: the one nothing is built on. Anything else - no
/// open proposal, or the legacy multi-open shape from before stacking - is
/// [`RemoteError::NoWithdrawTarget`] carrying both candidate lists, exactly
/// the teaching error the withdrawal itself would raise a moment later.
///
/// The one deliberate difference is the stacks question. `ops::withdraw` asks
/// the forge (through the cached `stacks_available` verdict) whether this
/// origin serves stacks at all; a preview may not touch the forge, so it takes
/// `stacks_allowed` (the `github.stacks` setting) together with the two chain
/// facts the report already carries. Those can only disagree for a chain that
/// records a stack number no forge ever served, and the cost of the
/// disagreement is a question naming the top layer in front of a withdrawal
/// that then refuses - never a withdrawal of some layer the user was not
/// shown.
///
/// `layers_above` counts the OPEN layers standing above the target in chain
/// order (zero for the top one, and for a declined record, which stands in no
/// chain), because those are the layers a withdrawal re-bases. `only_layer`
/// says the target is the domain's one open proposal. `reverting` carries the
/// call's own `revert` flag through, so the question can name the working-tree
/// half of what it is about to do.
pub(crate) fn withdraw_plan_json(
    report: &OriginStatusReport,
    proposal: Option<u64>,
    revert: bool,
    stacks_allowed: bool,
) -> Result<Value, RemoteError> {
    let open = &report.open_proposals;
    let target = match proposal {
        Some(number) => open
            .iter()
            .chain(report.declined_proposals.iter())
            .find(|p| p.number == number)
            .ok_or(RemoteError::ProposalNotFound { number })?,
        None => {
            // `ops::chain_is_stacked`'s test, over the same three facts: a
            // chain of one is trivially stackable, and beyond that this
            // machine must know the chain as a stack (a recorded number, or a
            // link it still owes the forge).
            let stacked_chain = stacks_allowed
                && (open.len() <= 1 || report.stack_number.is_some() || report.stack_link_pending);
            match open.as_slice() {
                [single] => single,
                [.., top] if stacked_chain => top,
                _ => {
                    return Err(RemoteError::NoWithdrawTarget {
                        open: open.iter().map(|p| p.number).collect(),
                        declined: report.declined_proposals.iter().map(|p| p.number).collect(),
                    });
                }
            }
        }
    };
    let layers_above = open
        .iter()
        .position(|p| p.number == target.number)
        .map_or(0, |index| open.len() - index - 1);
    Ok(json!({
        "number": target.number,
        "title": target.title,
        "url": target.url,
        "layers_above": layers_above,
        "only_layer": open.len() == 1 && open[0].number == target.number,
        "reverting": revert,
    }))
}

/// Shapes [`ops::withdraw`]'s report into `origin_withdraw`'s JSON: the
/// proposal number, whether a live pull request was closed on the forge, the
/// fixed `"withdrawn"` status the record now carries, and the four
/// working-tree lists a revert produces (all empty without one).
/// `skipped_reverts` is the fourth: paths a revert left alone because their
/// pre-share content is nowhere to be had, distinct from `skipped_diverged`,
/// where the local file simply moved on.
///
/// `repaired` says the chain around the withdrawn layer was rebuilt, and
/// `restacked` names the NEW stack number that rebuild allocated - `null`
/// when no stack was recreated, which is both "no repair happened" and "the
/// survivors no longer make a chain", so a caller reads it together with
/// `repaired` rather than alone.
pub(crate) fn withdraw_report_json(report: &ops::WithdrawReport) -> Value {
    json!({
        "number": report.number,
        "closed": report.closed,
        "status": "withdrawn",
        "restored": report.restored,
        "deleted": report.deleted,
        "skipped_diverged": report.skipped_diverged,
        "skipped_reverts": report.skipped_reverts,
        "repaired": report.repaired,
        "restacked": report.restacked,
    })
}

/// What a caller relays when a reviewer amended the proposal branch.
pub(crate) const DIVERGED_GUIDANCE: &str = "A reviewer pushed commits onto this proposal's branch. Let the review finish and merge on GitHub, or withdraw the proposal and share again.";

/// Builds the [`ops::Resolution`] `origin_resolve` acts on from its `keep`
/// and `content` arguments, which must be exactly one of: `keep` is
/// `"mine"` or `"theirs"` with `content` absent, or `content` is present
/// with `keep` absent. Any other combination - both absent, both present or
/// an unrecognized `keep` value - is `EngineError::Invalid`, naming exactly
/// what is wrong.
pub(crate) fn resolution_from<'a>(
    keep: Option<&str>,
    content: Option<&'a [u8]>,
) -> Result<ops::Resolution<'a>, EngineError> {
    match (keep, content) {
        (Some("mine"), None) => Ok(ops::Resolution::Mine),
        (Some("theirs"), None) => Ok(ops::Resolution::Theirs),
        (None, Some(bytes)) => Ok(ops::Resolution::Merged(bytes)),
        (Some(other), None) => Err(EngineError::Invalid(format!(
            "origin_resolve keep must be mine or theirs, got '{other}'"
        ))),
        (None, None) => Err(EngineError::Invalid(
            "origin_resolve requires keep (mine or theirs) or content".to_string(),
        )),
        (Some(_), Some(_)) => Err(EngineError::Invalid(
            "origin_resolve accepts only one of keep or content, not both".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_origin_spec_reads_owner_and_repo() {
        let (repo, subpath) = parse_origin_spec("acme/brand-knowledge").unwrap();
        assert_eq!(repo, "acme/brand-knowledge");
        assert_eq!(subpath, None);
    }

    #[test]
    fn parse_origin_spec_reads_a_subpath() {
        let (repo, subpath) = parse_origin_spec("acme/monorepo/teams/brand").unwrap();
        assert_eq!(repo, "acme/monorepo");
        assert_eq!(subpath.as_deref(), Some("teams/brand"));
    }

    #[test]
    fn parse_origin_spec_rejects_a_bare_owner() {
        let err = parse_origin_spec("acme").unwrap_err();
        assert!(err.contains("must look like"), "{err}");
        assert!(err.contains("acme"), "{err}");
    }

    #[test]
    fn parse_origin_spec_rejects_an_empty_repo_segment() {
        assert!(parse_origin_spec("acme/").is_err());
        assert!(parse_origin_spec("/repo").is_err());
        assert!(parse_origin_spec("").is_err());
    }

    #[test]
    fn default_domain_name_slugifies_the_repo_s_last_segment() {
        assert_eq!(
            default_domain_name("acme/brand-knowledge"),
            "brand-knowledge"
        );
        assert_eq!(default_domain_name("acme/Team Notes"), "team-notes");
    }

    #[test]
    fn default_domain_name_falls_back_to_domain_when_the_segment_slugifies_to_nothing() {
        assert_eq!(default_domain_name("acme/---"), "domain");
        assert_eq!(default_domain_name(""), "domain");
    }

    #[test]
    fn default_domain_folder_joins_the_domain_under_the_root() {
        use crystalline_core::config::GlobalConfig;
        let root = GlobalConfig::default().domains_root();
        let folder = default_domain_folder(&root, "brand-knowledge");
        // Normalise separators so the suffix check holds on Windows, where the
        // join appends a backslash (`Documents/Crystalline\brand-knowledge`).
        let s = folder.display().to_string().replace('\\', "/");
        assert!(s.ends_with("Documents/Crystalline/brand-knowledge"), "{s}");
        assert!(!s.starts_with('~'), "{s}");
    }

    #[test]
    fn token_host_is_none_for_the_default_github_com_api_url() {
        assert_eq!(token_host(None), None);
        assert_eq!(token_host(Some("https://api.github.com")), None);
        assert_eq!(token_host(Some("https://api.github.com/")), None);
    }

    #[test]
    fn token_host_is_the_bare_host_for_a_ghes_api_url() {
        assert_eq!(
            token_host(Some("https://github.acme.example/api/v3")),
            Some("github.acme.example".to_string())
        );
    }

    #[test]
    fn pull_report_json_carries_the_domain_and_every_field() {
        let report = PullReport {
            up_to_date: false,
            applied: vec!["notes/a.md".to_string()],
            merged: vec![],
            conflicts: vec![],
            proposals: vec![],
            skipped_large: vec![],
            re_baselined: false,
        };
        let v = pull_report_json("eng", &report, Vec::new());
        assert_eq!(v["domain"], "eng");
        assert_eq!(v["up_to_date"], false);
        assert_eq!(v["applied"][0], "notes/a.md");
        assert_eq!(v["re_baselined"], false);
    }

    #[test]
    fn pull_report_json_carries_the_joined_proposal_transitions() {
        let report = PullReport {
            up_to_date: false,
            applied: vec![],
            merged: vec![],
            conflicts: vec![],
            proposals: vec![(7, ProposalStatus::Merged)],
            skipped_large: vec![],
            re_baselined: false,
        };
        let proposals = vec![json!({
            "number": 7,
            "status": ProposalStatus::Merged,
            "url": "https://github.com/acme/brand-knowledge/pull/7",
            "title": "Share Q3 notes",
        })];
        let v = pull_report_json("eng", &report, proposals);
        assert_eq!(v["proposals"][0]["number"], 7);
        assert_eq!(
            v["proposals"][0]["url"],
            "https://github.com/acme/brand-knowledge/pull/7"
        );
        assert_eq!(v["proposals"][0]["title"], "Share Q3 notes");
    }

    /// A `Proposal` fixture with just enough shape for the join tests: a
    /// number, url and title, nothing about its files.
    fn proposal_fixture(
        number: u64,
        url: &str,
        title: &str,
    ) -> crystalline_remote::state::Proposal {
        crystalline_remote::state::Proposal {
            number,
            url: url.to_string(),
            branch: format!("share/{number}"),
            title: title.to_string(),
            created_at: chrono::Utc::now(),
            status: ProposalStatus::Open,
            files: vec![],
            head_commit: None,
            pending_head_commit: None,
            base_commit: None,
            review_state: None,
            feedback: Vec::new(),
            updated_at: None,
        }
    }

    #[test]
    fn proposal_transitions_json_finds_an_open_or_declined_transition_in_state_proposals() {
        let mut state = OriginState::new("acme/brand-knowledge", "main");
        state.proposals.push(proposal_fixture(
            3,
            "https://github.com/acme/brand-knowledge/pull/3",
            "Share glossary edits",
        ));
        let transitions = vec![(3, ProposalStatus::Declined)];

        let v = proposal_transitions_json(&transitions, Some(&state));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0]["number"], 3);
        assert_eq!(
            v[0]["url"],
            "https://github.com/acme/brand-knowledge/pull/3"
        );
        assert_eq!(v[0]["title"], "Share glossary edits");
    }

    #[test]
    fn proposal_transitions_json_finds_a_just_merged_transition_in_state_history() {
        // A merged proposal has already left `proposals` for `history` by the
        // time `ops::pull` returns and saves.
        let mut state = OriginState::new("acme/brand-knowledge", "main");
        let mut merged = proposal_fixture(
            9,
            "https://github.com/acme/brand-knowledge/pull/9",
            "Share onboarding rewrite",
        );
        merged.status = ProposalStatus::Merged;
        state.push_history(merged);
        let transitions = vec![(9, ProposalStatus::Merged)];

        let v = proposal_transitions_json(&transitions, Some(&state));
        assert_eq!(v[0]["number"], 9);
        assert_eq!(
            v[0]["url"],
            "https://github.com/acme/brand-knowledge/pull/9"
        );
        assert_eq!(v[0]["title"], "Share onboarding rewrite");
    }

    #[test]
    fn proposal_transitions_json_degrades_to_number_and_status_when_state_is_absent() {
        let transitions = vec![(11, ProposalStatus::Merged)];
        let v = proposal_transitions_json(&transitions, None);
        assert_eq!(v[0]["number"], 11);
        assert!(v[0]["url"].is_null());
        assert!(v[0]["title"].is_null());
    }

    #[test]
    fn proposal_transitions_json_degrades_to_number_and_status_when_no_match_is_found() {
        let state = OriginState::new("acme/brand-knowledge", "main");
        let transitions = vec![(42, ProposalStatus::Declined)];
        let v = proposal_transitions_json(&transitions, Some(&state));
        assert_eq!(v[0]["number"], 42);
        assert!(v[0]["url"].is_null());
        assert!(v[0]["title"].is_null());
    }

    #[test]
    fn status_report_json_carries_the_domain_and_every_field() {
        let report = OriginStatusReport {
            repo: "acme/brand-knowledge".to_string(),
            branch: "main".to_string(),
            base_commit: "abc123".to_string(),
            behind: Some(true),
            local_changes: 2,
            skipped_large: vec![],
            open_proposals: vec![],
            declined_proposals: vec![],
            conflicts: vec![],
            last_checked: None,
            amended_upstream: vec![],
            stack_number: None,
            stack_wedged: vec![],
            repair_pending: false,
            stack_link_pending: false,
        };
        let v = status_report_json("eng", &report, None);
        assert_eq!(v["domain"], "eng");
        assert_eq!(v["repo"], "acme/brand-knowledge");
        assert_eq!(v["behind"], true);
        assert_eq!(v["local_changes"], 2);
        assert!(v["probe_error"].is_null());
    }

    #[test]
    fn status_report_json_names_the_stack_and_its_debts() {
        let report = OriginStatusReport {
            repo: "acme/brand-knowledge".to_string(),
            branch: "main".to_string(),
            base_commit: "abc123".to_string(),
            behind: Some(false),
            local_changes: 0,
            skipped_large: vec![],
            open_proposals: vec![],
            declined_proposals: vec![],
            conflicts: vec![],
            last_checked: None,
            amended_upstream: vec![],
            stack_number: Some(42),
            stack_wedged: vec![7],
            repair_pending: true,
            stack_link_pending: true,
        };
        let v = status_report_json("eng", &report, None);
        assert_eq!(v["stack_number"], 42);
        assert_eq!(v["stack_wedged"], json!([7]));
        assert_eq!(v["repair_pending"], true);
        assert_eq!(v["stack_link_pending"], true);
    }

    #[test]
    fn status_report_json_keeps_the_stack_fields_present_when_nothing_is_stacked() {
        let report = OriginStatusReport {
            repo: "acme/brand-knowledge".to_string(),
            branch: "main".to_string(),
            base_commit: "abc123".to_string(),
            behind: Some(false),
            local_changes: 0,
            skipped_large: vec![],
            open_proposals: vec![],
            declined_proposals: vec![],
            conflicts: vec![],
            last_checked: None,
            amended_upstream: vec![],
            stack_number: None,
            stack_wedged: vec![],
            repair_pending: false,
            stack_link_pending: false,
        };
        let v = status_report_json("eng", &report, None);
        assert!(v["stack_number"].is_null(), "{v}");
        assert_eq!(v["stack_wedged"], json!([]));
        assert_eq!(v["repair_pending"], false);
        assert_eq!(v["stack_link_pending"], false);
    }

    #[test]
    fn status_report_json_carries_a_probe_error_verbatim() {
        let report = OriginStatusReport {
            repo: "acme/brand-knowledge".to_string(),
            branch: "main".to_string(),
            base_commit: "abc123".to_string(),
            behind: None,
            local_changes: 0,
            skipped_large: vec![],
            open_proposals: vec![],
            declined_proposals: vec![],
            conflicts: vec![],
            last_checked: None,
            amended_upstream: vec![],
            stack_number: None,
            stack_wedged: vec![],
            repair_pending: false,
            stack_link_pending: false,
        };
        let message = RemoteError::Offline.to_string();
        let v = status_report_json("eng", &report, Some(message.clone()));
        assert_eq!(v["probe_error"], message);
    }

    fn poll_status_fixture() -> OriginStatusReport {
        OriginStatusReport {
            repo: "acme/brand-knowledge".to_string(),
            branch: "main".to_string(),
            base_commit: "abc123".to_string(),
            behind: None,
            local_changes: 3,
            skipped_large: vec![],
            open_proposals: vec![proposal_fixture(
                1,
                "https://github.com/acme/brand-knowledge/pull/1",
                "Share glossary edits",
            )],
            declined_proposals: vec![],
            conflicts: vec![],
            last_checked: None,
            amended_upstream: vec![],
            stack_number: None,
            stack_wedged: vec![],
            repair_pending: false,
            stack_link_pending: false,
        }
    }

    #[test]
    fn origin_poll_status_json_carries_the_domain_and_every_field() {
        let report = poll_status_fixture();
        let next_due = Utc::now();
        let outcome = DomainPollOutcome::Applied {
            applied: 2,
            conflicts: 0,
        };
        let v = origin_poll_status_json("eng", &report, Some(next_due), Some(&outcome));
        assert_eq!(v["domain"], "eng");
        assert_eq!(v["repo"], "acme/brand-knowledge");
        assert_eq!(v["branch"], "main");
        assert_eq!(v["open_proposals"], 1);
        assert_eq!(v["declined_proposals"], 0);
        assert_eq!(v["conflicts"], 0);
        assert_eq!(v["local_changes"], 3);
        assert_eq!(v["next_due"], serde_json::to_value(next_due).unwrap());
        assert_eq!(v["last_result"]["outcome"], "applied");
        assert_eq!(v["last_result"]["applied"], 2);
    }

    #[test]
    fn origin_poll_status_json_names_the_stack_and_its_debts() {
        let mut report = poll_status_fixture();
        report.stack_number = Some(42);
        report.stack_wedged = vec![7, 9];
        report.repair_pending = true;
        report.stack_link_pending = true;
        let v = origin_poll_status_json("eng", &report, None, None);
        assert_eq!(v["stack_number"], 42);
        assert_eq!(v["stack_wedged"], json!([7, 9]));
        assert_eq!(v["repair_pending"], true);
        assert_eq!(v["stack_link_pending"], true);

        // Present and quiet rather than absent when nothing is stacked: the
        // overview reads the same shape whichever path the domain is on.
        let plain = origin_poll_status_json("eng", &poll_status_fixture(), None, None);
        assert!(plain["stack_number"].is_null(), "{plain}");
        assert_eq!(plain["stack_wedged"], json!([]));
        assert_eq!(plain["repair_pending"], false);
        assert_eq!(plain["stack_link_pending"], false);
    }

    #[test]
    fn origin_poll_status_json_is_null_for_next_due_and_last_result_when_absent() {
        let report = poll_status_fixture();
        let v = origin_poll_status_json("eng", &report, None, None);
        assert!(v["next_due"].is_null());
        assert!(v["last_result"].is_null());
    }

    #[test]
    fn poll_outcome_json_shapes_every_variant() {
        assert_eq!(
            poll_outcome_json(&DomainPollOutcome::UpToDate)["outcome"],
            "up_to_date"
        );
        let applied = poll_outcome_json(&DomainPollOutcome::Applied {
            applied: 4,
            conflicts: 1,
        });
        assert_eq!(applied["outcome"], "applied");
        assert_eq!(applied["applied"], 4);
        assert_eq!(applied["conflicts"], 1);
        let error = poll_outcome_json(&DomainPollOutcome::Error("offline".to_string()));
        assert_eq!(error["outcome"], "error");
        assert_eq!(error["error"], "offline");
    }

    #[test]
    fn is_probe_transport_error_is_true_for_offline_rate_limited_and_auth_expired() {
        assert!(is_probe_transport_error(&RemoteError::Offline));
        assert!(is_probe_transport_error(&RemoteError::RateLimited {
            reset: None
        }));
        assert!(is_probe_transport_error(&RemoteError::AuthExpired));
    }

    #[test]
    fn is_probe_transport_error_is_false_for_a_domain_or_state_error() {
        assert!(!is_probe_transport_error(&RemoteError::RepoNotFound {
            repo: "acme/brand-knowledge".to_string()
        }));
        assert!(!is_probe_transport_error(&RemoteError::State(
            "corrupt".to_string()
        )));
    }

    #[test]
    fn propose_outcome_json_shapes_a_proposed_outcome() {
        let outcome = ProposeOutcome::Proposed(ops::ProposeReport {
            url: "https://github.com/acme/brand-knowledge/pull/3".to_string(),
            number: 3,
            branch: "crystalline/share-brand-240101120000".to_string(),
            added: vec!["notes/new.md".to_string()],
            updated: vec![],
            deleted: vec![],
            skipped_large: vec![],
            summary: "Shares 1 new engram.".to_string(),
            stack_number: None,
            stack_position: None,
        });
        let v = propose_outcome_json(&outcome);
        assert_eq!(v["outcome"], "proposed");
        assert_eq!(v["number"], 3);
        assert_eq!(v["added"][0], "notes/new.md");
        assert_eq!(v["summary"], "Shares 1 new engram.");
    }

    #[test]
    fn propose_outcome_json_shapes_an_updated_outcome_under_a_proposal_key() {
        let outcome = ProposeOutcome::Updated(ops::ProposeReport {
            url: "https://github.com/acme/brand-knowledge/pull/3".to_string(),
            number: 3,
            branch: "crystalline/share-brand-240101120000".to_string(),
            added: vec![],
            updated: vec!["notes/a.md".to_string()],
            deleted: vec![],
            skipped_large: vec![],
            summary: "Refines 1 engram.".to_string(),
            stack_number: None,
            stack_position: None,
        });
        let v = propose_outcome_json(&outcome);
        assert_eq!(v["outcome"], "updated");
        assert_eq!(v["proposal"]["number"], 3);
        assert_eq!(v["proposal"]["updated"][0], "notes/a.md");
        assert_eq!(v["proposal"]["summary"], "Refines 1 engram.");
    }

    #[test]
    fn propose_outcome_json_carries_the_stack_number_and_position() {
        let outcome = ProposeOutcome::Proposed(ops::ProposeReport {
            url: "https://github.com/acme/brand-knowledge/pull/8".to_string(),
            number: 8,
            branch: "crystalline/share-brand-240101120000".to_string(),
            added: vec![],
            updated: vec![],
            deleted: vec![],
            skipped_large: vec![],
            summary: "Shares 1 new engram.".to_string(),
            stack_number: Some(42),
            stack_position: Some((2, 2)),
        });
        let v = propose_outcome_json(&outcome);
        assert_eq!(v["stack_number"], 42);
        assert_eq!(v["stack_position"], json!([2, 2]));
    }

    #[test]
    fn propose_outcome_json_carries_the_stack_fields_on_an_amended_layer_too() {
        let outcome = ProposeOutcome::Updated(ops::ProposeReport {
            url: "https://github.com/acme/brand-knowledge/pull/8".to_string(),
            number: 8,
            branch: "crystalline/share-brand-240101120000".to_string(),
            added: vec![],
            updated: vec!["notes/a.md".to_string()],
            deleted: vec![],
            skipped_large: vec![],
            summary: "Refines 1 engram.".to_string(),
            stack_number: Some(42),
            stack_position: Some((1, 3)),
        });
        let v = propose_outcome_json(&outcome);
        assert_eq!(v["proposal"]["stack_number"], 42);
        assert_eq!(v["proposal"]["stack_position"], json!([1, 3]));
    }

    #[test]
    fn propose_outcome_json_leaves_the_stack_fields_null_off_the_stacked_path() {
        let outcome = ProposeOutcome::Proposed(ops::ProposeReport {
            url: "https://github.com/acme/brand-knowledge/pull/3".to_string(),
            number: 3,
            branch: "crystalline/share-brand-240101120000".to_string(),
            added: vec![],
            updated: vec![],
            deleted: vec![],
            skipped_large: vec![],
            summary: "Shares 1 new engram.".to_string(),
            stack_number: None,
            stack_position: None,
        });
        let v = propose_outcome_json(&outcome);
        assert!(v["stack_number"].is_null(), "{v}");
        assert!(v["stack_position"].is_null(), "{v}");
    }

    #[test]
    fn propose_outcome_json_shapes_a_proposal_diverged_outcome_with_guidance() {
        let outcome = ProposeOutcome::ProposalDiverged {
            number: 3,
            url: "https://github.com/acme/brand-knowledge/pull/3".to_string(),
            branch: "crystalline/share-brand-240101120000".to_string(),
        };
        let v = propose_outcome_json(&outcome);
        assert_eq!(v["outcome"], "proposal_diverged");
        assert_eq!(v["proposal"]["number"], 3);
        assert_eq!(
            v["proposal"]["branch"],
            "crystalline/share-brand-240101120000"
        );
        assert_eq!(v["guidance"], DIVERGED_GUIDANCE);
    }

    #[test]
    fn share_plan_json_names_the_action_and_every_change() {
        use crystalline_remote::changes::{LocalChange, LocalChanges};
        let plan = ops::SharePlan {
            action: ops::PlannedAction::Update {
                number: 4,
                url: "https://github.com/acme/brand-knowledge/pull/4".to_string(),
            },
            changes: LocalChanges {
                changes: vec![
                    LocalChange::Added {
                        path: "notes/new.md".to_string(),
                        sha256: "aa".to_string(),
                    },
                    LocalChange::Deleted {
                        path: "notes/old.md".to_string(),
                    },
                ],
                skipped_large: vec![],
            },
            effective_title: "Share updates from brand".to_string(),
        };
        let v = share_plan_json(&plan);
        assert_eq!(v["action"], "update");
        assert_eq!(v["number"], 4);
        assert_eq!(v["url"], "https://github.com/acme/brand-knowledge/pull/4");
        assert_eq!(v["effective_title"], "Share updates from brand");
        assert_eq!(
            v["changes"][0],
            json!({"path": "notes/new.md", "kind": "added"})
        );
        assert_eq!(
            v["changes"][1],
            json!({"path": "notes/old.md", "kind": "deleted"})
        );
    }

    #[test]
    fn share_plan_json_shapes_the_remaining_actions() {
        use crystalline_remote::changes::LocalChanges;
        let plan = |action| ops::SharePlan {
            action,
            changes: LocalChanges::default(),
            effective_title: String::new(),
        };
        assert_eq!(
            share_plan_json(&plan(ops::PlannedAction::Create))["action"],
            "create"
        );
        assert_eq!(
            share_plan_json(&plan(ops::PlannedAction::NothingToShare))["action"],
            "nothing_to_share"
        );
        let conflicts = share_plan_json(&plan(ops::PlannedAction::ConflictsPending { count: 2 }));
        assert_eq!(conflicts["action"], "conflicts_pending");
        assert_eq!(conflicts["count"], 2);
        let diverged = share_plan_json(&plan(ops::PlannedAction::ProposalDiverged {
            number: 9,
            url: "https://github.test/pull/9".to_string(),
            branch: "crystalline/share-brand".to_string(),
        }));
        assert_eq!(diverged["action"], "proposal_diverged");
        assert_eq!(diverged["number"], 9);
        assert_eq!(diverged["branch"], "crystalline/share-brand");
    }

    #[test]
    fn share_plan_json_names_the_stack_and_amend_actions() {
        use crystalline_remote::changes::LocalChanges;
        let plan = |action| ops::SharePlan {
            action,
            changes: LocalChanges::default(),
            effective_title: String::new(),
        };
        let stack = share_plan_json(&plan(ops::PlannedAction::StackOnTop {
            top_number: 6,
            top_title: "Share glossary edits".to_string(),
        }));
        assert_eq!(stack["action"], "stack");
        assert_eq!(stack["top_number"], 6);
        assert_eq!(stack["top_title"], "Share glossary edits");
        let amend = share_plan_json(&plan(ops::PlannedAction::Amend {
            number: 9,
            url: "https://github.test/pull/9".to_string(),
            layers_above: 1,
        }));
        assert_eq!(amend["action"], "amend");
        assert_eq!(amend["number"], 9);
        assert_eq!(amend["url"], "https://github.test/pull/9");
        assert_eq!(amend["layers_above"], 1);
    }

    /// A chain fixture for the withdrawal preview: `open` layers bottom-first,
    /// the way origin state and [`OriginStatusReport`] both order them.
    fn chain_fixture(open: &[u64], declined: &[u64], stacked: bool) -> OriginStatusReport {
        let proposals = |numbers: &[u64], status: ProposalStatus| {
            numbers
                .iter()
                .map(|n| {
                    let mut p = proposal_fixture(
                        *n,
                        &format!("https://github.test/pull/{n}"),
                        &format!("Layer {n}"),
                    );
                    p.status = status.clone();
                    p
                })
                .collect::<Vec<_>>()
        };
        OriginStatusReport {
            repo: "acme/brand-knowledge".to_string(),
            branch: "main".to_string(),
            base_commit: "abc123".to_string(),
            behind: None,
            local_changes: 0,
            skipped_large: vec![],
            open_proposals: proposals(open, ProposalStatus::Open),
            declined_proposals: proposals(declined, ProposalStatus::Declined),
            conflicts: vec![],
            last_checked: None,
            amended_upstream: vec![],
            stack_number: stacked.then_some(42),
            stack_wedged: vec![],
            repair_pending: false,
            stack_link_pending: false,
        }
    }

    #[test]
    fn withdraw_plan_json_defaults_to_the_top_open_layer_and_counts_what_stands_above() {
        // No number on a stacked chain: the top layer, the one nothing is
        // built on, exactly as `ops::withdraw` resolves it.
        let report = chain_fixture(&[5, 6, 7], &[], true);
        let v = withdraw_plan_json(&report, None, false, true).unwrap();
        assert_eq!(v["number"], 7);
        assert_eq!(v["title"], "Layer 7");
        assert_eq!(v["url"], "https://github.test/pull/7");
        assert_eq!(v["layers_above"], 0);
        assert_eq!(v["only_layer"], false);
        assert_eq!(v["reverting"], false);

        // A named layer lower down carries the cascade count with it.
        let v = withdraw_plan_json(&report, Some(5), true, true).unwrap();
        assert_eq!(v["number"], 5);
        assert_eq!(v["layers_above"], 2);
        assert_eq!(v["reverting"], true);
    }

    #[test]
    fn withdraw_plan_json_names_the_lone_open_proposal() {
        let report = chain_fixture(&[4], &[], false);
        let v = withdraw_plan_json(&report, None, false, false).unwrap();
        assert_eq!(v["number"], 4);
        assert_eq!(v["layers_above"], 0);
        assert_eq!(v["only_layer"], true);
    }

    /// The two refusals travel verbatim, because they are the teaching text
    /// the caller acts on: an unknown number, and the legacy multi-open shape
    /// this machine does not know as a stack.
    #[test]
    fn withdraw_plan_json_refuses_an_unknown_number_and_the_legacy_multi_open_shape() {
        let report = chain_fixture(&[5, 6], &[3], true);
        let err = withdraw_plan_json(&report, Some(99), false, true).unwrap_err();
        assert!(matches!(err, RemoteError::ProposalNotFound { number: 99 }));

        // Two open layers and no stack this machine knows: the caller has to
        // say which one, and both candidate lists ride along.
        let legacy = chain_fixture(&[5, 6], &[3], false);
        let err = withdraw_plan_json(&legacy, None, false, true).unwrap_err();
        match err {
            RemoteError::NoWithdrawTarget { open, declined } => {
                assert_eq!(open, vec![5, 6]);
                assert_eq!(declined, vec![3]);
            }
            other => panic!("{other}"),
        }

        // And nothing open at all is the same refusal.
        let empty = chain_fixture(&[], &[], false);
        assert!(matches!(
            withdraw_plan_json(&empty, None, false, true),
            Err(RemoteError::NoWithdrawTarget { .. })
        ));
    }

    /// A declined proposal can be withdrawn - it tidies the record away - so
    /// naming its number previews rather than refusing. Nothing stands above a
    /// record that stands in no chain, so the cascade count is zero.
    #[test]
    fn withdraw_plan_json_previews_a_declined_proposal_named_by_number() {
        let report = chain_fixture(&[5], &[3], false);
        let v = withdraw_plan_json(&report, Some(3), false, false).unwrap();
        assert_eq!(v["number"], 3);
        assert_eq!(v["layers_above"], 0);
        assert_eq!(v["only_layer"], false);
    }

    #[test]
    fn withdraw_report_json_carries_the_repair_and_the_unrevertable_paths() {
        let report = ops::WithdrawReport {
            number: 7,
            closed: true,
            skipped_reverts: vec!["notes/d.md".to_string()],
            repaired: true,
            restacked: Some(43),
            ..Default::default()
        };
        let v = withdraw_report_json(&report);
        assert_eq!(v["repaired"], true);
        assert_eq!(v["restacked"], 43);
        assert_eq!(v["skipped_reverts"], json!(["notes/d.md"]));

        // A withdrawal off the stacked path says so rather than staying
        // silent: false, null and an empty list, never absent keys.
        let plain = withdraw_report_json(&ops::WithdrawReport {
            number: 7,
            ..Default::default()
        });
        assert_eq!(plain["repaired"], false);
        assert!(plain["restacked"].is_null(), "{plain}");
        assert_eq!(plain["skipped_reverts"], json!([]));
    }

    #[test]
    fn withdraw_report_json_carries_the_number_and_the_file_lists() {
        let report = ops::WithdrawReport {
            number: 7,
            closed: true,
            restored: vec!["notes/a.md".to_string()],
            deleted: vec!["notes/b.md".to_string()],
            skipped_diverged: vec!["notes/c.md".to_string()],
            ..Default::default()
        };
        let v = withdraw_report_json(&report);
        assert_eq!(v["number"], 7);
        assert_eq!(v["closed"], true);
        assert_eq!(v["status"], "withdrawn");
        assert_eq!(v["restored"][0], "notes/a.md");
        assert_eq!(v["deleted"][0], "notes/b.md");
        assert_eq!(v["skipped_diverged"][0], "notes/c.md");
    }

    #[test]
    fn status_report_json_decorates_open_proposals_with_amended_upstream() {
        let report = OriginStatusReport {
            repo: "acme/brand-knowledge".to_string(),
            branch: "main".to_string(),
            base_commit: "abc123".to_string(),
            behind: Some(false),
            local_changes: 0,
            skipped_large: vec![],
            open_proposals: vec![
                proposal_fixture(1, "https://github.test/pull/1", "One"),
                proposal_fixture(2, "https://github.test/pull/2", "Two"),
            ],
            declined_proposals: vec![],
            conflicts: vec![],
            last_checked: None,
            amended_upstream: vec![2],
            stack_number: None,
            stack_wedged: vec![],
            repair_pending: false,
            stack_link_pending: false,
        };
        let v = status_report_json("eng", &report, None);
        assert_eq!(v["open_proposals"][0]["number"], 1);
        assert_eq!(v["open_proposals"][0]["amended_upstream"], false);
        assert_eq!(v["open_proposals"][1]["number"], 2);
        assert_eq!(v["open_proposals"][1]["amended_upstream"], true);
        // The full record travels, feedback included: this is the channel a
        // REST client reads review comments from.
        assert!(v["open_proposals"][0]["feedback"].is_array(), "{v}");
    }

    #[test]
    fn propose_outcome_json_shapes_a_nothing_to_share_outcome() {
        let outcome = ProposeOutcome::NothingToShare {
            skipped_large: vec![("notes/huge.md".to_string(), 999)],
        };
        let v = propose_outcome_json(&outcome);
        assert_eq!(v["outcome"], "nothing_to_share");
        assert_eq!(v["skipped_large"][0][0], "notes/huge.md");
    }

    #[test]
    fn resolution_from_maps_mine_and_theirs() {
        assert!(matches!(
            resolution_from(Some("mine"), None).unwrap(),
            ops::Resolution::Mine
        ));
        assert!(matches!(
            resolution_from(Some("theirs"), None).unwrap(),
            ops::Resolution::Theirs
        ));
    }

    #[test]
    fn resolution_from_maps_content_to_merged() {
        let content = b"merged bytes";
        match resolution_from(None, Some(content)).unwrap() {
            ops::Resolution::Merged(bytes) => assert_eq!(bytes, content),
            other => panic!("expected Merged, got {other:?}"),
        }
    }

    #[test]
    fn resolution_from_rejects_neither_keep_nor_content() {
        let err = resolution_from(None, None).unwrap_err();
        assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    }

    #[test]
    fn resolution_from_rejects_both_keep_and_content() {
        let err = resolution_from(Some("mine"), Some(b"x")).unwrap_err();
        assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    }

    #[test]
    fn resolution_from_rejects_an_unrecognized_keep_value() {
        let err = resolution_from(Some("nope"), None).unwrap_err();
        match err {
            EngineError::Invalid(msg) => assert!(msg.contains("nope"), "{msg}"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
