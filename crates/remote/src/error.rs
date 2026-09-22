//! The error taxonomy for GitHub-backed collaboration.
//!
//! Every variant carries an actionable message in product vocabulary: no
//! GitHub jargon, no raw HTTP status text, always a next step the user or
//! agent can take.

use thiserror::Error;

/// An error from the GitHub collaboration plumbing: the provider, the merge
/// engine or the origin state.
#[derive(Debug, Error)]
pub enum RemoteError {
    /// A collaboration operation was attempted while `github.enabled` is
    /// false.
    #[error(
        "GitHub collaboration is not enabled. Set github.enabled to true with the configure tool or crystalline config set github.enabled true."
    )]
    NotEnabled,

    /// A collaboration operation needs a GitHub connection and none exists
    /// yet.
    #[error(
        "Not connected to GitHub yet. Use configure to connect - you will get a short code to confirm at github.com/login/device."
    )]
    NotConnected,

    /// A device-flow sign-in was started but has not been confirmed in the
    /// browser yet.
    #[error(
        "Sign-in is waiting for confirmation: enter the code at github.com/login/device, then check again."
    )]
    AuthPending,

    /// The stored GitHub token no longer works: expired, revoked or the
    /// authorizing app was uninstalled.
    #[error("The GitHub connection has expired or was revoked. Use configure to sign in again.")]
    AuthExpired,

    /// A SAML-enforced organization refused the token until the OAuth app is
    /// authorized for it (GitHub 403 with an `X-GitHub-SSO: required` header).
    #[error(
        "GitHub requires single sign-on for the {org} organization before this token can reach it. Authorize the Crystalline app for {org}: open {url}, sign in through your identity provider, then retry. No collaborator change and no reconnect is needed."
    )]
    SsoAuthorizationRequired {
        /// The organization that enforces single sign-on, or `this` when
        /// neither the SSO url nor the repository named one.
        org: String,
        /// Where the person authorizes the app: GitHub's own `X-GitHub-SSO`
        /// url when it sent one, else the authorized-apps page.
        url: String,
    },

    /// An organization with OAuth App access restrictions has not approved
    /// the app (GitHub 403 whose message names the restriction).
    #[error(
        "The {org} organization restricts third-party OAuth apps and has not approved Crystalline yet. Ask an organization owner to approve it under the organization's Settings > Third-party access > OAuth app policy, or request it yourself under GitHub > Settings > Applications > Authorized OAuth Apps > Crystalline (Request next to {org}); then retry."
    )]
    OauthAppRestricted {
        /// The organization that restricts third-party apps, or `this` when
        /// the request named no repository owner.
        org: String,
    },

    /// GitHub is rate limiting requests from this machine.
    #[error("GitHub is rate limiting this machine; trying again later. Nothing is lost.")]
    RateLimited {
        /// When the rate limit window resets, if GitHub reported one.
        reset: Option<chrono::DateTime<chrono::Utc>>,
    },

    /// The configured repository does not exist, or is not visible with the
    /// current GitHub connection.
    #[error(
        "Could not find the GitHub repository {repo}. If it exists, a team admin may need to grant access."
    )]
    RepoNotFound {
        /// The repository, `owner/name`.
        repo: String,
    },

    /// The repository, or the given subpath within it, has no MANIFEST.md, so
    /// it does not look like a domain Crystalline can subscribe to. When the
    /// download the refusal is built from held a MANIFEST.md somewhere else,
    /// `candidates` names it so the caller can copy the path rather than
    /// guess it and re-learn it as folklore.
    #[error(
        "{repo} does not look like a knowledge domain: no MANIFEST.md was found {}{}",
        manifest_location(.path),
        manifest_candidates_clause(candidates, *more_candidates)
    )]
    NotADomain {
        /// The repository, `owner/name`.
        repo: String,
        /// The subpath checked within the repository, or `None` for the
        /// repository root.
        path: Option<String>,
        /// Every other MANIFEST.md this download actually holds, rendered as
        /// the subpath value a retry passes: repository-relative, shallowest
        /// first then lexical, capped at a handful. Empty when none were
        /// found, which keeps the message identical to a repository that
        /// truly has no domain in it anywhere.
        candidates: Vec<String>,
        /// How many further candidates the cap left out, `0` when the list
        /// above is everything that was found.
        more_candidates: usize,
    },

    /// GitHub could not be reached at all: DNS failure, connection refused or
    /// a timeout.
    #[error(
        "Could not reach GitHub - you appear to be offline. Everything local keeps working; sharing and updating will succeed once you are back online."
    )]
    Offline,

    /// A share was attempted while conflicts from a previous pull are still
    /// unresolved. Every proposal must be mergeable at creation, so sharing
    /// refuses until the conflicts are settled.
    #[error(
        "{} before sharing; use resolve_conflict, then try again.",
        conflicts_pending_clause(*count)
    )]
    ConflictsPending {
        /// How many conflicts are outstanding.
        count: usize,
    },

    /// A withdraw named a proposal number that is not among this domain's
    /// open or declined proposals: never registered, already withdrawn, or
    /// merged (and so already moved to history, not withdrawable).
    #[error("no open or declined proposal #{number} found for this domain")]
    ProposalNotFound {
        /// The proposal number that was not found.
        number: u64,
    },

    /// A withdraw with no proposal number found no single open proposal to
    /// act on: none is open, or (only possible in pre-living-proposal state)
    /// several are. Lists every candidate so the caller can retry naming one.
    #[error(
        "no single proposal to withdraw; pass a proposal number. Open: {}; declined: {}",
        join_numbers(open),
        join_numbers(declined)
    )]
    NoWithdrawTarget {
        /// The numbers of every open proposal this domain records.
        open: Vec<u64>,
        /// The numbers of every declined proposal this domain records.
        declined: Vec<u64>,
    },

    /// A resolve named a path with no recorded conflict for this domain.
    /// Names every currently open conflict path so the caller can retry with
    /// one that actually exists.
    #[error(
        "no conflict recorded for {path}; open conflicts: {}",
        if open.is_empty() { "none".to_string() } else { open.join(", ") }
    )]
    ConflictNotFound {
        /// The path that was requested.
        path: String,
        /// Every path with a currently open conflict.
        open: Vec<String>,
    },

    /// The recorded base commit is no longer reachable upstream, for example
    /// because the repository history was rewritten. Recovers automatically:
    /// the next pull re-baselines by fetching head and treating locally
    /// differing files as new local changes.
    #[error("The repository history changed underneath this domain; re-baselining automatically.")]
    BaseUnavailable,

    /// A stack operation was attempted against a provider that does not serve
    /// stacked pull requests. Every provider answers this by default: only a
    /// forge that actually stacks proposals overrides the stack verbs.
    #[error(
        "This forge does not serve stacked pull requests; shares use a single living proposal instead."
    )]
    StacksUnsupported,

    /// A teaching refusal: the caller asked for something the current state
    /// cannot honor, and the message names the way out (which proposal numbers
    /// are actually open, what to withdraw or merge first, what to pull
    /// before retrying). The caller's own request is at fault, never this
    /// machine and never the saved state, so the message travels with no
    /// prefix of any kind and every mapper classes this on the caller-fault
    /// side: a wrong proposal number must not read as a corrupt origin or as a
    /// server error, or the guidance is buried under a verdict that
    /// contradicts it.
    #[error("{0}")]
    Refused(String),

    /// The branch moved between this share's pull and its push: the forge
    /// refused a commit whose parent is no longer the head. Raised only by a
    /// direct share; `ops` pulls once more and retries once before it answers
    /// `BranchMoved`.
    #[error(
        "the branch {branch} moved while this share was prepared; run update_domain (or `crystalline origin update`) and share again"
    )]
    NotFastForward {
        /// The branch that moved.
        branch: String,
    },

    /// The branch's rules do not accept a direct commit. `message` is the
    /// forge's own sentence naming the rule.
    #[error("{}", branch_protected_guidance(branch, message))]
    BranchProtected {
        /// The branch the rule guards.
        branch: String,
        /// The forge's sentence, verbatim.
        message: String,
    },

    /// The GitHub API answered in a shape this client did not expect, with no
    /// more specific variant to map it to.
    #[error("GitHub returned an unexpected answer (status {status}): {message}")]
    Api {
        /// The HTTP status code.
        status: u16,
        /// The response body, or a short description of it.
        message: String,
    },

    /// A filesystem error while reading or writing origin state, a base
    /// snapshot or a working-tree file.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// An on-disk origin state file could not be parsed: corrupt content or
    /// an unexpected shape. A plain read or write failure is `Io`, not this.
    #[error("the origin state is corrupt: {0}")]
    State(String),

    /// The saved GitHub credential, in the OS keychain or the local token
    /// file, could not be read, written or deleted: a credential backend
    /// refusing the operation (locked, revoked permissions, no backend
    /// available at all) or a token file whose content is not valid JSON.
    /// A plain filesystem error opening or creating the token file itself
    /// is `Io`, not this.
    #[error(
        "The saved GitHub sign-in could not be read or written: {detail}. Use configure to sign in again."
    )]
    Credential {
        /// A short, human-readable description of what went wrong.
        detail: String,
    },
}

