//! An in-memory forge implementing `crystalline_remote::Provider`, for the
//! CLI tests whose in-process server shares a team domain.
//!
//! Lifted from the service tests' `MockProvider` (`crates/service/tests/
//! support/mod.rs`, a test-only module of that crate, not reachable from
//! here) and trimmed to what a subscribe and a share as a pull request call:
//! one branch's commit history, the tarball a subscribe downloads, a compare
//! for a pull and the write side of a proposal. Stacks keep the trait's
//! default refusal.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::Mutex;

use crystalline_remote::RemoteError;
use crystalline_remote::provider::{
    ChangeKind, CompareResult, Feedback, HeadProbe, OpenProposalRef, OriginSpec, ProposalHandle,
    ProposalRequest, ProposalState, Provider, TreeWrite, UpstreamChange,
};
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Default)]
struct Inner {
    commits: HashMap<String, BTreeMap<String, Vec<u8>>>,
    trees: HashMap<String, BTreeMap<String, Vec<u8>>>,
    branches: HashMap<String, String>,
    blobs: HashMap<String, Vec<u8>>,
    proposals: BTreeMap<u64, (String, String, ProposalState)>,
    counter: u64,
    calls: Vec<String>,
}

/// A forge with one repository, whatever the origin names.
#[derive(Default)]
pub struct Forge {
    inner: Mutex<Inner>,
    /// When set, every new proposal is refused with this sentence.
    refuse_proposals: Mutex<Option<String>>,
}

impl Forge {
    /// A forge whose `main` holds `files`.
    pub fn with_main(files: BTreeMap<String, Vec<u8>>) -> Forge {
        let forge = Forge::default();
        {
            let mut inner = forge.inner.lock().unwrap();
            for content in files.values() {
                inner.blobs.insert(sha256_hex(content), content.clone());
            }
            inner.commits.insert("commit0".into(), files);
            inner.branches.insert("main".into(), "commit0".into());
        }
        forge
    }

    /// Refuses every new proposal from now on, with `sentence`.
    pub fn refuse_proposals(&self, sentence: &str) {
        *self.refuse_proposals.lock().unwrap() = Some(sentence.to_string());
    }

    /// Every write call so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.inner.lock().unwrap().calls.clone()
    }

    /// The files of the open pull request `number`'s head commit.
    pub fn proposal_files(&self, number: u64) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let Some((branch, _, _)) = inner.proposals.get(&number) else {
            return Vec::new();
        };
        let head = &inner.branches[branch];
        inner.commits[head].keys().cloned().collect()
    }

    fn next(inner: &mut Inner, kind: &str) -> String {
        inner.counter += 1;
        format!("{kind}{}", inner.counter)
    }
}

fn not_found(origin: &OriginSpec) -> RemoteError {
    RemoteError::RepoNotFound {
        repo: origin.repo.clone(),
    }
}

#[async_trait::async_trait]
impl Provider for Forge {
    async fn branch_head(
        &self,
        origin: &OriginSpec,
        _etag: Option<&str>,
    ) -> Result<HeadProbe, RemoteError> {
        let inner = self.inner.lock().unwrap();
        let head = inner
            .branches
            .get(&origin.branch)
            .cloned()
            .ok_or_else(|| not_found(origin))?;
        Ok(HeadProbe::Changed { head, etag: None })
    }

    async fn compare(
        &self,
        origin: &OriginSpec,
        base: &str,
        head: &str,
    ) -> Result<CompareResult, RemoteError> {
        let inner = self.inner.lock().unwrap();
        let old = inner.commits.get(base).ok_or_else(|| not_found(origin))?;
        let new = inner.commits.get(head).ok_or_else(|| not_found(origin))?;
        let mut files = Vec::new();
        for (path, content) in new {
            let kind = match old.get(path) {
                None => ChangeKind::Added,
                Some(before) if before != content => ChangeKind::Modified,
                Some(_) => continue,
            };
            files.push(UpstreamChange {
                path: path.clone(),
                kind,
                blob_sha: Some(sha256_hex(content)),
            });
        }
        for path in old.keys().filter(|p| !new.contains_key(*p)) {
            files.push(UpstreamChange {
                path: path.clone(),
                kind: ChangeKind::Removed,
                blob_sha: None,
            });
        }
        Ok(CompareResult {
            files,
            truncated: false,
        })
    }

    async fn blob(&self, _origin: &OriginSpec, sha: &str) -> Result<Vec<u8>, RemoteError> {
        self.inner
            .lock()
            .unwrap()
            .blobs
            .get(sha)
            .cloned()
            .ok_or_else(|| RemoteError::Api {
                status: 404,
                message: format!("no blob {sha}"),
            })
    }

