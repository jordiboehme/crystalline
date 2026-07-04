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
    /// it does not look like a domain Crystalline can subscribe to.
    #[error(
        "{repo} does not look like a knowledge domain: no MANIFEST.md was found {}",
        manifest_location(.path)
    )]
    NotADomain {
        /// The repository, `owner/name`.
        repo: String,
        /// The subpath checked within the repository, or `None` for the
        /// repository root.
        path: Option<String>,
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
        "{count} conflict(s) need to be settled before sharing; use resolve_conflict, then try again."
    )]
    ConflictsPending {
        /// How many conflicts are outstanding.
        count: usize,
    },

    /// A discard named a proposal number that is not among this domain's
    /// open or declined proposals: never registered, already discarded, or
    /// merged (and so already moved to history, not discardable).
    #[error("no open or declined proposal #{number} found for this domain")]
    ProposalNotFound {
        /// The proposal number that was not found.
        number: u64,
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

/// Renders where a MANIFEST.md was expected, for the `NotADomain` message.
fn manifest_location(path: &Option<String>) -> String {
    match path {
        Some(p) => format!("at {p}"),
        None => "at the repository root".to_string(),
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
        };
        assert!(err.to_string().contains("at the repository root"));
    }

    #[test]
    fn conflicts_pending_carries_the_count() {
        let err = RemoteError::ConflictsPending { count: 3 };
        let msg = err.to_string();
        assert!(msg.contains('3'), "{msg}");
        assert!(msg.contains("resolve_conflict"), "{msg}");
    }

    #[test]
    fn proposal_not_found_names_the_number() {
        let err = RemoteError::ProposalNotFound { number: 7 };
        let msg = err.to_string();
        assert!(msg.contains('7'), "{msg}");
        assert!(msg.contains("proposal"), "{msg}");
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
        // dash, no en dash and no Oxford-comma list in any rendered message.
        let em_dash = '\u{2014}';
        let en_dash = '\u{2013}';
        let samples = [
            RemoteError::NotEnabled.to_string(),
            RemoteError::NotConnected.to_string(),
            RemoteError::AuthPending.to_string(),
            RemoteError::AuthExpired.to_string(),
            RemoteError::Offline.to_string(),
            RemoteError::RateLimited { reset: None }.to_string(),
            RemoteError::RepoNotFound {
                repo: "acme/brand-knowledge".to_string(),
            }
            .to_string(),
            RemoteError::NotADomain {
                repo: "acme/brand-knowledge".to_string(),
                path: Some("knowledge".to_string()),
            }
            .to_string(),
            RemoteError::ConflictsPending { count: 1 }.to_string(),
            RemoteError::ProposalNotFound { number: 1 }.to_string(),
            RemoteError::ConflictNotFound {
                path: "notes/a.md".to_string(),
                open: vec!["notes/b.md".to_string()],
            }
            .to_string(),
            RemoteError::BaseUnavailable.to_string(),
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