/// The sentence a caller is given when a branch rule refused a direct
/// commit: what the forge said, and the two ways out (share as a proposal,
/// or have the rule relaxed).
///
/// Lives here rather than beside each surface that renders it, so the
/// [`RemoteError::BranchProtected`] display and every receipt carrying a
/// `guidance` field say the same thing word for word.
pub fn branch_protected_guidance(branch: &str, message: &str) -> String {
    format!(
        "The branch {branch} does not accept direct commits ({message}). Set `sharing: proposal` in this domain's MANIFEST so shares open a proposal the branch's rules can review, or ask a repository admin to allow direct pushes."
    )
}

/// The sentence a caller is given when a direct share gave up because the
/// branch moved a second time, after the retry's pull had already caught up
/// with the first move.
///
/// Deliberately not [`RemoteError::NotFastForward`]'s own text: that one is
/// raised on the first refusal, which the share answers by pulling and
/// retrying, and this one is what a person reads once the retry lost the
/// race too ("moved again").
pub fn branch_moved_guidance(branch: &str) -> String {
    format!(
        "the branch {branch} moved again while this share was prepared; run update_domain (or `crystalline origin update`) and share again"
    )
}

/// Renders where a MANIFEST.md was expected, for the `NotADomain` message.
fn manifest_location(path: &Option<String>) -> String {
    match path {
        Some(p) => format!("at {p}"),
        None => "at the repository root".to_string(),
    }
}