    async fn read_file(
        &self,
        _origin: &OriginSpec,
        reference: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, RemoteError> {
        let inner = self.inner.lock().unwrap();
        let commit = inner
            .branches
            .get(reference)
            .cloned()
            .unwrap_or_else(|| reference.to_string());
        Ok(inner
            .commits
            .get(&commit)
            .and_then(|c| c.get(path).cloned()))
    }

    async fn tarball(&self, origin: &OriginSpec, commit: &str) -> Result<Vec<u8>, RemoteError> {
        let inner = self.inner.lock().unwrap();
        let files = inner.commits.get(commit).ok_or_else(|| not_found(origin))?;
        let top = format!("{}-{commit}", origin.repo.replace('/', "-"));
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{top}/{path}"), content.as_slice())
                .unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar).unwrap();
        Ok(gz.finish().unwrap())
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
            .cloned()
            .ok_or_else(|| not_found(origin))?;
        for write in writes {
            match &write.blob_sha {
                Some(sha) => {
                    let content = inner.blobs.get(sha).cloned().ok_or(RemoteError::Api {
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
        let id = Forge::next(&mut inner, "tree");
        inner.trees.insert(id.clone(), files);
        inner.calls.push(format!("create_tree:{id}"));
        Ok(id)
    }

    async fn create_root_tree(
        &self,
        _origin: &OriginSpec,
        writes: &[TreeWrite],
    ) -> Result<String, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        let mut files = BTreeMap::new();
        for write in writes {
            if let Some(sha) = &write.blob_sha
                && let Some(content) = inner.blobs.get(sha).cloned()
            {
                files.insert(write.path.clone(), content);
            }
        }
        let id = Forge::next(&mut inner, "tree");
        inner.trees.insert(id.clone(), files);
        inner.calls.push(format!("create_root_tree:{id}"));
        Ok(id)
    }

    async fn create_commit(
        &self,
        origin: &OriginSpec,
        _message: &str,
        tree: &str,
        _parents: &[String],
    ) -> Result<String, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        let files = inner
            .trees
            .get(tree)
            .cloned()
            .ok_or_else(|| not_found(origin))?;
        let id = Forge::next(&mut inner, "commit");
        inner.commits.insert(id.clone(), files);
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
        inner.branches.insert(name.to_string(), commit.to_string());
        inner.calls.push(format!("create_branch:{name}"));
        Ok(())
    }

    async fn delete_branch(&self, _origin: &OriginSpec, name: &str) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.branches.remove(name);
        inner.calls.push(format!("delete_branch:{name}"));
        Ok(())
    }

    async fn branch_ref(
        &self,
        _origin: &OriginSpec,
        name: &str,
    ) -> Result<Option<String>, RemoteError> {
        Ok(self.inner.lock().unwrap().branches.get(name).cloned())
    }

    async fn update_branch(
        &self,
        _origin: &OriginSpec,
        name: &str,
        commit: &str,
        force: bool,
    ) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.branches.insert(name.to_string(), commit.to_string());
        inner
            .calls
            .push(format!("update_branch:{name}:{commit}:force={force}"));
        Ok(())
    }

    async fn update_proposal(
        &self,
        _origin: &OriginSpec,
        number: u64,
        _title: Option<&str>,
        _body: Option<&str>,
        _base: Option<&str>,
    ) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push(format!("update_proposal:{number}"));
        Ok(())
    }

    async fn close_proposal(&self, _origin: &OriginSpec, number: u64) -> Result<(), RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.proposals.get_mut(&number) {
            entry.2 = ProposalState::Declined;
        }
        inner.calls.push(format!("close_proposal:{number}"));
        Ok(())
    }

    async fn proposal_feedback(
        &self,
        _origin: &OriginSpec,
        _number: u64,
    ) -> Result<Feedback, RemoteError> {
        Ok(Feedback::default())
    }

    async fn list_open_proposals(
        &self,
        origin: &OriginSpec,
    ) -> Result<Vec<OpenProposalRef>, RemoteError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .proposals
            .iter()
            .filter(|(_, (_, _, state))| *state == ProposalState::Open)
            .map(|(number, (branch, base, _))| OpenProposalRef {
                number: *number,
                branch: branch.clone(),
                head_sha: inner.branches.get(branch).cloned().unwrap_or_default(),
                base: base.clone(),
                head_repo: Some(origin.repo.clone()),
            })
            .collect())
    }

    async fn create_proposal(
        &self,
        _origin: &OriginSpec,
        req: &ProposalRequest,
    ) -> Result<ProposalHandle, RemoteError> {
        if let Some(sentence) = self.refuse_proposals.lock().unwrap().clone() {
            return Err(RemoteError::Refused(sentence));
        }
        let mut inner = self.inner.lock().unwrap();
        let number = inner.proposals.len() as u64 + 1;
        inner.proposals.insert(
            number,
            (
                req.branch.clone(),
                req.base_branch.clone(),
                ProposalState::Open,
            ),
        );
        inner.calls.push(format!("create_proposal:{}", req.branch));
        Ok(ProposalHandle {
            number,
            url: format!("https://github.test/acme/pull/{number}"),
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
            .map(|p| p.2)
            .ok_or(RemoteError::Api {
                status: 404,
                message: format!("no proposal {number}"),
            })
    }

    async fn current_user(&self) -> Result<String, RemoteError> {
        Ok("forge-user".to_string())
    }

    async fn default_branch(&self, _repo: &str) -> Result<String, RemoteError> {
        Ok("main".to_string())
    }
}
