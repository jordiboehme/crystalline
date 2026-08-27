//! The forge-neutral provider trait and its wire types.
//!
//! [`Provider`] is the seam between the merge engine and orchestration in
//! this crate and a concrete forge client. `GitHubProvider` (a later task) is
//! the only implementation today, built over the GitHub REST and Git Data
//! APIs; another forge could implement this same trait without touching the
//! merge engine, the origin state or the service layer at all.

use serde::{Deserialize, Serialize};

use crate::error::RemoteError;

/// A resolved origin: which repository, subfolder and branch a domain syncs
/// with. Built from a domain's [`crystalline_core::config::OriginConfig`],
/// with `branch` already defaulted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginSpec {
    /// The repository, `owner/name`.
    pub repo: String,
    /// The subfolder within the repository that is the domain root, or
    /// `None` for the repository root.
    pub subpath: Option<String>,
    /// The branch this domain tracks.
    pub branch: String,
}

/// The result of a conditional check for whether a branch has moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeadProbe {
    /// The branch head has not moved since the last check (a conditional
    /// request answered 304, or the reported head still equals the base).
    Unchanged,
    /// The branch head has moved.
    Changed {
        /// The new head commit sha.
        head: String,
        /// The response ETag, if the provider returned one, to use as the
        /// conditional value on the next check.
        etag: Option<String>,
    },
}

/// How a single file changed between two commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    /// The file is new upstream.
    Added,
    /// The file's content changed upstream.
    Modified,
    /// The file was deleted upstream.
    Removed,
    /// The file was renamed upstream.
    Renamed {
        /// The file's previous repo-relative path.
        previous: String,
    },
}

/// One file changed upstream between two commits. Paths are repo-relative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamChange {
    /// The file's repo-relative path (its current path for a rename).
    pub path: String,
    /// How the file changed.
    pub kind: ChangeKind,
    /// The blob sha of the new content, absent for a removal.
    pub blob_sha: Option<String>,
}

/// The result of comparing two commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareResult {
    /// The files that changed.
    pub files: Vec<UpstreamChange>,
    /// Whether the provider truncated the file list because the comparison
    /// covers too many files to list in full. Callers fall back to a
    /// tarball diff against the base snapshot in that case.
    pub truncated: bool,
}

/// A tree entry for a proposed commit. `blob_sha: None` deletes the path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeWrite {
    /// The repo-relative path being written or deleted.
    pub path: String,
    /// The sha of an already-uploaded blob (see
    /// [`Provider::create_blob`]), or `None` to delete `path`.
    pub blob_sha: Option<String>,
}

/// A request to open a review proposal (a GitHub pull request) for a share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRequest {
    /// The proposal title.
    pub title: String,
    /// The proposal description body.
    pub body: String,
    /// The branch carrying the proposed commits.
    pub branch: String,
    /// The branch the proposal targets.
    pub base_branch: String,
}

/// A reference to a created proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalHandle {
    /// The proposal number.
    pub number: u64,
    /// The web URL a human reviews the proposal at.
    pub url: String,
}

/// One proposal inside a stack, as the forge reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackMember {
    /// The proposal number.
    pub number: u64,
    /// The proposal's state as the forge spells it, for example `"open"`.
    pub state: String,
    /// The commit that member's branch currently points at.
    pub head_sha: String,
}

/// A stack of proposals: an ordered group the forge reviews and merges as a
/// chain, bottom layer first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackInfo {
    /// The stack's own number on the forge.
    pub number: u64,
    /// Whether the stack is still open.
    pub open: bool,
    /// The stack's members, bottom layer first.
    pub members: Vec<StackMember>,
}

/// The lifecycle state of a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProposalState {
    /// Still open for review.
    Open,
    /// Merged into the base branch.
    Merged,
    /// Closed without merging.
    Declined,
}

/// A proposal's review standing plus its flattened feedback items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Feedback {
    /// "approved", "changes_requested" or "commented"; None when no review
    /// exists.
    pub review_state: Option<String>,
    /// Every review body, inline review comment and conversation comment on
    /// the proposal, flattened into one list.
    pub items: Vec<crate::state::FeedbackItem>,
}

/// One open proposal as the forge lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenProposalRef {
    /// The proposal number.
    pub number: u64,
    /// The branch carrying the proposed commits.
    pub branch: String,
    /// The commit that branch currently points at.
    pub head_sha: String,
}