/// Renders the `NotADomain` suggestion clause: empty when nothing else was
/// found, so the message reads exactly as it always did for a repository with
/// no domain in it anywhere. Otherwise names every MANIFEST.md the download
/// actually held and the subpath value a retry passes for each, so the
/// caller copies a fact instead of guessing one.
fn manifest_candidates_clause(candidates: &[String], more: usize) -> String {
    if candidates.is_empty() {
        return String::new();
    }
    let manifests: Vec<String> = candidates
        .iter()
        .map(|c| format!("{c}/MANIFEST.md"))
        .collect();
    let found = if more > 0 {
        format!("{}, and {more} more", manifests.join(", "))
    } else {
        join_with(&manifests, "and")
    };
    let pass = join_with(candidates, "or");
    // The value has to be named as what it is passed as. "pass memory" reads
    // like an instruction to pass something called memory somewhere; the
    // subpath is the `path` parameter, and saying so is the difference between
    // a fact to copy and a guess to make.
    format!(". Found MANIFEST.md at {found}; pass {pass} as the path.")
}

/// Joins a list in natural language with `conj` ("and" or "or") before the
/// last item: one item alone, `"a {conj} b"` for two, `"a, b, {conj} c"` for
/// three or more.
fn join_with(items: &[String], conj: &str) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} {conj} {second}"),
        [init @ .., last] => format!("{}, {conj} {last}", init.join(", ")),
    }
}

