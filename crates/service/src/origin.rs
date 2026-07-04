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

use crystalline_remote::ops::{OriginStatusReport, PullReport};
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
/// proposals, skipped_large, re_baselined }`.
pub(crate) fn pull_report_json(domain: &str, report: &PullReport) -> Value {
    json!({
        "domain": domain,
        "up_to_date": report.up_to_date,
        "applied": report.applied,
        "merged": report.merged,
        "conflicts": report.conflicts,
        "proposals": report.proposals,
        "skipped_large": report.skipped_large,
        "re_baselined": report.re_baselined,
    })
}

/// Shapes one domain's [`OriginStatusReport`] into `origin_status`'s
/// per-domain entry: `{ domain, repo, branch, base_commit, behind,
/// local_changes, skipped_large, open_proposals, declined_proposals,
/// conflicts, last_checked }`.
pub(crate) fn status_report_json(domain: &str, report: &OriginStatusReport) -> Value {
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
    })
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
        let v = pull_report_json("eng", &report);
        assert_eq!(v["domain"], "eng");
        assert_eq!(v["up_to_date"], false);
        assert_eq!(v["applied"][0], "notes/a.md");
        assert_eq!(v["re_baselined"], false);
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
        let v = status_report_json("eng", &report);
        assert_eq!(v["domain"], "eng");
        assert_eq!(v["repo"], "acme/brand-knowledge");
        assert_eq!(v["behind"], true);
        assert_eq!(v["local_changes"], 2);
    }
}
