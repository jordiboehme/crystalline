//! An in-memory forge implementing [`Provider`], a full GitHub stand-in for
//! the lifecycle tests.
//!
//! It models a fake commit graph (commits as maps of repo-relative path to
//! bytes, with parent links), branches as name to commit id, per-branch ETags
//! that bump on every branch move, a compare computed by diffing two commit
//! snapshots, blobs addressed by content hash and tarballs wrapped in the
//! single top-level directory GitHub's tarball endpoint uses. The write side
//! (`create_blob`/`create_tree`/`create_commit`/`create_branch`/
//! `create_proposal`) works for real against the same in-memory graph, so a
//! `propose` call under test produces a genuine new commit a later `pull` can
//! merge in, with every call logged (see [`MockProvider::calls`]). A settable
//! proposal registry and two fault injectors (a garbage-collected base commit
//! and a forced compare truncation) let the tests drive the reconciliation
//! and recovery paths. Nothing here reaches the network and nothing panics on
//! an injected fault.
//!
//! Stacks are modelled too, but only once a test calls
//! [`MockProvider::enable_stacks`]: until then the four stack verbs answer
//! [`RemoteError::StacksUnsupported`], the way a forge without the preview
//! does. The rules come from the 2026-08-27 live spike: a chain runs bottom
//! to top with every member's base branch equal to the previous member's
//! head branch, stack numbers come off the same counter as proposals,
//! closing a member leaves it in the stack, an extend is validated against
//! the current top member (a closed one included) and a dissolve takes the
//! stack out of the registry outright.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::sync::Mutex;

