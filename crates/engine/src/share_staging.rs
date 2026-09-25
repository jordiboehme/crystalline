//! The tree one share of a domain that reviews changes is detected against, and
//! the one provider screen that keeps a share from merging into it.
//!
//! A share of a reviewing domain cannot be a walk of the folder on disk. Nothing
//! a member writes is there - every write joins that member's own draft overlay
//! and the folder goes on saying what the team reviewed - so a walk would find
//! nothing, or find somebody's stray direct edit and propose that instead. So
//! the engine builds the tree the share really is about, out of the base
//! snapshot and the acting actor's own rows, and hands THAT down as the domain
//! root. That is what keeps `crystalline_remote` identity-unaware: it is handed
//! a root and detects changes the one way it always has, and whose root it is
//! was decided here.
//!
//! Two pieces live here, and the second exists because of the first.
//! [`crate::domain_view::DomainView::materialise`] stages the tree.
//! [`PinnedHead`] wraps the provider for the one call
//! that runs against that tree: `ops::propose` and `ops::propose_preview` open
//! with a pull of their own, and a pull into the staged tree would write the
//! team's merged work into a folder that is deleted moments later while the base
//! snapshot advanced past it - leaving the real folder permanently behind its
//! own base, with no later pull that would bring it back (`ops::pull`
//! short-circuits once `head == base_commit` and writes no file). The engine
//! pulls the REAL folder before it stages, so that pull has nothing left to do;
//! [`PinnedHead`] is what makes "nothing left to do" true rather than likely.
//!
//! Everything here is filesystem work over rows and paths somebody else read.
//! The verb itself - the gates, the credential, the lock, the ordering argument
//! - is `Engine::origin_share` in [`crate::engine`].

use std::path::{Path, PathBuf};

use crystalline_remote::RemoteError;
use crystalline_remote::provider::{
    CompareResult, Feedback, HeadProbe, OpenProposalRef, OriginSpec, ProposalHandle,
    ProposalRequest, ProposalState, Provider, StackInfo, TreeWrite,
};

use crate::engine::{
    EngineError, OVERLAY_NEEDS_IDENTITY, OWNER_IDENTITY_NAME, Result as EngineResult, ShareActor,
    join_rel,
};

/// The folder under a domain's origin state directory that one share of a
/// reviewing domain stages its tree in.
pub(crate) const OVERLAY_STAGING_DIR: &str = "share-staging";

/// What a share is told when the team's copy moved between the pull it opened
/// with and the proposal it was about to make.
///
/// A retry is the whole answer: the next share pulls the move into the folder
/// first and then carries the draft onto what the team has now. Nothing is lost
/// and nothing has to be resolved, so the message says to run it again rather
/// than describing machinery the reader cannot act on.
pub(crate) const SHARE_HEAD_MOVED: &str = "the team's copy moved while this share was prepared - run it again, and the share will carry \
     your draft onto what the team has now";

/// The actor key a share acts in on a domain that reviews changes before they
/// land.
///
/// The one mapping from a [`ShareActor`] - which is what every surface resolves
/// a sharer into - to the key the overlay rows are written under, and it says
/// the same three things [`crate::scope::overlay_actor`] says on the write side:
/// the machine owner drafts as [`OWNER_IDENTITY_NAME`], an account drafts under
/// its own login, and an agent over HTTP that never authenticated is nobody in
/// particular. The two have to agree or the owner's CLI would share an overlay
/// nothing ever writes into, which is why they are written as one answer each
/// rather than derived from one another.
///
/// Nobody in particular is [`OVERLAY_NEEDS_IDENTITY`] here for the same reason
/// it is a refusal there: with no identity there is no draft for the share to
/// be of, and the one thing that must never happen instead is a share of the
/// whole folder going out under nobody's name.
pub(crate) fn overlay_share_actor(actor: &ShareActor) -> EngineResult<String> {
    match actor {
        ShareActor::Owner => Ok(OWNER_IDENTITY_NAME.to_string()),
        ShareActor::Account(account) => Ok(account.clone()),
        ShareActor::HttpAgent => Err(EngineError::Refused(OVERLAY_NEEDS_IDENTITY.to_string())),
    }
}

