//! A minimal in-memory forge implementing `crystalline_remote::Provider`, for
//! the engine-level origin tests in `tests/origin.rs`.
//!
//! Lifted from `crystalline_remote`'s own `tests/mock/mod.rs` (a test-only
//! module of that crate, not reachable from here) and trimmed to what the
//! origin engine methods exercise: a single branch's commit history, tarball
//! download for `subscribe`, a diff-based compare for `pull` and a
//! conditional branch probe for `status`. Production code never depends on
//! this; it exists only under `tests/`.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::sync::Mutex;

use crystalline_remote::RemoteError;
use crystalline_remote::provider::{
    ChangeKind, CompareResult, HeadProbe, OriginSpec, ProposalHandle, ProposalRequest,
    ProposalState, Provider, TreeWrite, UpstreamChange,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tar::Header;

/// The lowercase hex SHA-256 digest of `bytes`.
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

/// A commit in the fake graph: its full tree.
struct Commit {
    files: BTreeMap<String, Vec<u8>>,
}

#[derive(Default)]
struct Inner {
    commits: HashMap<String, Commit>,
    branches: HashMap<String, String>,
    etags: HashMap<String, String>,
    blobs: HashMap<String, Vec<u8>>,
    etag_counter: u64,
    commit_counter: u64,
    current_user: String,
    /// Branches whose `branch_head` probe should fail with
    /// `RemoteError::Offline`, simulating a live network outage. Set through
    /// `MockProvider::fail_branch_head_offline`.
    offline_branches: HashSet<String>,
    /// The lifecycle state `proposal_state` reports for a given proposal
    /// number, set through `MockProvider::set_proposal_state`. A number with
    /// no entry here errors as unknown, matching a genuinely nonexistent
    /// proposal.
    proposal_states: HashMap<u64, ProposalState>,
}

/// An in-memory forge implementing [`Provider`] for the origin engine tests.
pub struct MockProvider {
    inner: Mutex<Inner>,
}

impl Default for MockProvider {
    fn default() -> Self {
        MockProvider::new()
    }
}

impl MockProvider {
    /// A forge with no commits or branches yet, reporting `mock-user` as the
    /// signed-in login.
    pub fn new() -> Self {
        MockProvider {
            inner: Mutex::new(Inner {
                current_user: "mock-user".to_string(),
                ..Inner::default()
            }),
        }
    }

    /// Adds a commit built from repo-relative path to content pairs and
    /// returns its generated commit id. Every file's content is registered as
    /// a retrievable blob.
    pub fn add_commit(&self, files: BTreeMap<String, Vec<u8>>) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.commit_counter += 1;
        let id = format!("commit{}", inner.commit_counter);
        for content in files.values() {
            let sha = sha256_hex(content);
            inner.blobs.insert(sha, content.clone());
        }
        inner.commits.insert(id.clone(), Commit { files });
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

    /// Marks `branch` as unreachable: every subsequent `branch_head` probe
    /// against it returns `Err(RemoteError::Offline)`, simulating a live
    /// network outage while a saved GitHub connection still exists (as
    /// opposed to no connection at all, which the engine already handles by
    /// never resolving a provider in the first place).
    pub fn fail_branch_head_offline(&self, branch: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.offline_branches.insert(branch.to_string());
    }

    /// Sets the lifecycle state `proposal_state` reports for `number`, so a
    /// `pull`'s open-proposal refresh can observe a proposal moving to
    /// merged or declined.
    pub fn set_proposal_state(&self, number: u64, state: ProposalState) {
        let mut inner = self.inner.lock().unwrap();
        inner.proposal_states.insert(number, state);
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
        if inner.offline_branches.contains(&origin.branch) {
            return Err(RemoteError::Offline);
        }
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
            truncated: false,
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
        _content: &[u8],
    ) -> Result<String, RemoteError> {
        unsupported_write("create_blob")
    }

    async fn create_tree(
        &self,
        _origin: &OriginSpec,
        _parent_commit: &str,
        _writes: &[TreeWrite],
    ) -> Result<String, RemoteError> {
        unsupported_write("create_tree")
    }

    async fn create_commit(
        &self,
        _origin: &OriginSpec,
        _message: &str,
        _tree: &str,
        _parent: &str,
    ) -> Result<String, RemoteError> {
        unsupported_write("create_commit")
    }

    async fn create_branch(
        &self,
        _origin: &OriginSpec,
        _name: &str,
        _commit: &str,
    ) -> Result<(), RemoteError> {
        unsupported_write("create_branch").map(|_: String| ())
    }

    async fn delete_branch(&self, _origin: &OriginSpec, _name: &str) -> Result<(), RemoteError> {
        Ok(())
    }

    async fn create_proposal(
        &self,
        _origin: &OriginSpec,
        _req: &ProposalRequest,
    ) -> Result<ProposalHandle, RemoteError> {
        Err(RemoteError::Api {
            status: 0,
            message: "create_proposal is not supported by the mock".to_string(),
        })
    }

    async fn proposal_state(
        &self,
        _origin: &OriginSpec,
        number: u64,
    ) -> Result<ProposalState, RemoteError> {
        let inner = self.inner.lock().unwrap();
        inner
            .proposal_states
            .get(&number)
            .copied()
            .ok_or_else(|| RemoteError::Api {
                status: 404,
                message: format!("no proposal {number}"),
            })
    }

    async fn current_user(&self) -> Result<String, RemoteError> {
        Ok(self.inner.lock().unwrap().current_user.clone())
    }
}

fn unsupported_write(op: &str) -> Result<String, RemoteError> {
    Err(RemoteError::Api {
        status: 0,
        message: format!("{op} is not supported by the mock"),
    })
}
