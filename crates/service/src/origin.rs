//! Pure helpers for GitHub-origin collaboration, factored out of
//! [`crate::engine::Engine`] so its `origin_add`, `origin_update`,
//! `origin_status`, `origin_share`, `origin_discard` and `origin_resolve`
//! methods stay orchestration-only: everything here is a free function over
//! plain data, with no access to `Engine`'s private state, mirroring how
//! [`crate::settings`] operates on [`crystalline_core::config::GlobalConfig`]
//! rather than reaching into the engine itself.
//!
//! Nothing here talks to GitHub, the filesystem or the token store; that is
//! `crystalline_remote::ops` and `crystalline_remote::token`'s job. This
//! module only shapes the inputs (a default domain name, a default folder, a
//! token-store host key, a validated conflict resolution) and the outputs
//! (aggregate JSON) around those calls.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use crystalline_remote::RemoteError;
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
/// free function over the already-resolved root so both `origin_add` and the
/// local-domain path share one placement rule.
pub(crate) fn default_domain_folder(root: &Path, domain: &str) -> PathBuf {
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
/// conflicts, last_checked, probe_error }`. `probe_error` carries the live
/// probe's own error message, verbatim, when the probe failed for a
/// transport reason (offline, rate limited, an expired connection) and the
/// report was produced by retrying with no probe at all; `null` when the
/// probe succeeded or was never attempted (no connection).
pub(crate) fn status_report_json(
    domain: &str,
    report: &OriginStatusReport,
    probe_error: Option<String>,
) -> Value {
    json!({
        "domain": domain,
        "repo": report.repo,
        "branch": report.branch,
        "base_commit": report.base_commit,
        "behind": report.behind,
        "local_changes": report.local_changes,
        "skipped_large": report.skipped_large,
        "open_proposals": report.open_proposals,
        "declined_proposals": report.declined_proposals,
        "conflicts": report.conflicts,
        "last_checked": report.last_checked,
        "probe_error": probe_error,
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
/// next_due, open_proposals, declined_proposals, conflicts, local_changes
/// }`. `next_due` and `last_result` are `null` for a domain the poller has
/// not scheduled or completed a tick for yet: a freshly enabled or freshly
/// added domain, or any domain when no daemon runs the poller at all.
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
/// summary }` when a pull request was opened, or `{ outcome:
/// "nothing_to_share", skipped_large }` when the team already has everything
/// the domain knows. The third outcome a caller may see, `conflicts_pending`,
/// is not shaped here: `Engine::origin_share` builds it directly from the
/// reloaded conflict list when `ops::propose` itself refuses, since
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
        }),
        ProposeOutcome::NothingToShare { skipped_large } => json!({
            "outcome": "nothing_to_share",
            "skipped_large": skipped_large,
        }),
    }
}

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
        };
        let v = status_report_json("eng", &report, None);
        assert_eq!(v["domain"], "eng");
        assert_eq!(v["repo"], "acme/brand-knowledge");
        assert_eq!(v["behind"], true);
        assert_eq!(v["local_changes"], 2);
        assert!(v["probe_error"].is_null());
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
        });
        let v = propose_outcome_json(&outcome);
        assert_eq!(v["outcome"], "proposed");
        assert_eq!(v["number"], 3);
        assert_eq!(v["added"][0], "notes/new.md");
        assert_eq!(v["summary"], "Shares 1 new engram.");
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