/// One share's staged tree and the commit the team's folder stood at when it was
/// built.
///
/// The two travel together because the second is only meaningful for the first:
/// the tree was staged over that base, so that is the head
/// [`PinnedHead`] holds the inline pull to. Separating them would let a caller
/// pin a share to a commit its tree was not staged over, which is the one
/// mistake this pair exists to make unavailable.
pub(crate) struct PreparedShare {
    /// The tree to detect against, and its lifetime.
    pub(crate) staging: OverlayStaging,
    /// The commit the folder stands at, for [`PinnedHead::new`].
    pub(crate) pinned: String,
}

/// A staged tree, owned by the share that built it.
///
/// Removed on every exit path, a refusal and a failed forge call included, by
/// [`Drop`]: a staged tree is a copy of somebody's unreviewed knowledge, and one
/// left behind would sit under the state directory until something happened to
/// overwrite it.
pub(crate) struct OverlayStaging {
    root: PathBuf,
}

impl OverlayStaging {
    /// A staged tree rooted at `root`, owned from here on by whoever holds it:
    /// [`crate::domain_view::DomainView::materialise`] is what stages one.
    pub(crate) fn at(root: PathBuf) -> OverlayStaging {
        OverlayStaging { root }
    }

    /// The staged tree's root, to hand to `crystalline_remote::ops` in place of
    /// the domain's folder.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for OverlayStaging {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                "the staged share tree at {} could not be removed: {e}",
                self.root.display()
            ),
        }
    }
}

/// Writes `bytes` at `rel` under `root`, creating the folders on the way.
///
/// `rel` is a domain-relative forward-slashed path whose containment the caller
/// has already asserted; [`join_rel`] puts it together a segment at a time, so a
/// separator that means something else on another platform cannot re-root the
/// result.
pub(crate) fn write_staged_file(root: &Path, rel: &str, bytes: &[u8]) -> EngineResult<()> {
    let path = join_rel(root, rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| EngineError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(&path, bytes).map_err(|source| EngineError::Io {
        path: path.display().to_string(),
        source,
    })
}

/// The provider a share of a reviewing domain hands to `crystalline_remote`:
/// every call is the real one, except the branch-head probe the inline pull
/// opens with.
///
/// The engine has already pulled the team's folder to `pinned`, so that pull has
/// nothing left to apply - and it must not apply anything, because the root it
/// would apply it to is the staged tree. So the probe answers `Unchanged` while
/// upstream still stands where the engine left it, which is the short-circuit
/// [`crystalline_remote::ops::pull`] takes before it writes a single file, and
/// refuses the share outright when it has moved. Refusing is the honest answer
/// rather than the cautious one: the alternative is a merge landing in a folder
/// that is about to be deleted, and the retry the message asks for costs one
/// more pull and loses nothing.
///
/// Only the probe is screened. Every other call - the blobs, the trees, the
/// branch and the proposal - is the share doing its real work against the real
/// forge.
pub(crate) struct PinnedHead<'a> {
    inner: &'a dyn Provider,
    pinned: String,
}

impl<'a> PinnedHead<'a> {
    /// Wraps `inner` for a share whose folder has just been pulled to `pinned`.
    pub(crate) fn new(inner: &'a dyn Provider, pinned: String) -> PinnedHead<'a> {
        PinnedHead { inner, pinned }
    }
}

#[async_trait::async_trait]
impl Provider for PinnedHead<'_> {
    async fn branch_head(
        &self,
        origin: &OriginSpec,
        _etag: Option<&str>,
    ) -> Result<HeadProbe, RemoteError> {
        // Probed unconditionally, ignoring the caller's stored etag on purpose.
        // A conditional probe answers "unchanged since the etag you gave me",
        // which is only the same question as "still at the commit this tree was
        // staged over" while the stored etag is known to track the base commit.
        // It does track it today, but that invariant is kept in
        // `crystalline_remote` and this screen is the one thing that must not
        // depend on it: the cost of asking outright is one non-conditional
        // request per share, and the cost of being wrong is a merge landing in a
        // folder that is about to be deleted.
        match self.inner.branch_head(origin, None).await? {
            HeadProbe::Unchanged => Ok(HeadProbe::Unchanged),
            // Still where the engine's own pull left it, so the inline pull has
            // nothing to do: this is the short-circuit it takes before it writes
            // a single file.
            HeadProbe::Changed { head, .. } if head == self.pinned => Ok(HeadProbe::Unchanged),
            HeadProbe::Changed { .. } => Err(RemoteError::Refused(SHARE_HEAD_MOVED.to_string())),
        }
    }

