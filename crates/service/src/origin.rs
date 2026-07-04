//! Pure helpers for GitHub-origin collaboration, factored out of
//! [`crate::engine::Engine`] so its `origin_add`, `origin_update` and
//! `origin_status` methods stay orchestration-only: everything here is a free
//! function over plain data, with no access to `Engine`'s private state,
//! mirroring how [`crate::settings`] operates on [`crystalline_core::config::GlobalConfig`]
//! rather than reaching into the engine itself.
//!
//! Nothing here talks to GitHub, the filesystem or the token store; that is
//! `crystalline_remote::ops` and `crystalline_remote::token`'s job. This
//! module only shapes the inputs (a default domain name, a default folder, a
//! token-store host key) and the outputs (aggregate JSON) around those calls.

use std::path::PathBuf;

use crystalline_remote::RemoteError;
use crystalline_remote::ops::{OriginStatusReport, PullReport};
use crystalline_remote::state::{OriginState, ProposalStatus};
use serde_json::{Value, json};

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

/// The domain folder `origin_add` uses when the caller does not supply one:
/// `~/Documents/Crystalline/<domain>`, tilde-expanded via the core helper so
/// it resolves the same way every other configured path does.
pub(crate) fn default_domain_folder(domain: &str) -> PathBuf {
    crystalline_core::config::expand_tilde(&format!("~/Documents/Crystalline/{domain}"))
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn default_domain_folder_expands_under_documents_crystalline() {
        let folder = default_domain_folder("brand-knowledge");
        let s = folder.display().to_string();
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
}
