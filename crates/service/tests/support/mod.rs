//! A minimal in-memory forge implementing `crystalline_remote::Provider`, for
//! the engine-level origin tests in `tests/origin.rs`.
//!
//! Lifted from `crystalline_remote`'s own `tests/mock/mod.rs` (a test-only
//! module of that crate, not reachable from here) and trimmed to what the
//! origin engine methods exercise: a single branch's commit history, tarball
//! download for `subscribe`, a diff-based compare for `pull`, a conditional
//! branch probe for `status`, and a working write side
//! (`create_blob`/`create_tree`/`create_commit`/`create_branch`/
//! `create_proposal`) against the same in-memory graph for `origin_share`.
//! Production code never depends on this; it exists only under `tests/`.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::sync::{Arc, Mutex};

use crystalline_index::EmbeddingProvider;
use crystalline_remote::provider::{
    ChangeKind, CompareResult, HeadProbe, OriginSpec, ProposalHandle, ProposalRequest,
    ProposalState, Provider, TreeWrite, UpstreamChange,
};
use crystalline_remote::{DeviceFlowStart, RemoteError};
use crystalline_service::engine::ConnectAuth;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tar::Header;

/// Every operation the `/api/v1` router mounts, as `METHOD path`, spelled the
/// way the OpenAPI document spells it (the engram wildcard becomes an ordinary
/// `{permalink}` template, which is the only notation OpenAPI has for it).
///
/// This list is the contract rather than a convenience, and it is shared
/// rather than duplicated on purpose: `openapi_snapshot.rs`'s
/// `the_document_covers_every_mounted_path` compares it against the served
/// OpenAPI document in *both* directions, so a route the router mounts but
/// nobody annotated, and an annotation for a route nobody mounts, each fail
/// there; `rest_write_api.rs`'s `write_ops_covers_every_mutating_route_mounted`
/// filters it down to the unsafe methods and checks that list against the
/// auth/CSRF matrix's own route set, so a write route mounted without a
/// matching matrix row fails there too. One list feeding two checks is the
/// point: a hand-kept second copy is exactly the kind of thing that drifts
/// unnoticed (Task 13 shipped three routes uncovered by the matrix this way).
/// The method is part of the string because a documented method the router
/// does not serve would generate a client function that compiles and then
/// answers 405.
pub const MOUNTED_OPERATIONS: &[&str] = &[
    "GET /api/v1/openapi.json",
    "POST /api/v1/auth/login",
    "POST /api/v1/auth/logout",
    "GET /api/v1/auth/me",
    "POST /api/v1/auth/setup",
    "GET /api/v1/domains",
    "POST /api/v1/domains",
    "DELETE /api/v1/domains/{domain}",
    "GET /api/v1/domains/{domain}/sync",
    "POST /api/v1/domains/{domain}/sync",
    "GET /api/v1/domains/{domain}/archive",
    "POST /api/v1/domains/{domain}/archive/preview",
    "POST /api/v1/domains/{domain}/archive/import",
    "GET /api/v1/domains/{domain}/tree",
    "GET /api/v1/domains/{domain}/manifest",
    "PUT /api/v1/domains/{domain}/manifest",
    "GET /api/v1/domains/{domain}/engrams",
    "POST /api/v1/domains/{domain}/engrams",
    "GET /api/v1/domains/{domain}/engrams/{permalink}",
    "GET /api/v1/domains/{domain}/inbound/{permalink}",
    "PUT /api/v1/domains/{domain}/engrams/{permalink}",
    "DELETE /api/v1/domains/{domain}/engrams/{permalink}",
    "POST /api/v1/domains/{domain}/retire",
    "POST /api/v1/domains/{domain}/move",
    "POST /api/v1/validate",
    "GET /api/v1/collab/{domain}/{permalink}",
    "GET /api/v1/search",
    "GET /api/v1/vocabulary",
    "GET /api/v1/context",
    "GET /api/v1/activity",
    "GET /api/v1/graph",
    "GET /api/v1/users",
    "POST /api/v1/users",
    "PATCH /api/v1/users/{name}",
    "DELETE /api/v1/users/{name}",
    "POST /api/v1/users/{name}/password",
    "GET /api/v1/settings/github",
    "DELETE /api/v1/settings/github",
    "POST /api/v1/settings/github/connect",
    "POST /api/v1/settings/github/token",
];

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
    /// Branches whose `branch_head` probe should fail with
    /// `RemoteError::RateLimited`, simulating GitHub throttling this
    /// machine. Set through `MockProvider::fail_branch_head_rate_limited`,
    /// cleared through `MockProvider::clear_branch_head_rate_limited`.
    rate_limited_branches: HashMap<String, Option<chrono::DateTime<chrono::Utc>>>,
    /// Branches whose `branch_head` probe should fail with
    /// `RemoteError::AuthExpired`, the mapped GitHub 401, simulating a token
    /// revoked or rotated out from under this machine while a connection was
    /// still on file. Set through `MockProvider::fail_branch_head_auth_expired`,
    /// cleared through `MockProvider::clear_branch_head_auth_expired`.
    auth_expired_branches: HashSet<String>,
    /// The lifecycle state `proposal_state` reports for a given proposal
    /// number, set through `MockProvider::set_proposal_state`. A number with
    /// no entry here errors as unknown, matching a genuinely nonexistent
    /// proposal.
    proposal_states: HashMap<u64, ProposalState>,
    /// Trees built by `create_tree`, keyed by a generated tree id: the parent
    /// commit's files with every write applied, ready for `create_commit` to
    /// snapshot into a new [`Commit`].
    trees: HashMap<String, BTreeMap<String, Vec<u8>>>,
    tree_counter: u64,
    proposal_counter: u64,
    /// How many times `branch_head` has been called, for the daemon poller
    /// tests: it is always the first call any `origin_update`/`origin_status`
    /// makes, so a count of zero after a tick proves the poller made no
    /// provider call at all (disabled, unauthenticated, or paused for a rate
    /// limit).
    branch_head_calls: usize,
    /// How many times `tarball` has been called, for the connect-race test:
    /// a first connect parks mid-download while an identical retry queues on
    /// the origin lock, so a count of exactly one proves the retry answered
    /// idempotently under the lock instead of re-downloading the whole repo.
    tarball_calls: usize,
    /// An optional gate every `tarball` download waits on until its sender
    /// flips it open. Set through `MockProvider::block_tarball`; unset (the
    /// default) means downloads never block. A `watch` channel is used so the
    /// gate stays open once released - a second, racing download proceeds
    /// too, rather than hanging.
    tarball_gate: Option<tokio::sync::watch::Receiver<bool>>,
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

    /// Marks `branch` as rate limited: every subsequent `branch_head` probe
    /// against it returns `Err(RemoteError::RateLimited { reset })`,
    /// simulating GitHub throttling this machine. `reset` is the reported
    /// reset instant, `None` when the mock forge reports no reset (the
    /// poller then falls back to its own default pause).
    pub fn fail_branch_head_rate_limited(
        &self,
        branch: &str,
        reset: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .rate_limited_branches
            .insert(branch.to_string(), reset);
    }

    /// Clears a previously injected rate limit for `branch`, simulating
    /// GitHub's rate limit window resetting.
    pub fn clear_branch_head_rate_limited(&self, branch: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.rate_limited_branches.remove(branch);
    }

    /// Marks `branch` as authenticating with a revoked or rotated token:
    /// every subsequent `branch_head` probe against it returns
    /// `Err(RemoteError::AuthExpired)`, simulating a token that stopped
    /// working while a connection was still on file, so a pull or status
    /// probe trips the engine's auth-invalidation path.
    pub fn fail_branch_head_auth_expired(&self, branch: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.auth_expired_branches.insert(branch.to_string());
    }

    /// Clears a previously injected auth-expired failure for `branch`,
    /// simulating a fresh token being connected in its place.
    pub fn clear_branch_head_auth_expired(&self, branch: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.auth_expired_branches.remove(branch);
    }

    /// How many times `branch_head` has been called so far.
    pub fn branch_head_calls(&self) -> usize {
        self.inner.lock().unwrap().branch_head_calls
    }

    /// Arms a gate that blocks every `tarball` download until the returned
    /// sender flips it open, and starts counting `tarball` calls. The
    /// connect-race test uses it to park a first connect mid-download while
    /// an identical retry races in behind it; `send(true)` on the sender
    /// releases the download. The gate stays open once released, so a second
    /// download never hangs.
    pub fn block_tarball(&self) -> tokio::sync::watch::Sender<bool> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        self.inner.lock().unwrap().tarball_gate = Some(rx);
        tx
    }

    /// How many times `tarball` has been called so far.
    pub fn tarball_calls(&self) -> usize {
        self.inner.lock().unwrap().tarball_calls
    }

    /// Sets the lifecycle state `proposal_state` reports for `number`, so a
    /// `pull`'s open-proposal refresh can observe a proposal moving to
    /// merged or declined.
    pub fn set_proposal_state(&self, number: u64, state: ProposalState) {
        let mut inner = self.inner.lock().unwrap();
        inner.proposal_states.insert(number, state);
    }

    /// The commit `branch` currently points at, or `None` if it was never
    /// set. Lets a test fast-forward `main` onto exactly the commit an
    /// `origin_share` call created, simulating GitHub merging its pull
    /// request.
    pub fn branch_commit(&self, branch: &str) -> Option<String> {
        self.inner.lock().unwrap().branches.get(branch).cloned()
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn branch_head(
        &self,
        origin: &OriginSpec,
        etag: Option<&str>,
    ) -> Result<HeadProbe, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.branch_head_calls += 1;
        if inner.offline_branches.contains(&origin.branch) {
            return Err(RemoteError::Offline);
        }
        if inner.auth_expired_branches.contains(&origin.branch) {
            return Err(RemoteError::AuthExpired);
        }
        if let Some(reset) = inner.rate_limited_branches.get(&origin.branch) {
            return Err(RemoteError::RateLimited { reset: *reset });
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
        // Count the download, then optionally park on the gate until its
        // sender flips it open. The receiver is cloned out from under the std
        // mutex so nothing is held across the await.
        let gate = {
            let mut inner = self.inner.lock().unwrap();
            inner.tarball_calls += 1;
            inner.tarball_gate.clone()
        };
        if let Some(mut rx) = gate {
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
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
        Ok(id)
    }

    async fn create_commit(
        &self,
        origin: &OriginSpec,
        _message: &str,
        tree: &str,
        _parent: &str,
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
        inner.commits.insert(id.clone(), Commit { files });
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
        Ok(())
    }

    async fn delete_branch(&self, _origin: &OriginSpec, _name: &str) -> Result<(), RemoteError> {
        Ok(())
    }

    async fn create_proposal(
        &self,
        _origin: &OriginSpec,
        req: &ProposalRequest,
    ) -> Result<ProposalHandle, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        inner.proposal_counter += 1;
        let number = inner.proposal_counter;
        inner.proposal_states.insert(number, ProposalState::Open);
        Ok(ProposalHandle {
            number,
            url: format!("https://github.test/{}/pull/{number}", req.branch),
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

/// An embedding provider that returns fixed small vectors and counts calls,
/// for the background embed worker tests in `tests/origin.rs`.
pub struct CountingEmbedder {
    pub calls: std::sync::atomic::AtomicUsize,
}

impl CountingEmbedder {
    pub fn new() -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl Default for CountingEmbedder {
    fn default() -> Self {
        CountingEmbedder::new()
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for CountingEmbedder {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![vec![0.1_f32; 4]; texts.len()])
    }

    fn model_id(&self) -> &str {
        "test-model"
    }

    fn dims(&self) -> usize {
        4
    }

    fn max_input_tokens(&self) -> usize {
        512
    }
}

// --- GitHub connect auth: shared test double --------------------------------

/// A fake [`ConnectAuth`] for the `configure` tool's connect actions and the
/// engine-level GitHub status/ready/disconnect verbs. Lifted out of
/// `tests/mcp_collab.rs` (formerly `FakeConnectAuth`) so both that suite and
/// `tests/domain_admin.rs` share one double instead of keeping two: the
/// general one-shot constructor [`fake_auth`] sets all three outcomes once,
/// each consumed exactly once by its matching method, with
/// `run_device_flow` blockable on `run_gate` so a test can observe the
/// "still waiting on the user" state before letting the flow land;
/// [`StubConnectAuth::accepting`] and [`StubConnectAuth::denying`] are
/// narrower convenience constructors for tests that only need a
/// token-validate acceptor or a device flow that always fails once released.
pub struct StubConnectAuth {
    start_result: Mutex<Option<Result<DeviceFlowStart, RemoteError>>>,
    /// Gates `run_device_flow`'s completion; a test releases it with
    /// `auth.run_gate.notify_one()` once it has observed the "still waiting
    /// on the user" state.
    pub run_gate: Arc<tokio::sync::Notify>,
    run_result: Mutex<Option<Result<String, RemoteError>>>,
    validate_result: Mutex<Option<Result<String, RemoteError>>>,
    /// When set, every `validate_token` call returns this login instead of
    /// consuming `validate_result`, so a connect in any test never panics on
    /// a used-up one-shot outcome. Backs [`StubConnectAuth::accepting`].
    accept_any: Option<String>,
}

/// The general one-shot double (the original `FakeConnectAuth` constructor):
/// each of the three outcomes is set once here and consumed exactly once by
/// its matching `ConnectAuth` method.
pub fn fake_auth(
    start: Result<DeviceFlowStart, RemoteError>,
    run: Result<String, RemoteError>,
    validate: Result<String, RemoteError>,
) -> Arc<StubConnectAuth> {
    Arc::new(StubConnectAuth {
        start_result: Mutex::new(Some(start)),
        run_gate: Arc::new(tokio::sync::Notify::new()),
        run_result: Mutex::new(Some(run)),
        validate_result: Mutex::new(Some(validate)),
        accept_any: None,
    })
}

/// A device-flow start payload with fixed, inert field values, for a
/// `ConnectAuth` fake's `start_device_flow` outcome.
pub fn device_flow_start() -> DeviceFlowStart {
    DeviceFlowStart {
        device_code: "devcode".to_string(),
        user_code: "ABCD-1234".to_string(),
        verification_url: "https://github.com/login/device".to_string(),
        interval_secs: 0,
        expires_in_secs: 900,
    }
}

impl StubConnectAuth {
    /// Validates any token as `user`, repeatably. Device flow start always
    /// fails (`RemoteError::NotConnected`): a test built this way needs no
    /// device path.
    pub fn accepting(user: &str) -> Self {
        Self {
            start_result: Mutex::new(Some(Err(RemoteError::NotConnected))),
            run_gate: Arc::new(tokio::sync::Notify::new()),
            run_result: Mutex::new(Some(Err(RemoteError::NotConnected))),
            validate_result: Mutex::new(None),
            accept_any: Some(user.to_string()),
        }
    }

    /// A device flow that starts with a canned code (`ABCD-1234` at
    /// `https://github.example/device`), blocks on the returned `Notify`
    /// until released, then fails, reporting `reason` as GitHub's own
    /// refusal (a `RemoteError::Api`, the shape a declined or expired flow
    /// arrives in).
    ///
    /// The gate is what makes a device-flow test deterministic: the engine
    /// *spawns* the task that runs the flow, so a stub that failed instantly
    /// could land - and, on the next status read, clear - its outcome before
    /// the caller has even read the body of the response that started it.
    pub fn denying(reason: &str) -> (Self, Arc<tokio::sync::Notify>) {
        let gate = Arc::new(tokio::sync::Notify::new());
        let auth = Self {
            start_result: Mutex::new(Some(Ok(DeviceFlowStart {
                device_code: "devcode".to_string(),
                user_code: "ABCD-1234".to_string(),
                verification_url: "https://github.example/device".to_string(),
                interval_secs: 0,
                expires_in_secs: 900,
            }))),
            run_gate: gate.clone(),
            run_result: Mutex::new(Some(Err(RemoteError::Api {
                status: 403,
                message: reason.to_string(),
            }))),
            validate_result: Mutex::new(None),
            accept_any: None,
        };
        (auth, gate)
    }
}

#[async_trait::async_trait]
impl ConnectAuth for StubConnectAuth {
    async fn start_device_flow(
        &self,
        _auth_base: &str,
        _client_id: &str,
    ) -> Result<DeviceFlowStart, RemoteError> {
        if self.accept_any.is_some() {
            // The acceptor has no device path, and says so repeatably: a
            // suite that drives the connect route more than once (the write
            // matrix does) must get the same refusal every time rather than
            // panic on a used-up one-shot.
            return Err(RemoteError::NotConnected);
        }
        self.start_result
            .lock()
            .unwrap()
            .take()
            .expect("start_device_flow result not set")
    }

    async fn run_device_flow(
        &self,
        _auth_base: &str,
        _client_id: &str,
        _start: &DeviceFlowStart,
    ) -> Result<String, RemoteError> {
        self.run_gate.notified().await;
        self.run_result
            .lock()
            .unwrap()
            .take()
            .expect("run_device_flow result not set")
    }

    async fn validate_token(
        &self,
        _api_url: Option<&str>,
        _token: &str,
    ) -> Result<String, RemoteError> {
        if let Some(user) = &self.accept_any {
            return Ok(user.clone());
        }
        self.validate_result
            .lock()
            .unwrap()
            .take()
            .expect("validate_token result not set")
    }
}

// --- scratch state directory -------------------------------------------------

/// The process-wide scratch redirection, reference counted. `None` until the
/// first handle is taken and again once the last one drops.
static SCRATCH: std::sync::Mutex<Option<Scratch>> = std::sync::Mutex::new(None);

struct Scratch {
    dir: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    handles: usize,
}

/// The base-directory variables both platform strategies read, the same seven
/// `crates/cli/tests/common` sets for the child processes it spawns. Setting
/// the Windows names on unix is harmless, so one list covers both.
const BASE_DIR_VARS: [&str; 7] = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
];

/// Points the base directories at a scratch home for as long as any handle is
/// alive, restoring the surrounding environment when the last one drops.
///
/// Needed by any in-process test that reaches code writing under
/// `crystalline_core::config::state_dir()` - the maintenance state file the
/// REST write handlers and the evolve run recorder stamp - so a test run never
/// leaves anything in the developer's real state directory.
///
/// Reference counted rather than a mutex held for the test's duration,
/// because a single test may build several servers (the write matrix builds
/// three) and a second acquisition would otherwise wait on the first for ever.
/// One redirection per process is exactly one per test under `cargo nextest`,
/// which runs each test in its own process and is what CI runs; a plain
/// `cargo test` run shares one scratch home across the tests of a binary,
/// which still keeps every write out of the developer's state directory.
pub struct ScratchStateDir {
    home: std::path::PathBuf,
}

impl ScratchStateDir {
    /// Take a handle, redirecting the base directories if this is the first.
    pub fn acquire() -> ScratchStateDir {
        let mut slot = SCRATCH.lock().unwrap();
        if let Some(scratch) = slot.as_mut() {
            scratch.handles += 1;
            return ScratchStateDir {
                home: scratch.dir.path().to_path_buf(),
            };
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        let previous = BASE_DIR_VARS
            .iter()
            .map(|v| (*v, std::env::var_os(v)))
            .collect();
        // SAFETY: the environment is restored when the last handle drops, and
        // every mutation happens under the SCRATCH lock.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
            std::env::set_var("XDG_STATE_HOME", home.join("state"));
            std::env::set_var("XDG_CACHE_HOME", home.join("cache"));
            std::env::set_var("USERPROFILE", &home);
            std::env::set_var("APPDATA", home.join("state"));
            std::env::set_var("LOCALAPPDATA", home.join("local"));
        }
        *slot = Some(Scratch {
            dir,
            previous,
            handles: 1,
        });
        ScratchStateDir { home }
    }

    /// The scratch home itself, for a test asserting that what it wrote landed
    /// under it.
    pub fn home(&self) -> &std::path::Path {
        &self.home
    }

    /// The maintenance state file this redirection resolves.
    pub fn maintenance_path(&self) -> std::path::PathBuf {
        crystalline_service::maintenance::path().unwrap()
    }
}

impl Drop for ScratchStateDir {
    fn drop(&mut self) {
        let mut slot = SCRATCH.lock().unwrap();
        let Some(scratch) = slot.as_mut() else {
            return;
        };
        scratch.handles -= 1;
        if scratch.handles > 0 {
            return;
        }
        for (var, value) in &scratch.previous {
            // SAFETY: as above, under the SCRATCH lock.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(var, v),
                    None => std::env::remove_var(var),
                }
            }
        }
        *slot = None;
    }
}