    async fn compare(
        &self,
        origin: &OriginSpec,
        base: &str,
        head: &str,
    ) -> Result<CompareResult, RemoteError> {
        self.inner.compare(origin, base, head).await
    }

    async fn blob(&self, origin: &OriginSpec, sha: &str) -> Result<Vec<u8>, RemoteError> {
        self.inner.blob(origin, sha).await
    }

    async fn tarball(&self, origin: &OriginSpec, commit: &str) -> Result<Vec<u8>, RemoteError> {
        self.inner.tarball(origin, commit).await
    }

    async fn create_blob(
        &self,
        origin: &OriginSpec,
        content: &[u8],
    ) -> Result<String, RemoteError> {
        self.inner.create_blob(origin, content).await
    }

    async fn create_tree(
        &self,
        origin: &OriginSpec,
        parent_commit: &str,
        writes: &[TreeWrite],
    ) -> Result<String, RemoteError> {
        self.inner.create_tree(origin, parent_commit, writes).await
    }

    async fn create_commit(
        &self,
        origin: &OriginSpec,
        message: &str,
        tree: &str,
        parents: &[String],
    ) -> Result<String, RemoteError> {
        self.inner
            .create_commit(origin, message, tree, parents)
            .await
    }

    async fn create_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
    ) -> Result<(), RemoteError> {
        self.inner.create_branch(origin, name, commit).await
    }

    async fn delete_branch(&self, origin: &OriginSpec, name: &str) -> Result<(), RemoteError> {
        self.inner.delete_branch(origin, name).await
    }

    async fn branch_ref(
        &self,
        origin: &OriginSpec,
        name: &str,
    ) -> Result<Option<String>, RemoteError> {
        self.inner.branch_ref(origin, name).await
    }

    async fn update_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
        force: bool,
    ) -> Result<(), RemoteError> {
        self.inner.update_branch(origin, name, commit, force).await
    }

    fn commit_url(&self, origin: &OriginSpec, sha: &str) -> Option<String> {
        self.inner.commit_url(origin, sha)
    }

    async fn update_proposal(
        &self,
        origin: &OriginSpec,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        base: Option<&str>,
    ) -> Result<(), RemoteError> {
        self.inner
            .update_proposal(origin, number, title, body, base)
            .await
    }

    async fn close_proposal(&self, origin: &OriginSpec, number: u64) -> Result<(), RemoteError> {
        self.inner.close_proposal(origin, number).await
    }

    async fn proposal_feedback(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> Result<Feedback, RemoteError> {
        self.inner.proposal_feedback(origin, number).await
    }

    async fn list_open_proposals(
        &self,
        origin: &OriginSpec,
    ) -> Result<Vec<OpenProposalRef>, RemoteError> {
        self.inner.list_open_proposals(origin).await
    }

    async fn create_proposal(
        &self,
        origin: &OriginSpec,
        req: &ProposalRequest,
    ) -> Result<ProposalHandle, RemoteError> {
        self.inner.create_proposal(origin, req).await
    }

    async fn proposal_state(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> Result<ProposalState, RemoteError> {
        self.inner.proposal_state(origin, number).await
    }

    async fn current_user(&self) -> Result<String, RemoteError> {
        self.inner.current_user().await
    }

    async fn default_branch(&self, repo: &str) -> Result<String, RemoteError> {
        self.inner.default_branch(repo).await
    }

    // The stack verbs are delegated rather than left to the trait's
    // `StacksUnsupported` defaults: a wrapper that answered those defaults would
    // turn a stacking forge into a non-stacking one for exactly the shares that
    // go through it.
    async fn list_stacks(
        &self,
        origin: &OriginSpec,
        pull_request: Option<u64>,
    ) -> Result<Vec<StackInfo>, RemoteError> {
        self.inner.list_stacks(origin, pull_request).await
    }

    async fn create_stack(
        &self,
        origin: &OriginSpec,
        pull_requests: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        self.inner.create_stack(origin, pull_requests).await
    }

    async fn extend_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
        pull_requests: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        self.inner
            .extend_stack(origin, stack_number, pull_requests)
            .await
    }

    async fn dissolve_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
    ) -> Result<(), RemoteError> {
        self.inner.dissolve_stack(origin, stack_number).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A forge that answers only the default-branch question, so a wrapper
    /// that answered it without asking would be caught returning anything
    /// but `trunk`.
    struct TrunkForge;

    #[async_trait::async_trait]
    impl Provider for TrunkForge {
        async fn branch_head(
            &self,
            _: &OriginSpec,
            _: Option<&str>,
        ) -> Result<HeadProbe, RemoteError> {
            unreachable!()
        }
        async fn compare(
            &self,
            _: &OriginSpec,
            _: &str,
            _: &str,
        ) -> Result<CompareResult, RemoteError> {
            unreachable!()
        }
        async fn blob(&self, _: &OriginSpec, _: &str) -> Result<Vec<u8>, RemoteError> {
            unreachable!()
        }
        async fn tarball(&self, _: &OriginSpec, _: &str) -> Result<Vec<u8>, RemoteError> {
            unreachable!()
        }
        async fn create_blob(&self, _: &OriginSpec, _: &[u8]) -> Result<String, RemoteError> {
            unreachable!()
        }
        async fn create_tree(
            &self,
            _: &OriginSpec,
            _: &str,
            _: &[TreeWrite],
        ) -> Result<String, RemoteError> {
            unreachable!()
        }
        async fn create_commit(
            &self,
            _: &OriginSpec,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<String, RemoteError> {
            unreachable!()
        }
        async fn create_branch(&self, _: &OriginSpec, _: &str, _: &str) -> Result<(), RemoteError> {
            unreachable!()
        }
        async fn delete_branch(&self, _: &OriginSpec, _: &str) -> Result<(), RemoteError> {
            unreachable!()
        }
        async fn branch_ref(&self, _: &OriginSpec, _: &str) -> Result<Option<String>, RemoteError> {
            unreachable!()
        }
        async fn update_branch(
            &self,
            _: &OriginSpec,
            _: &str,
            _: &str,
            _: bool,
        ) -> Result<(), RemoteError> {
            unreachable!()
        }
        async fn update_proposal(
            &self,
            _: &OriginSpec,
            _: u64,
            _: Option<&str>,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<(), RemoteError> {
            unreachable!()
        }
        async fn close_proposal(&self, _: &OriginSpec, _: u64) -> Result<(), RemoteError> {
            unreachable!()
        }
        async fn proposal_feedback(&self, _: &OriginSpec, _: u64) -> Result<Feedback, RemoteError> {
            unreachable!()
        }
        async fn list_open_proposals(
            &self,
            _: &OriginSpec,
        ) -> Result<Vec<OpenProposalRef>, RemoteError> {
            unreachable!()
        }
        async fn create_proposal(
            &self,
            _: &OriginSpec,
            _: &ProposalRequest,
        ) -> Result<ProposalHandle, RemoteError> {
            unreachable!()
        }
        async fn proposal_state(
            &self,
            _: &OriginSpec,
            _: u64,
        ) -> Result<ProposalState, RemoteError> {
            unreachable!()
        }
        async fn current_user(&self) -> Result<String, RemoteError> {
            unreachable!()
        }
        async fn default_branch(&self, repo: &str) -> Result<String, RemoteError> {
            assert_eq!(repo, "acme/kb");
            Ok("trunk".to_string())
        }
    }

    #[tokio::test]
    async fn pinned_head_asks_the_real_forge_for_the_default_branch() {
        let pinned = PinnedHead::new(&TrunkForge, "commit1".to_string());
        assert_eq!(pinned.default_branch("acme/kb").await.unwrap(), "trunk");
    }
}