/// Renders a proposal-number list for the `NoWithdrawTarget` message:
/// `"none"` for an empty list, `"#3, #7"` otherwise.
fn join_numbers(numbers: &[u64]) -> String {
    if numbers.is_empty() {
        "none".to_string()
    } else {
        numbers
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The leading clause of the `ConflictsPending` message, singular or plural
/// depending on `count`: `"1 conflict needs to be settled"` or `"N conflicts
/// need to be settled"`.
fn conflicts_pending_clause(count: usize) -> String {
    if count == 1 {
        "1 conflict needs to be settled".to_string()
    } else {
        format!("{count} conflicts need to be settled")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_enabled_message_is_actionable() {
        assert_eq!(
            RemoteError::NotEnabled.to_string(),
            "GitHub collaboration is not enabled. Set github.enabled to true with the configure tool or crystalline config set github.enabled true."
        );
    }

    /// A refusal renders bare. The whole point of the variant is that the
    /// teaching text reaches the caller with nothing in front of it: a
    /// framing clause like "the origin state is corrupt" would blame this
    /// machine for what the request asked for, and the reader would stop
    /// at the verdict instead of following the way out.
    #[test]
    fn refused_renders_its_message_with_no_prefix() {
        let text = "proposal #9 is not an open layer of this domain; open layers: #3 (layer 1)";
        assert_eq!(RemoteError::Refused(text.to_string()).to_string(), text);
    }

    /// A branch rule refusing a direct commit has to name three things: the
    /// branch, the forge's own sentence for the rule and what to do instead.
    /// The way out is the policy key, because that is the one a person can
    /// change without asking anybody.
    #[test]
    fn branch_protected_names_the_branch_the_rule_and_the_way_out() {
        let text = RemoteError::BranchProtected {
            branch: "main".to_string(),
            message: "Changes must be made through a pull request.".to_string(),
        }
        .to_string();
        assert!(
            text.starts_with(
                "The branch main does not accept direct commits (Changes must be made through a pull request.)."
            ),
            "{text}"
        );
        assert!(text.contains("sharing: proposal"), "{text}");
    }

    /// The two guidance sentences are one move apart: the error a refused
    /// push raises says the branch moved, the guidance a given-up share
    /// carries says it moved again - after the retry's own pull.
    #[test]
    fn branch_moved_guidance_names_the_second_move() {
        let guidance = branch_moved_guidance("main");
        assert!(guidance.contains("moved again while"), "{guidance}");
        assert!(guidance.contains("share again"), "{guidance}");
        let first = RemoteError::NotFastForward {
            branch: "main".to_string(),
        }
        .to_string();
        assert!(
            first.contains("moved while this share was prepared"),
            "{first}"
        );
        assert!(!first.contains("moved again"), "{first}");
    }

    #[test]
    fn not_connected_message_points_to_the_device_flow() {
        assert_eq!(
            RemoteError::NotConnected.to_string(),
            "Not connected to GitHub yet. Use configure to connect - you will get a short code to confirm at github.com/login/device."
        );
    }

    #[test]
    fn auth_pending_message_is_actionable() {
        assert_eq!(
            RemoteError::AuthPending.to_string(),
            "Sign-in is waiting for confirmation: enter the code at github.com/login/device, then check again."
        );
    }

    #[test]
    fn auth_expired_message_is_actionable() {
        assert_eq!(
            RemoteError::AuthExpired.to_string(),
            "The GitHub connection has expired or was revoked. Use configure to sign in again."
        );
    }

    #[test]
    fn offline_message_reassures_local_work_keeps_going() {
        assert_eq!(
            RemoteError::Offline.to_string(),
            "Could not reach GitHub - you appear to be offline. Everything local keeps working; sharing and updating will succeed once you are back online."
        );
    }

    #[test]
    fn rate_limited_message_says_nothing_is_lost() {
        let err = RemoteError::RateLimited { reset: None };
        assert_eq!(
            err.to_string(),
            "GitHub is rate limiting this machine; trying again later. Nothing is lost."
        );
    }

    #[test]
    fn rate_limited_carries_an_optional_reset_time() {
        let reset = chrono::Utc::now();
        let err = RemoteError::RateLimited { reset: Some(reset) };
        match err {
            RemoteError::RateLimited { reset: got } => assert_eq!(got, Some(reset)),
            _ => panic!("expected RateLimited"),
        }
    }

    /// The single sign-on refusal is the one 403 the person can clear alone,
    /// so its message has to carry the org, the exact url and the two things
    /// that are NOT the fix: no collaborator change, no reconnect.
    #[test]
    fn sso_authorization_required_names_the_org_the_url_and_what_is_not_needed() {
        let err = RemoteError::SsoAuthorizationRequired {
            org: "acme".to_string(),
            url: "https://github.com/orgs/acme/sso?authorization_request=abc".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "GitHub requires single sign-on for the acme organization before this token can reach it. Authorize the Crystalline app for acme: open https://github.com/orgs/acme/sso?authorization_request=abc, sign in through your identity provider, then retry. No collaborator change and no reconnect is needed."
        );
    }

    /// The OAuth App restriction is the one 403 somebody ELSE has to clear,
    /// so the message names the owner's page and the person's own request
    /// path rather than any retry-and-hope instruction.
    #[test]
    fn oauth_app_restricted_names_both_the_owner_page_and_the_request_path() {
        let err = RemoteError::OauthAppRestricted {
            org: "acme".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "The acme organization restricts third-party OAuth apps and has not approved Crystalline yet. Ask an organization owner to approve it under the organization's Settings > Third-party access > OAuth app policy, or request it yourself under GitHub > Settings > Applications > Authorized OAuth Apps > Crystalline (Request next to acme); then retry."
        );
    }

    #[test]
    fn repo_not_found_names_the_repo_and_hints_at_access() {
        let err = RemoteError::RepoNotFound {
            repo: "acme/brand-knowledge".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("acme/brand-knowledge"), "{msg}");
        assert!(msg.contains("team admin"), "{msg}");
    }

    #[test]
    fn not_a_domain_mentions_the_subpath_when_present() {
        let err = RemoteError::NotADomain {
            repo: "acme/brand-knowledge".to_string(),
            path: Some("knowledge".to_string()),
            candidates: vec![],
            more_candidates: 0,
        };
        let msg = err.to_string();
        assert!(msg.contains("acme/brand-knowledge"), "{msg}");
        assert!(msg.contains("at knowledge"), "{msg}");
        assert!(msg.contains("MANIFEST.md"), "{msg}");
    }

    #[test]
    fn not_a_domain_mentions_the_repository_root_when_path_absent() {
        let err = RemoteError::NotADomain {
            repo: "acme/brand-knowledge".to_string(),
            path: None,
            candidates: vec![],
            more_candidates: 0,
        };
        assert!(err.to_string().contains("at the repository root"));
    }

    /// A repository whose only manifest sits one folder down: the refusal
    /// names it, and the value it names is exactly what a retry passes as
    /// the subpath - copied, not translated.
    #[test]
    fn not_a_domain_names_a_manifest_found_one_folder_down() {
        let err = RemoteError::NotADomain {
            repo: "planview-dev/scotty-knowledge".to_string(),
            path: None,
            candidates: vec!["memory".to_string()],
            more_candidates: 0,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("no MANIFEST.md was found at the repository root"),
            "{msg}"
        );
        assert!(msg.contains("memory/MANIFEST.md"), "{msg}");
        assert!(msg.contains("pass memory"), "{msg}");
    }

    /// Two depths: both are listed, shallowest first, so the caller sees the
    /// more likely one named first without having to compare depths itself.
    #[test]
    fn not_a_domain_lists_manifests_at_two_depths_shallowest_first() {
        let err = RemoteError::NotADomain {
            repo: "acme/brand-knowledge".to_string(),
            path: None,
            candidates: vec!["memory".to_string(), "archive/notes".to_string()],
            more_candidates: 0,
        };
        let msg = err.to_string();
        let memory_at = msg.find("memory/MANIFEST.md").expect(&msg);
        let archive_at = msg.find("archive/notes/MANIFEST.md").expect(&msg);
        assert!(memory_at < archive_at, "{msg}");
        assert!(msg.contains("pass memory or archive/notes"), "{msg}");
    }

    /// No MANIFEST.md anywhere in what was downloaded: the message is
    /// unchanged from before this feature existed, which is now true and
    /// complete rather than a guess about what else might be there.
    #[test]
    fn not_a_domain_keeps_todays_wording_when_nothing_else_was_found() {
        let err = RemoteError::NotADomain {
            repo: "acme/brand-knowledge".to_string(),
            path: None,
            candidates: vec![],
            more_candidates: 0,
        };
        assert_eq!(
            err.to_string(),
            "acme/brand-knowledge does not look like a knowledge domain: no MANIFEST.md was found at the repository root"
        );
    }

    /// Asked for at a subpath that itself has no manifest, while one exists
    /// nested under that same subpath: the candidate names the OTHER path,
    /// composed with the requested subpath folded back in, since that is
    /// what a retry must pass from the repository root.
    #[test]
    fn not_a_domain_names_a_manifest_found_elsewhere_under_the_requested_subpath() {
        let err = RemoteError::NotADomain {
            repo: "acme/brand-knowledge".to_string(),
            path: Some("wrong".to_string()),
            candidates: vec!["wrong/memory".to_string()],
            more_candidates: 0,
        };
        let msg = err.to_string();
        assert!(msg.contains("no MANIFEST.md was found at wrong"), "{msg}");
        assert!(msg.contains("wrong/memory/MANIFEST.md"), "{msg}");
        assert!(msg.contains("pass wrong/memory"), "{msg}");
    }

    /// The cap: more candidates exist than the message lists, and it says
    /// so rather than pretending the list is exhaustive.
    #[test]
    fn not_a_domain_says_how_many_more_candidates_the_cap_left_out() {
        let err = RemoteError::NotADomain {
            repo: "acme/brand-knowledge".to_string(),
            path: None,
            candidates: vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
            ],
            more_candidates: 3,
        };
        let msg = err.to_string();
        assert!(msg.contains("a/MANIFEST.md"), "{msg}");
        assert!(msg.contains("e/MANIFEST.md"), "{msg}");
        assert!(msg.contains("3 more"), "{msg}");
    }

    #[test]
    fn conflicts_pending_carries_the_count() {
        let err = RemoteError::ConflictsPending { count: 3 };
        let msg = err.to_string();
        assert!(msg.contains('3'), "{msg}");
        assert!(msg.contains("resolve_conflict"), "{msg}");
    }

    #[test]
    fn conflicts_pending_uses_singular_wording_for_one() {
        assert_eq!(
            RemoteError::ConflictsPending { count: 1 }.to_string(),
            "1 conflict needs to be settled before sharing; use resolve_conflict, then try again."
        );
    }

    #[test]
    fn conflicts_pending_uses_plural_wording_for_more_than_one() {
        assert_eq!(
            RemoteError::ConflictsPending { count: 2 }.to_string(),
            "2 conflicts need to be settled before sharing; use resolve_conflict, then try again."
        );
    }

    #[test]
    fn proposal_not_found_names_the_number() {
        let err = RemoteError::ProposalNotFound { number: 7 };
        let msg = err.to_string();
        assert!(msg.contains('7'), "{msg}");
        assert!(msg.contains("proposal"), "{msg}");
    }

    #[test]
    fn no_withdraw_target_reports_none_when_nothing_is_open_or_declined() {
        let err = RemoteError::NoWithdrawTarget {
            open: vec![],
            declined: vec![],
        };
        let msg = err.to_string();
        assert!(msg.contains("pass a proposal number"), "{msg}");
        assert!(msg.contains("Open: none"), "{msg}");
        assert!(msg.contains("declined: none"), "{msg}");
    }

    #[test]
    fn no_withdraw_target_lists_every_candidate_number() {
        let err = RemoteError::NoWithdrawTarget {
            open: vec![3, 7],
            declined: vec![1],
        };
        let msg = err.to_string();
        assert!(msg.contains("Open: #3, #7"), "{msg}");
        assert!(msg.contains("declined: #1"), "{msg}");
    }

    #[test]
    fn conflict_not_found_names_the_path_and_lists_open_conflicts() {
        let err = RemoteError::ConflictNotFound {
            path: "notes/missing.md".to_string(),
            open: vec!["notes/a.md".to_string(), "notes/b.md".to_string()],
        };
        let msg = err.to_string();
        assert!(msg.contains("notes/missing.md"), "{msg}");
        assert!(msg.contains("notes/a.md"), "{msg}");
        assert!(msg.contains("notes/b.md"), "{msg}");
    }

    #[test]
    fn conflict_not_found_reports_none_when_no_conflicts_are_open() {
        let err = RemoteError::ConflictNotFound {
            path: "notes/missing.md".to_string(),
            open: vec![],
        };
        assert!(err.to_string().contains("none"));
    }

    #[test]
    fn base_unavailable_mentions_re_baselining() {
        assert!(
            RemoteError::BaseUnavailable
                .to_string()
                .contains("re-baselining")
        );
    }

    #[test]
    fn stacks_unsupported_says_what_happens_instead() {
        assert_eq!(
            RemoteError::StacksUnsupported.to_string(),
            "This forge does not serve stacked pull requests; shares use a single living proposal instead."
        );
    }

    #[test]
    fn api_error_carries_status_and_message() {
        let err = RemoteError::Api {
            status: 502,
            message: "bad gateway".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("502"), "{msg}");
        assert!(msg.contains("bad gateway"), "{msg}");
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::other("disk full");
        let err: RemoteError = io_err.into();
        assert!(matches!(err, RemoteError::Io(_)));
        assert!(err.to_string().contains("disk full"));
    }

    #[test]
    fn state_error_carries_its_message() {
        let err = RemoteError::State("unexpected version 3".to_string());
        assert!(err.to_string().contains("unexpected version 3"));
    }

    #[test]
    fn credential_error_carries_its_detail_and_points_to_signing_in_again() {
        let err = RemoteError::Credential {
            detail: "the keychain is locked".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("the keychain is locked"), "{msg}");
        assert!(msg.contains("configure to sign in again"), "{msg}");
    }

    #[test]
    fn no_style_lint_violations_in_messages() {
        // A cheap in-process guard mirroring scripts/style-lint.sh: no em
        // dash and no en dash in any rendered message.
        let em_dash = '\u{2014}';
        let en_dash = '\u{2013}';
        let samples = [
            RemoteError::NotEnabled.to_string(),
            RemoteError::NotConnected.to_string(),
            RemoteError::AuthPending.to_string(),
            RemoteError::AuthExpired.to_string(),
            RemoteError::Offline.to_string(),
            RemoteError::RateLimited { reset: None }.to_string(),
            RemoteError::SsoAuthorizationRequired {
                org: "acme".to_string(),
                url: "https://github.com/orgs/acme/sso".to_string(),
            }
            .to_string(),
            RemoteError::OauthAppRestricted {
                org: "acme".to_string(),
            }
            .to_string(),
            RemoteError::RepoNotFound {
                repo: "acme/brand-knowledge".to_string(),
            }
            .to_string(),
            RemoteError::NotADomain {
                repo: "acme/brand-knowledge".to_string(),
                path: Some("knowledge".to_string()),
                candidates: vec!["knowledge/memory".to_string(), "archive/notes".to_string()],
                more_candidates: 2,
            }
            .to_string(),
            RemoteError::ConflictsPending { count: 1 }.to_string(),
            RemoteError::ProposalNotFound { number: 1 }.to_string(),
            RemoteError::NoWithdrawTarget {
                open: vec![3],
                declined: vec![],
            }
            .to_string(),
            RemoteError::ConflictNotFound {
                path: "notes/a.md".to_string(),
                open: vec!["notes/b.md".to_string()],
            }
            .to_string(),
            RemoteError::BaseUnavailable.to_string(),
            RemoteError::StacksUnsupported.to_string(),
            RemoteError::Refused("withdraw the proposal and share again".to_string()).to_string(),
            RemoteError::Api {
                status: 500,
                message: "boom".to_string(),
            }
            .to_string(),
            RemoteError::State("bad".to_string()).to_string(),
            RemoteError::Credential {
                detail: "boom".to_string(),
            }
            .to_string(),
        ];
        for msg in samples {
            assert!(!msg.contains(em_dash), "{msg}");
            assert!(!msg.contains(en_dash), "{msg}");
        }
    }
}