use crystalline_remote::RemoteError;
use crystalline_remote::provider::{
    ChangeKind, CompareResult, Feedback, HeadProbe, OpenProposalRef, OriginSpec, ProposalHandle,
    ProposalRequest, ProposalState, Provider, StackInfo, StackMember, TreeWrite, UpstreamChange,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tar::Header;

/// The lowercase hex SHA-256 digest of `bytes`, matching the encoding the
/// crate under test uses for blob shas and base stamps.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A commit in the fake graph: its full tree and links to its parents (two
/// of them for the merge commit a share-update makes after the base moved).
struct Commit {
    files: BTreeMap<String, Vec<u8>>,
    parents: Vec<String>,
}

/// The message GitHub answers a misaligned chain with, taken verbatim from
/// the spike. Both a bad `POST /stacks` body and an `/add` that does not meet
/// the current top's head ref come back as this 422.
const STACK_CHAIN_MESSAGE: &str =
    "Pull requests must form a stack, where each PR's base ref is the previous PR's head ref";

/// The message GitHub answers a retarget of a stacked proposal with, taken
/// verbatim from the spike.
const STACK_RETARGET_MESSAGE: &str =
    "Cannot change the base branch because the pull request is part of a stack.";

/// The 422 a misaligned chain answers.
fn chain_mismatch() -> RemoteError {
    RemoteError::Api {
        status: 422,
        message: STACK_CHAIN_MESSAGE.to_string(),
    }
}

/// Renders proposal numbers the way the recorded stack calls carry them:
/// `[6,8]`, bottom member first, no spaces.
fn number_list(numbers: &[u64]) -> String {
    let rendered: Vec<String> = numbers.iter().map(u64::to_string).collect();
    format!("[{}]", rendered.join(","))
}

/// One stack in the registry. Only the member order is stored: each member's
/// state and head sha are read live off the proposal and branch registries
/// when a stack is reported, and whether the stack is open follows from its
/// members, so a member closing never has to touch this.
struct StoredStack {
    members: Vec<u64>,
}

#[derive(Default)]
struct Inner {
    commits: HashMap<String, Commit>,
    branches: HashMap<String, String>,
    etags: HashMap<String, String>,
    blobs: HashMap<String, Vec<u8>>,
    proposals: HashMap<u64, ProposalState>,
    /// The request each created proposal was opened with, for tests that
    /// assert on the generated title or body without threading them through
    /// `calls`.
    proposal_requests: HashMap<u64, ProposalRequest>,
    /// The branch each proposal carries, filled by `create_proposal` and by
    /// `register_proposal_branch` for a proposal a test seeded into state
    /// without ever opening it through the provider.
    proposal_branches: HashMap<u64, String>,
    /// The feedback `proposal_feedback` reports per proposal number, set
    /// through `MockProvider::set_feedback`.
    feedback: HashMap<u64, Feedback>,
    /// Proposal numbers whose `proposal_feedback` call fails with a 500.
    feedback_failures: HashSet<u64>,
    /// Proposal numbers whose `close_proposal` call fails with a 500.
    close_failures: HashSet<u64>,
    /// Proposal numbers whose `update_proposal` call fails with a 500, which
    /// is how a test cuts a share-update in half exactly where it hurts:
    /// after the branch already moved.
    update_proposal_failures: HashSet<u64>,
    /// Whether `list_open_proposals` answers [`RemoteError::Offline`].
    open_list_fails: bool,
    /// Whether this forge serves stacks at all. Until
    /// `MockProvider::enable_stacks` flips it, the four stack verbs answer
    /// [`RemoteError::StacksUnsupported`].
    stacks_enabled: bool,
    /// The stack registry, keyed by stack number, bottom member first.
    stacks: BTreeMap<u64, StoredStack>,
    /// How many upcoming `extend_stack` calls answer a 409 before doing any
    /// work, modelling GitHub's concurrent-modification conflict.
    extend_conflicts: u32,
    /// Whether `create_stack` fails outright, for the link-pending path.
    create_stack_fails: bool,
    /// Trees built by `create_tree`, keyed by a generated tree id: the
    /// parent commit's files with every write applied, ready for
    /// `create_commit` to snapshot into a new [`Commit`].
    trees: HashMap<String, BTreeMap<String, Vec<u8>>>,
    gc: HashSet<String>,
    truncate: bool,
    etag_counter: u64,
    commit_counter: u64,
    tree_counter: u64,
    proposal_counter: u64,
    calls: Vec<String>,
}

impl Inner {
    /// The branch proposal `number` carries, as `create_proposal` or
    /// `register_proposal_branch` recorded it.
    fn head_branch(&self, number: u64) -> Option<&str> {
        self.proposal_branches.get(&number).map(String::as_str)
    }

    /// The branch proposal `number` targets, as its opening request or a
    /// later retarget left it.
    fn base_branch(&self, number: u64) -> Option<&str> {
        self.proposal_requests
            .get(&number)
            .map(|req| req.base_branch.as_str())
    }

    /// Checks `numbers` chain bottom to top: each member's base branch is the
    /// previous member's head branch. `previous_head` seeds the check with
    /// the head branch a member must already sit on (the current top's, for
    /// an extend); `None` leaves the bottom member's base unchecked, which is
    /// what `POST /stacks` does - a stack may sit on any branch.
    fn validate_chain(
        &self,
        numbers: &[u64],
        previous_head: Option<&str>,
    ) -> Result<(), RemoteError> {
        let mut expected = previous_head.map(str::to_string);
        for number in numbers {
            if !self.proposals.contains_key(number) {
                return Err(RemoteError::Api {
                    status: 404,
                    message: format!("no proposal {number}"),
                });
            }
            let base = self.base_branch(*number).unwrap_or_default();
            if let Some(want) = expected.as_deref()
                && base != want
            {
                return Err(chain_mismatch());
            }
            expected = self.head_branch(*number).map(str::to_string);
        }
        Ok(())
    }

    /// One stack as the forge reports it: the stored member order, each
    /// member's state and head sha read live, and `open` true while any
    /// member is still open (a stack whose members are all closed or merged
    /// flips shut, exactly as the spike saw).
    fn stack_info(&self, number: u64, stored: &StoredStack) -> StackInfo {
        let members: Vec<StackMember> = stored
            .members
            .iter()
            .map(|member| StackMember {
                number: *member,
                // Unreachable in practice: a member is only ever recorded
                // after `validate_chain` found its proposal.
                state: match self.proposals.get(member) {
                    Some(ProposalState::Open) => "open",
                    Some(ProposalState::Merged) => "merged",
                    Some(ProposalState::Declined) => "closed",
                    None => "unknown",
                }
                .to_string(),
                head_sha: self
                    .head_branch(*member)
                    .and_then(|branch| self.branches.get(branch))
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect();
        StackInfo {
            number,
            open: members.iter().any(|member| member.state == "open"),
            members,
        }
    }
}

/// An in-memory forge implementing [`Provider`] for the lifecycle tests.
pub struct MockProvider {
    inner: Mutex<Inner>,
}

impl Default for MockProvider {
    fn default() -> Self {
        MockProvider::new()
    }
}

impl MockProvider {
    /// A forge with no commits, branches or proposals yet.
    pub fn new() -> Self {
        MockProvider {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Adds a commit built from repo-relative path to content pairs, links it
    /// to `parent` and returns its generated commit id. Every file's content
    /// is registered as a retrievable blob.
    pub fn add_commit(&self, files: BTreeMap<String, Vec<u8>>, parent: Option<&str>) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.commit_counter += 1;
        let id = format!("commit{}", inner.commit_counter);
        for content in files.values() {
            let sha = sha256_hex(content);
            inner.blobs.insert(sha, content.clone());
        }
        inner.commits.insert(
            id.clone(),
            Commit {
                files,
                parents: parent.map(str::to_string).into_iter().collect(),
            },
        );
        id
    }

    /// Points `branch` at `commit`, bumping the branch ETag so the next
    /// conditional probe reports the branch as moved.
    pub fn set_branch(&self, branch: &str, commit: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.etag_counter += 1;
        let etag = format!("etag{}", inner.etag_counter);
        inner
            .branches
            .insert(branch.to_string(), commit.to_string());
        inner.etags.insert(branch.to_string(), etag);
    }

    /// Sets the lifecycle state a later [`Provider::proposal_state`] call will
    /// report for proposal `number`.
    pub fn set_proposal_state(&self, number: u64, state: ProposalState) {
        self.inner.lock().unwrap().proposals.insert(number, state);
    }

    /// The commit `branch` currently points at, or `None` if it was never
    /// set. Lets a test fast-forward `main` onto exactly the commit a
    /// `propose` call created (simulating GitHub merging its pull request
    /// verbatim) without `ProposeReport` itself needing to expose a commit
    /// sha.
    pub fn branch_commit(&self, branch: &str) -> Option<String> {
        self.inner.lock().unwrap().branches.get(branch).cloned()
    }

    /// The request every proposal was opened with, keyed by its number, for
    /// tests asserting on the generated title or body.
    pub fn proposal_request(&self, number: u64) -> Option<ProposalRequest> {
        self.inner
            .lock()
            .unwrap()
            .proposal_requests
            .get(&number)
            .cloned()
    }

    /// The full repo-relative file tree of `commit`, for tests asserting
    /// exactly which paths a `create_tree`/`create_commit` call produced
    /// (repo-relative, subpath prefix included) and that untouched files
    /// carried over from the parent tree.
    pub fn commit_tree(&self, commit: &str) -> Option<BTreeMap<String, Vec<u8>>> {
        self.inner
            .lock()
            .unwrap()
            .commits
            .get(commit)
            .map(|c| c.files.clone())
    }

    /// Marks `commit` as garbage-collected: a [`Provider::compare`] using it
    /// as the base fails with [`RemoteError::RepoNotFound`], the fault that
    /// drives the re-baseline recovery path.
    pub fn gc_commit(&self, commit: &str) {
        self.inner.lock().unwrap().gc.insert(commit.to_string());
    }

    /// Forces every subsequent [`Provider::compare`] to report truncation, so
    /// the pull falls back to a tarball diff.
    pub fn set_truncate(&self, truncate: bool) {
        self.inner.lock().unwrap().truncate = truncate;
    }

    /// The provider calls made so far, for asserting side effects like a
    /// best-effort branch delete.
    pub fn calls(&self) -> Vec<String> {
        self.inner.lock().unwrap().calls.clone()
    }

    /// The parent commits of `commit`, for asserting a merge commit's shape.
    pub fn commit_parents(&self, commit: &str) -> Option<Vec<String>> {
        self.inner
            .lock()
            .unwrap()
            .commits
            .get(commit)
            .map(|c| c.parents.clone())
    }

    /// Sets the review standing and feedback items a later
    /// [`Provider::proposal_feedback`] call reports for `number`.
    pub fn set_feedback(&self, number: u64, feedback: Feedback) {
        self.inner.lock().unwrap().feedback.insert(number, feedback);
    }

    /// Records which branch proposal `number` carries, for a proposal a test
    /// seeded straight into origin state rather than opening through the
    /// provider (so no `create_proposal` request exists to read it from).
    pub fn register_proposal_branch(&self, number: u64, branch: &str) {
        self.inner
            .lock()
            .unwrap()
            .proposal_branches
            .insert(number, branch.to_string());
    }

    /// Makes every subsequent [`Provider::list_open_proposals`] call fail with
    /// [`RemoteError::Offline`].
    pub fn fail_open_proposals(&self) {
        self.inner.lock().unwrap().open_list_fails = true;
    }

    /// Makes [`Provider::close_proposal`] fail with a 500 for `number`.
    pub fn fail_close_proposal(&self, number: u64) {
        self.inner.lock().unwrap().close_failures.insert(number);
    }

    /// Makes [`Provider::proposal_feedback`] fail with a 500 for `number`.
    pub fn fail_feedback(&self, number: u64) {
        self.inner.lock().unwrap().feedback_failures.insert(number);
    }

    /// Makes [`Provider::update_proposal`] fail with a 500 for `number`, so a
    /// share-update lands its `update_branch` and then fails - the one
    /// interruption that leaves the live branch head ahead of what the local
    /// record knows about.
    pub fn fail_update_proposal(&self, number: u64) {
        self.inner
            .lock()
            .unwrap()
            .update_proposal_failures
            .insert(number);
    }

    /// Lets [`Provider::update_proposal`] succeed again for `number`, so a
    /// test can retry the share the injected failure interrupted.
    pub fn heal_update_proposal(&self, number: u64) {
        self.inner
            .lock()
            .unwrap()
            .update_proposal_failures
            .remove(&number);
    }

    /// Turns this forge into one that serves stacks. Until this is called the
    /// four stack verbs answer [`RemoteError::StacksUnsupported`], so a test
    /// gets the fallback forge by default and opts into the preview.
    pub fn enable_stacks(&self) {
        self.inner.lock().unwrap().stacks_enabled = true;
    }

    /// Every stack in the registry, lowest number first.
    pub fn stacks(&self) -> Vec<StackInfo> {
        let inner = self.inner.lock().unwrap();
        inner
            .stacks
            .iter()
            .map(|(number, stored)| inner.stack_info(*number, stored))
            .collect()
    }

    /// One stack by number, or `None` once it was dissolved.
    pub fn stack(&self, number: u64) -> Option<StackInfo> {
        let inner = self.inner.lock().unwrap();
        inner
            .stacks
            .get(&number)
            .map(|stored| inner.stack_info(number, stored))
    }

    /// Makes the next `times` [`Provider::extend_stack`] calls answer a 409,
    /// GitHub's "the stack is being modified" conflict. The injector fires
    /// ahead of any validation and is spent one call at a time. The real
    /// client retries a 409 internally, so a caller driven through this mock
    /// sees a failure rather than a retry.
    pub fn fail_extend_stack_with_conflicts(&self, times: u32) {
        self.inner.lock().unwrap().extend_conflicts = times;
    }

    /// Makes every [`Provider::create_stack`] call fail with a 500, the hard
    /// failure that leaves a fresh layer shared but not linked.
    pub fn fail_create_stack(&self) {
        self.inner.lock().unwrap().create_stack_fails = true;
    }

    /// Lets [`Provider::create_stack`] succeed again, so a test can retry the
    /// link the injected failure lost.
    pub fn heal_create_stack(&self) {
        self.inner.lock().unwrap().create_stack_fails = false;
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn branch_head(
        &self,
        origin: &OriginSpec,
        etag: Option<&str>,
    ) -> Result<HeadProbe, RemoteError> {
        let inner = self.inner.lock().unwrap();
        let commit = inner.branches.get(&origin.branch).cloned().ok_or_else(|| {
            RemoteError::RepoNotFound {
                repo: origin.repo.clone(),
            }
        })?;
        let current = inner.etags.get(&origin.branch).cloned();
        if etag.is_some() && etag == current.as_deref() {
            Ok(HeadProbe::Unchanged)
        } else {
            Ok(HeadProbe::Changed {
                head: commit,
                etag: current,
            })
        }
    }

    async fn compare(
        &self,
        origin: &OriginSpec,
        base: &str,
        head: &str,
    ) -> Result<CompareResult, RemoteError> {
        let inner = self.inner.lock().unwrap();
        if inner.gc.contains(base) {
            return Err(RemoteError::RepoNotFound {
                repo: origin.repo.clone(),
            });
        }
        let base_files = &inner
            .commits
            .get(base)
            .ok_or_else(|| RemoteError::RepoNotFound {
                repo: origin.repo.clone(),
            })?
            .files;
        let head_files = &inner
            .commits
            .get(head)
            .ok_or_else(|| RemoteError::RepoNotFound {
                repo: origin.repo.clone(),
            })?
            .files;

        let mut files = Vec::new();
        for (path, content) in head_files {
            match base_files.get(path) {
                None => files.push(UpstreamChange {
                    path: path.clone(),
                    kind: ChangeKind::Added,
                    blob_sha: Some(sha256_hex(content)),
                }),
                Some(old) if old != content => files.push(UpstreamChange {
                    path: path.clone(),
                    kind: ChangeKind::Modified,
                    blob_sha: Some(sha256_hex(content)),
                }),
                Some(_) => {}
            }
        }
        for path in base_files.keys() {
            if !head_files.contains_key(path) {
                files.push(UpstreamChange {
                    path: path.clone(),
                    kind: ChangeKind::Removed,
                    blob_sha: None,
                });
            }
        }
        Ok(CompareResult {
            files,
            truncated: inner.truncate,
        })
    }

    async fn blob(&self, _origin: &OriginSpec, sha: &str) -> Result<Vec<u8>, RemoteError> {
        let inner = self.inner.lock().unwrap();
        inner
            .blobs
            .get(sha)
            .cloned()
            .ok_or_else(|| RemoteError::Api {
                status: 404,
                message: format!("no blob {sha}"),
            })
    }

    async fn tarball(&self, origin: &OriginSpec, commit: &str) -> Result<Vec<u8>, RemoteError> {
        let inner = self.inner.lock().unwrap();
        let c = inner
            .commits
            .get(commit)
            .ok_or_else(|| RemoteError::RepoNotFound {
                repo: origin.repo.clone(),
            })?;
        let top = format!("{}-{}", origin.repo.replace('/', "-"), commit);
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in &c.files {
            let mut header = Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{top}/{path}"), content.as_slice())
                .unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        Ok(encoder.finish().unwrap())
    }

    async fn create_blob(
        &self,
        _origin: &OriginSpec,
        content: &[u8],
    ) -> Result<String, RemoteError> {
        let sha = sha256_hex(content);
        let mut inner = self.inner.lock().unwrap();
        inner.blobs.insert(sha.clone(), content.to_vec());
        inner.calls.push(format!("create_blob:{sha}"));
        Ok(sha)
    }

    async fn create_tree(
        &self,
        origin: &OriginSpec,
        parent_commit: &str,
        writes: &[TreeWrite],
    ) -> Result<String, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        let mut files = inner
            .commits
            .get(parent_commit)
            .ok_or_else(|| RemoteError::RepoNotFound {
                repo: origin.repo.clone(),
            })?
            .files
            .clone();
        for write in writes {
            match &write.blob_sha {
                Some(sha) => {
                    let content =
                        inner
                            .blobs
                            .get(sha)
                            .cloned()
                            .ok_or_else(|| RemoteError::Api {
                                status: 404,
                                message: format!("no blob {sha}"),
                            })?;
                    files.insert(write.path.clone(), content);
                }
                None => {
                    files.remove(&write.path);
                }
            }
        }
        inner.tree_counter += 1;
        let id = format!("tree{}", inner.tree_counter);
        inner.trees.insert(id.clone(), files);
        inner.calls.push(format!("create_tree:{id}"));
        Ok(id)
    }

    async fn create_commit(
        &self,
        origin: &OriginSpec,
        _message: &str,
        tree: &str,
        parents: &[String],
    ) -> Result<String, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        let files = inner
            .trees
            .get(tree)
            .cloned()
            .ok_or_else(|| RemoteError::RepoNotFound {
                repo: origin.repo.clone(),
            })?;
        inner.commit_counter += 1;
        let id = format!("commit{}", inner.commit_counter);
        inner.commits.insert(
            id.clone(),
            Commit {
                files,
                parents: parents.to_vec(),
            },
        );
        inner.calls.push(format!("create_commit:{id}"));
        Ok(id)
    }

    async fn create_branch(
        &self,
        _origin: &OriginSpec,
        name: &str,
        commit: &str,
    ) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.etag_counter += 1;
        let etag = format!("etag{}", inner.etag_counter);
        inner.branches.insert(name.to_string(), commit.to_string());
        inner.etags.insert(name.to_string(), etag);
        inner.calls.push(format!("create_branch:{name}:{commit}"));
        Ok(())
    }

    async fn delete_branch(&self, _origin: &OriginSpec, name: &str) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.branches.remove(name);
        inner.etags.remove(name);
        inner.calls.push(format!("delete_branch:{name}"));
        Ok(())
    }

    async fn branch_ref(
        &self,
        _origin: &OriginSpec,
        name: &str,
    ) -> Result<Option<String>, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push(format!("branch_ref:{name}"));
        Ok(inner.branches.get(name).cloned())
    }

    async fn update_branch(
        &self,
        _origin: &OriginSpec,
        name: &str,
        commit: &str,
        force: bool,
    ) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.etag_counter += 1;
        let etag = format!("etag{}", inner.etag_counter);
        inner.branches.insert(name.to_string(), commit.to_string());
        inner.etags.insert(name.to_string(), etag);
        // The recorded string extends the old `update_branch:{name}:{commit}`
        // with the force flag rather than rewording it, so the `starts_with`
        // assertions across lifecycle.rs keep matching.
        inner
            .calls
            .push(format!("update_branch:{name}:{commit}:force={force}"));
        Ok(())
    }

    async fn update_proposal(
        &self,
        _origin: &OriginSpec,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        base: Option<&str>,
    ) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        // Logged before the failure check, like `close_proposal`, so an
        // injected failure still leaves a trace of the attempt.
        inner.calls.push(format!("update_proposal:{number}"));
        if let Some(base) = base {
            inner
                .calls
                .push(format!("update_proposal_base:{number}:{base}"));
        }
        if inner.update_proposal_failures.contains(&number) {
            return Err(RemoteError::Api {
                status: 500,
                message: format!("injected update failure for {number}"),
            });
        }
        // A stacked proposal cannot be retargeted: the repair order is
        // unstack first, then retarget. Title and body edits are unaffected.
        if base.is_some()
            && inner
                .stacks
                .values()
                .any(|stack| stack.members.contains(&number))
        {
            return Err(RemoteError::Api {
                status: 422,
                message: STACK_RETARGET_MESSAGE.to_string(),
            });
        }
        if let Some(req) = inner.proposal_requests.get_mut(&number) {
            // Only what the caller supplied is applied: a retarget-only call
            // leaves the title and body exactly as they stand.
            if let Some(title) = title {
                req.title = title.to_string();
            }
            if let Some(body) = body {
                req.body = body.to_string();
            }
            if let Some(base) = base {
                req.base_branch = base.to_string();
            }
        }
        Ok(())
    }

    async fn close_proposal(&self, _origin: &OriginSpec, number: u64) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        // Logged before the failure check, like every other call here, so an
        // injected failure still leaves a trace of the attempt.
        inner.calls.push(format!("close_proposal:{number}"));
        if inner.close_failures.contains(&number) {
            return Err(RemoteError::Api {
                status: 500,
                message: format!("injected close failure for {number}"),
            });
        }
        inner.proposals.insert(number, ProposalState::Declined);
        Ok(())
    }

    async fn proposal_feedback(
        &self,
        _origin: &OriginSpec,
        number: u64,
    ) -> Result<Feedback, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push(format!("proposal_feedback:{number}"));
        if inner.feedback_failures.contains(&number) {
            return Err(RemoteError::Api {
                status: 500,
                message: format!("injected feedback failure for {number}"),
            });
        }
        Ok(inner.feedback.get(&number).cloned().unwrap_or_default())
    }

    async fn list_open_proposals(
        &self,
        _origin: &OriginSpec,
    ) -> Result<Vec<OpenProposalRef>, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push("list_open_proposals".to_string());
        if inner.open_list_fails {
            return Err(RemoteError::Offline);
        }
        let mut out = Vec::new();
        for (number, state) in &inner.proposals {
            if *state != ProposalState::Open {
                continue;
            }
            let branch = inner
                .proposal_branches
                .get(number)
                .cloned()
                .unwrap_or_default();
            let head_sha = inner.branches.get(&branch).cloned().unwrap_or_default();
            out.push(OpenProposalRef {
                number: *number,
                branch,
                head_sha,
            });
        }
        // `proposals` is a HashMap, so sort before returning: tests assert on
        // the order the caller then works through these in.
        out.sort_by_key(|p| p.number);
        Ok(out)
    }

    async fn create_proposal(
        &self,
        _origin: &OriginSpec,
        req: &ProposalRequest,
    ) -> Result<ProposalHandle, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.proposal_counter += 1;
        let number = inner.proposal_counter;
        inner.proposals.insert(number, ProposalState::Open);
        inner.proposal_requests.insert(number, req.clone());
        inner.proposal_branches.insert(number, req.branch.clone());
        inner.calls.push(format!("create_proposal:{}", req.branch));
        Ok(ProposalHandle {
            number,
            url: format!("https://github.test/pulls/{number}"),
        })
    }

    async fn proposal_state(
        &self,
        _origin: &OriginSpec,
        number: u64,
    ) -> Result<ProposalState, RemoteError> {
        self.inner
            .lock()
            .unwrap()
            .proposals
            .get(&number)
            .copied()
            .ok_or_else(|| RemoteError::Api {
                status: 404,
                message: format!("no proposal {number}"),
            })
    }

    async fn list_stacks(
        &self,
        _origin: &OriginSpec,
        pull_request: Option<u64>,
    ) -> Result<Vec<StackInfo>, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        match pull_request {
            Some(number) => inner.calls.push(format!("list_stacks:{number}")),
            None => inner.calls.push("list_stacks".to_string()),
        }
        if !inner.stacks_enabled {
            return Err(RemoteError::StacksUnsupported);
        }
        Ok(inner
            .stacks
            .iter()
            .filter(|(_, stored)| match pull_request {
                Some(number) => stored.members.contains(&number),
                None => true,
            })
            .map(|(number, stored)| inner.stack_info(*number, stored))
            .collect())
    }

    async fn create_stack(
        &self,
        _origin: &OriginSpec,
        numbers: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .calls
            .push(format!("create_stack:{}", number_list(numbers)));
        if !inner.stacks_enabled {
            return Err(RemoteError::StacksUnsupported);
        }
        if inner.create_stack_fails {
            return Err(RemoteError::Api {
                status: 500,
                message: "injected create_stack failure".to_string(),
            });
        }
        inner.validate_chain(numbers, None)?;
        // Stack numbers come off the issue and pull-request sequence, so a
        // stack takes the next number the proposals would have taken.
        inner.proposal_counter += 1;
        let number = inner.proposal_counter;
        inner.stacks.insert(
            number,
            StoredStack {
                members: numbers.to_vec(),
            },
        );
        let stored = &inner.stacks[&number];
        Ok(inner.stack_info(number, stored))
    }

    async fn extend_stack(
        &self,
        _origin: &OriginSpec,
        stack: u64,
        numbers: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .calls
            .push(format!("extend_stack:{stack}:{}", number_list(numbers)));
        if !inner.stacks_enabled {
            return Err(RemoteError::StacksUnsupported);
        }
        if inner.extend_conflicts > 0 {
            inner.extend_conflicts -= 1;
            return Err(RemoteError::Api {
                status: 409,
                message: "Stack is being modified".to_string(),
            });
        }
        let top = *inner
            .stacks
            .get(&stack)
            .and_then(|stored| stored.members.last())
            .ok_or_else(|| RemoteError::Api {
                status: 404,
                message: format!("no stack {stack}"),
            })?;
        // A closed top blocks the extend, the way the spike saw it: GitHub
        // answers the same base-ref 422 even for a layer branched off that
        // closed member's head.
        if inner.proposals.get(&top) != Some(&ProposalState::Open) {
            return Err(chain_mismatch());
        }
        let top_head = inner.head_branch(top).map(str::to_string);
        inner.validate_chain(numbers, top_head.as_deref())?;
        if let Some(stored) = inner.stacks.get_mut(&stack) {
            stored.members.extend_from_slice(numbers);
        }
        let stored = &inner.stacks[&stack];
        Ok(inner.stack_info(stack, stored))
    }

    async fn dissolve_stack(&self, _origin: &OriginSpec, stack: u64) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push(format!("dissolve_stack:{stack}"));
        if !inner.stacks_enabled {
            return Err(RemoteError::StacksUnsupported);
        }
        // Dissolving takes the stack out of the registry entirely; the forge
        // 404s on it afterwards. The member proposals are untouched.
        inner
            .stacks
            .remove(&stack)
            .map(|_| ())
            .ok_or_else(|| RemoteError::Api {
                status: 404,
                message: format!("no stack {stack}"),
            })
    }

    async fn current_user(&self) -> Result<String, RemoteError> {
        Ok("mock-user".to_string())
    }
}