/// Forge-neutral access to the operations Crystalline needs to collaborate
/// over a repository: reading commits, blobs and trees, writing new commits
/// and branches and opening or checking review proposals.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Checks whether `origin`'s branch head has moved, conditional on `etag`
    /// when the provider supports it.
    async fn branch_head(
        &self,
        origin: &OriginSpec,
        etag: Option<&str>,
    ) -> Result<HeadProbe, RemoteError>;

    /// Lists the files that changed between `base` and `head`.
    async fn compare(
        &self,
        origin: &OriginSpec,
        base: &str,
        head: &str,
    ) -> Result<CompareResult, RemoteError>;

    /// Fetches the raw content of a blob by sha.
    async fn blob(&self, origin: &OriginSpec, sha: &str) -> Result<Vec<u8>, RemoteError>;

    /// The tar.gz archive of the repository at a commit, as raw bytes.
    async fn tarball(&self, origin: &OriginSpec, commit: &str) -> Result<Vec<u8>, RemoteError>;

    /// Uploads content as a new blob, returns its sha.
    async fn create_blob(&self, origin: &OriginSpec, content: &[u8])
    -> Result<String, RemoteError>;

    /// Creates a tree on top of `parent_commit`'s tree from the given writes,
    /// returns the new tree sha.
    async fn create_tree(
        &self,
        origin: &OriginSpec,
        parent_commit: &str,
        writes: &[TreeWrite],
    ) -> Result<String, RemoteError>;

    /// Creates a commit with the given tree and parents, returns its sha.
    /// One parent for an ordinary share commit, two for the merge commit a
    /// share-update makes after the base advanced; order matters, the branch
    /// head first.
    async fn create_commit(
        &self,
        origin: &OriginSpec,
        message: &str,
        tree: &str,
        parents: &[String],
    ) -> Result<String, RemoteError>;

    /// Creates a branch pointing at `commit`.
    async fn create_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
    ) -> Result<(), RemoteError>;

    /// Deletes a branch.
    async fn delete_branch(&self, origin: &OriginSpec, name: &str) -> Result<(), RemoteError>;

    /// The commit an arbitrary branch points at, or `None` when the branch
    /// does not exist. Unlike [`Provider::branch_head`] this reads any branch
    /// rather than only the tracked one, and reports a missing branch as an
    /// answer rather than as a repository problem. That widening cuts both
    /// ways: a repository that is gone, renamed or no longer readable with
    /// this token answers `None` too, so `None` means "no branch found here"
    /// and never on its own means "the repository is fine and the branch was
    /// deleted". Callers must not make it the sole basis for a success
    /// verdict.
    async fn branch_ref(
        &self,
        origin: &OriginSpec,
        name: &str,
    ) -> Result<Option<String>, RemoteError>;

    /// Moves an existing branch to `commit`. `force` allows a move that is not
    /// a fast-forward, which a layer branch needs when the layers below it
    /// moved and its commits were rewritten on top of the new base.
    async fn update_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
        force: bool,
    ) -> Result<(), RemoteError>;

    /// Rewrites the parts of an open proposal the caller supplies: its title,
    /// its body and the branch it targets. Every field is optional and an
    /// absent one is left untouched, so a retarget can move the base without
    /// overwriting a body a reviewer is reading.
    async fn update_proposal(
        &self,
        origin: &OriginSpec,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        base: Option<&str>,
    ) -> Result<(), RemoteError>;

    /// Closes a proposal without merging it.
    async fn close_proposal(&self, origin: &OriginSpec, number: u64) -> Result<(), RemoteError>;

    /// The review standing and every piece of feedback on a proposal.
    async fn proposal_feedback(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> Result<Feedback, RemoteError>;

    /// Every proposal still open on the repository, with the branch and head
    /// commit each one carries.
    async fn list_open_proposals(
        &self,
        origin: &OriginSpec,
    ) -> Result<Vec<OpenProposalRef>, RemoteError>;

    /// Opens a review proposal.
    async fn create_proposal(
        &self,
        origin: &OriginSpec,
        req: &ProposalRequest,
    ) -> Result<ProposalHandle, RemoteError>;

    /// Checks the current lifecycle state of a proposal.
    async fn proposal_state(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> Result<ProposalState, RemoteError>;

    /// The authenticated user's login, used to report who is connected.
    async fn current_user(&self) -> Result<String, RemoteError>;

    // The stack verbs below all default to `StacksUnsupported`. Stacking is a
    // forge capability rather than a part of the collaboration contract: a
    // provider that cannot stack proposals implements none of these and its
    // shares keep using a single living proposal.

    /// Every stack on the repository, or just the ones `pull_request` belongs
    /// to when a proposal number is given.
    async fn list_stacks(
        &self,
        origin: &OriginSpec,
        pull_request: Option<u64>,
    ) -> Result<Vec<StackInfo>, RemoteError> {
        let _ = (origin, pull_request);
        Err(RemoteError::StacksUnsupported)
    }

    /// Groups the given proposals into a new stack, bottom layer first.
    async fn create_stack(
        &self,
        origin: &OriginSpec,
        pull_requests: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        let _ = (origin, pull_requests);
        Err(RemoteError::StacksUnsupported)
    }

    /// Adds the given proposals on top of an existing stack.
    async fn extend_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
        pull_requests: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        let _ = (origin, stack_number, pull_requests);
        Err(RemoteError::StacksUnsupported)
    }

    /// Breaks a stack up again, leaving its members as ordinary proposals.
    async fn dissolve_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
    ) -> Result<(), RemoteError> {
        let _ = (origin, stack_number);
        Err(RemoteError::StacksUnsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `Provider` that always fails, used only to prove the trait
    /// is object-safe and usable behind `Box<dyn Provider>` as callers will
    /// need once a real client and a test double exist.
    struct AlwaysOffline;

    #[async_trait::async_trait]
    impl Provider for AlwaysOffline {
        async fn branch_head(
            &self,
            _origin: &OriginSpec,
            _etag: Option<&str>,
        ) -> Result<HeadProbe, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn compare(
            &self,
            _origin: &OriginSpec,
            _base: &str,
            _head: &str,
        ) -> Result<CompareResult, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn blob(&self, _origin: &OriginSpec, _sha: &str) -> Result<Vec<u8>, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn tarball(
            &self,
            _origin: &OriginSpec,
            _commit: &str,
        ) -> Result<Vec<u8>, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn create_blob(
            &self,
            _origin: &OriginSpec,
            _content: &[u8],
        ) -> Result<String, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn create_tree(
            &self,
            _origin: &OriginSpec,
            _parent_commit: &str,
            _writes: &[TreeWrite],
        ) -> Result<String, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn create_commit(
            &self,
            _origin: &OriginSpec,
            _message: &str,
            _tree: &str,
            _parents: &[String],
        ) -> Result<String, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn create_branch(
            &self,
            _origin: &OriginSpec,
            _name: &str,
            _commit: &str,
        ) -> Result<(), RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn delete_branch(
            &self,
            _origin: &OriginSpec,
            _name: &str,
        ) -> Result<(), RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn branch_ref(
            &self,
            _origin: &OriginSpec,
            _name: &str,
        ) -> Result<Option<String>, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn update_branch(
            &self,
            _origin: &OriginSpec,
            _name: &str,
            _commit: &str,
            _force: bool,
        ) -> Result<(), RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn update_proposal(
            &self,
            _origin: &OriginSpec,
            _number: u64,
            _title: Option<&str>,
            _body: Option<&str>,
            _base: Option<&str>,
        ) -> Result<(), RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn close_proposal(
            &self,
            _origin: &OriginSpec,
            _number: u64,
        ) -> Result<(), RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn proposal_feedback(
            &self,
            _origin: &OriginSpec,
            _number: u64,
        ) -> Result<Feedback, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn list_open_proposals(
            &self,
            _origin: &OriginSpec,
        ) -> Result<Vec<OpenProposalRef>, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn create_proposal(
            &self,
            _origin: &OriginSpec,
            _req: &ProposalRequest,
        ) -> Result<ProposalHandle, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn proposal_state(
            &self,
            _origin: &OriginSpec,
            _number: u64,
        ) -> Result<ProposalState, RemoteError> {
            Err(RemoteError::Offline)
        }

        async fn current_user(&self) -> Result<String, RemoteError> {
            Err(RemoteError::Offline)
        }
    }

    fn sample_origin() -> OriginSpec {
        OriginSpec {
            repo: "acme/brand-knowledge".to_string(),
            subpath: Some("knowledge".to_string()),
            branch: "main".to_string(),
        }
    }

    #[test]
    fn provider_is_object_safe_behind_a_trait_object() {
        // This proves object safety at the type level: `dyn Provider`
        // compiles and calling through it returns a boxed future without
        // needing one polled. Polling behavior is exercised by the GitHub
        // client's own tests.
        let provider: Box<dyn Provider> = Box::new(AlwaysOffline);
        let origin = sample_origin();
        let _pending_branch_head = provider.branch_head(&origin, None);
        let _pending_current_user = provider.current_user();
    }

    #[tokio::test]
    async fn stack_verbs_default_to_stacks_unsupported() {
        // AlwaysOffline implements none of the four, so what runs here are the
        // trait's own default bodies.
        let provider = AlwaysOffline;
        let origin = sample_origin();
        assert!(matches!(
            provider.list_stacks(&origin, None).await,
            Err(RemoteError::StacksUnsupported)
        ));
        assert!(matches!(
            provider.create_stack(&origin, &[1, 2]).await,
            Err(RemoteError::StacksUnsupported)
        ));
        assert!(matches!(
            provider.extend_stack(&origin, 3, &[4]).await,
            Err(RemoteError::StacksUnsupported)
        ));
        assert!(matches!(
            provider.dissolve_stack(&origin, 3).await,
            Err(RemoteError::StacksUnsupported)
        ));
    }

    #[test]
    fn stack_info_carries_its_members_bottom_layer_first() {
        let stack = StackInfo {
            number: 9,
            open: true,
            members: vec![
                StackMember {
                    number: 4,
                    state: "open".to_string(),
                    head_sha: "aaa".to_string(),
                },
                StackMember {
                    number: 5,
                    state: "open".to_string(),
                    head_sha: "bbb".to_string(),
                },
            ],
        };
        assert!(stack.open);
        assert_eq!(stack.members[0].number, 4);
        assert_eq!(stack.members[1].head_sha, "bbb");
    }

    #[test]
    fn head_probe_changed_carries_head_and_optional_etag() {
        let probe = HeadProbe::Changed {
            head: "abc123".to_string(),
            etag: Some("W/\"abc\"".to_string()),
        };
        match probe {
            HeadProbe::Changed { head, etag } => {
                assert_eq!(head, "abc123");
                assert_eq!(etag.as_deref(), Some("W/\"abc\""));
            }
            HeadProbe::Unchanged => panic!("expected Changed"),
        }
        assert_eq!(HeadProbe::Unchanged, HeadProbe::Unchanged);
    }

    #[test]
    fn change_kind_renamed_carries_the_previous_path() {
        let change = UpstreamChange {
            path: "notes/new-name.md".to_string(),
            kind: ChangeKind::Renamed {
                previous: "notes/old-name.md".to_string(),
            },
            blob_sha: Some("deadbeef".to_string()),
        };
        match change.kind {
            ChangeKind::Renamed { previous } => assert_eq!(previous, "notes/old-name.md"),
            other => panic!("expected Renamed, got {other:?}"),
        }
    }

    #[test]
    fn tree_write_with_no_blob_sha_means_delete() {
        let delete = TreeWrite {
            path: "notes/retired.md".to_string(),
            blob_sha: None,
        };
        assert_eq!(delete.blob_sha, None);
    }

    #[test]
    fn compare_result_reports_truncation_for_large_diffs() {
        let result = CompareResult {
            files: Vec::new(),
            truncated: true,
        };
        assert!(result.truncated);
        assert!(result.files.is_empty());
    }

    #[test]
    fn proposal_state_variants_are_distinct() {
        assert_ne!(ProposalState::Open, ProposalState::Merged);
        assert_ne!(ProposalState::Merged, ProposalState::Declined);
        assert_eq!(ProposalState::Open, ProposalState::Open);
    }

    #[test]
    fn proposal_handle_carries_number_and_url() {
        let handle = ProposalHandle {
            number: 42,
            url: "https://github.com/acme/brand-knowledge/pull/42".to_string(),
        };
        assert_eq!(handle.number, 42);
        assert!(handle.url.ends_with("/pull/42"));
    }
}
