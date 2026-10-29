//! The credential seam with a GitHub App configured: pulls, shares, the
//! status and the Reflection backup go out on the App's installation token
//! for each repository, and a repository the App is not installed on is
//! refused with the install url instead of falling back to the instance
//! token.
//!
//! Unlike the rest of this binary, nothing here injects a provider through
//! `Engine::with_origin_provider`: that override short-circuits the very seam
//! under test. Instead a throwaway axum server on a loopback port speaks the
//! slice of GitHub's REST API the engine uses, backed by one in-memory
//! [`MockProvider`] per repository, plus the three App routes (`GET /app`,
//! the installation lookup and the token mint). Every request is logged with
//! its `Authorization` header, which is what the tests assert on.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use crystalline_core::config::{GitHubConfig, GlobalConfig, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_remote::provider::{
    ChangeKind, OriginSpec, ProposalRequest, ProposalState, Provider, TreeWrite,
};
use crystalline_remote::reflections_backup::BACKUP_BRANCH;
use crystalline_remote::state::OriginState;
use crystalline_remote::{AppCredentials, AppLookup, AppStore, RemoteError};
use crystalline_service::engine::{AppHome, AppKeychainSeam};
use crystalline_service::engine::{ConfigureAction, EngineError, ShareActor};
use crystalline_service::params::Readers;
use crystalline_service::reflection_backup::BackupCause;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EnvOverlay, Scope};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::support::MockProvider;

/// The throwaway test key the remote crate's App tests sign with; it belongs
/// to no App.
pub(crate) const PEM: &str = include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem");

pub(crate) const SLUG: &str = "crystalline-acme";
pub(crate) const INSTANCE_TOKEN: &str = "gho_instance";
const HOME: &str = "https://kb.example.com";

/// One request the fake forge saw.
#[derive(Clone, Debug)]
pub(crate) struct Hit {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) auth: Option<String>,
}

impl Hit {
    /// The repository a `/repos/{owner}/{name}/...` request is about.
    pub(crate) fn repo(&self) -> Option<String> {
        let rest = self.path.strip_prefix("/repos/")?;
        let mut parts = rest.split('/');
        Some(format!("{}/{}", parts.next()?, parts.next()?))
    }

    /// A request for the repository's content or history, as opposed to the
    /// App's installation lookup.
    pub(crate) fn is_repo_call(&self) -> bool {
        self.repo().is_some() && !self.path.ends_with("/installation")
    }
}

/// What one refresh token's exchange answers: the new access token, the new
/// refresh token, and the GitHub user both belong to.
pub(crate) type Refreshed = (String, String, (u64, String));

/// A GitHub stand-in: repositories, which installation covers each one, the
/// token each installation mints, and the log.
pub(crate) struct Forge {
    repos: HashMap<String, Arc<MockProvider>>,
    /// Repository -> installation id, for the repositories the App is
    /// installed on.
    installed: StdMutex<HashMap<String, u64>>,
    /// Installation id -> the token its mint answers.
    tokens: HashMap<u64, String>,
    log: StdMutex<Vec<Hit>>,
    /// Installation ids in the order their tokens were minted.
    mints: StdMutex<Vec<u64>>,
    /// How many `POST .../git/trees` still answer 401.
    trees_unauthorized: AtomicUsize,
    /// The body of every `POST .../git/commits`, in order.
    commits: StdMutex<Vec<Value>>,
    /// The App user tokens (`ghu_`) that may read and write every
    /// repository, and who each one belongs to on `GET /user`.
    pub(crate) users: StdMutex<HashMap<String, (u64, String)>>,
    /// Refresh token -> the access and refresh token its exchange answers,
    /// and whose they are. A refresh token is spent on use, the way GitHub
    /// spends it: a second exchange of the same one answers
    /// `bad_refresh_token`.
    pub(crate) refreshable: StdMutex<HashMap<String, Refreshed>>,
    /// Every refresh token a `POST /login/oauth/access_token` presented, in
    /// order.
    pub(crate) refreshes: StdMutex<Vec<String>>,
    /// Every `DELETE /applications/{client_id}/token`: its `Authorization`
    /// header and the token in its body.
    pub(crate) revokes: StdMutex<Vec<(Option<String>, String)>>,
    /// When set, every revoke answers 503.
    pub(crate) revoke_fails: std::sync::atomic::AtomicBool,
    /// When set, a refresh answers only once the test adds a permit here,
    /// instead of after 200 ms.
    pub(crate) refresh_gate: StdMutex<Option<Arc<tokio::sync::Semaphore>>>,
    /// Repository -> its collaborators as `GET .../collaborators` lists
    /// them (`id`, `login`, `role_name`).
    pub(crate) collaborators: StdMutex<HashMap<String, Vec<Value>>>,
    /// The repositories `GET /repos/{o}/{r}` calls public.
    pub(crate) public: StdMutex<std::collections::HashSet<String>>,
    /// When set, every collaborator listing answers 503.
    pub(crate) collaborators_down: std::sync::atomic::AtomicBool,
}

impl Forge {
    pub(crate) fn hits(&self) -> Vec<Hit> {
        self.log.lock().unwrap().clone()
    }

    fn mints(&self) -> Vec<u64> {
        self.mints.lock().unwrap().clone()
    }

    pub(crate) fn commits(&self) -> Vec<Value> {
        self.commits.lock().unwrap().clone()
    }

    /// Takes the App off `repo`: the lookup answers 404 from now on, and the
    /// tokens of its installation stop working, the way GitHub revokes them
    /// on an uninstall.
    pub(crate) fn uninstall(&self, repo: &str) {
        self.installed.lock().unwrap().remove(repo);
    }

    /// Puts the App on `repo` as installation `id`.
    pub(crate) fn install(&self, repo: &str, id: u64) {
        self.installed.lock().unwrap().insert(repo.to_string(), id);
    }

    /// Whether `auth` may read and write `repo`: the instance token always,
    /// an installation token while its installation still covers the
    /// repository.
    fn authorized(&self, repo: &str, auth: Option<&str>) -> bool {
        let Some(token) = auth.and_then(|a| a.strip_prefix("Bearer ")) else {
            return false;
        };
        if token == INSTANCE_TOKEN || self.users.lock().unwrap().contains_key(token) {
            return true;
        }
        match self.installed.lock().unwrap().get(repo) {
            Some(id) => self.tokens.get(id).is_some_and(|t| t == token),
            None => false,
        }
    }
}

/// Builds the forge and serves it on a loopback port; returns it with its
/// base url, which is also the `github.api_url` the engine is pointed at.
pub(crate) async fn spawn_forge(
    repos: &[(&str, Arc<MockProvider>)],
    installed: &[(&str, u64)],
    tokens: &[(u64, &str)],
) -> (Arc<Forge>, String) {
    let forge = Arc::new(Forge {
        repos: repos
            .iter()
            .map(|(name, mock)| (name.to_string(), mock.clone()))
            .collect(),
        installed: StdMutex::new(
            installed
                .iter()
                .map(|(name, id)| (name.to_string(), *id))
                .collect(),
        ),
        tokens: tokens
            .iter()
            .map(|(id, token)| (*id, token.to_string()))
            .collect(),
        log: StdMutex::new(Vec::new()),
        mints: StdMutex::new(Vec::new()),
        trees_unauthorized: AtomicUsize::new(0),
        commits: StdMutex::new(Vec::new()),
        users: StdMutex::new(HashMap::new()),
        refreshable: StdMutex::new(HashMap::new()),
        refreshes: StdMutex::new(Vec::new()),
        revokes: StdMutex::new(Vec::new()),
        revoke_fails: std::sync::atomic::AtomicBool::new(false),
        refresh_gate: StdMutex::new(None),
        collaborators: StdMutex::new(HashMap::new()),
        public: StdMutex::new(std::collections::HashSet::new()),
        collaborators_down: std::sync::atomic::AtomicBool::new(false),
    });
    let router = Router::new().fallback(handle).with_state(forge.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (forge, format!("http://{addr}"))
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({"message": "Not Found"})),
    )
        .into_response()
}

fn ok(value: Value) -> Response {
    (StatusCode::OK, axum::Json(value)).into_response()
}

fn created(value: Value) -> Response {
    (StatusCode::CREATED, axum::Json(value)).into_response()
}

fn failed(e: RemoteError) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        axum::Json(json!({"message": e.to_string()})),
    )
        .into_response()
}

fn query_param(uri: &Uri, key: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| {
            percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned()
        })
    })
}

async fn handle(
    State(forge): State<Arc<Forge>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let hit = Hit {
        method: method.to_string(),
        path: path.clone(),
        auth: auth.clone(),
    };
    forge.log.lock().unwrap().push(hit.clone());

    if path == "/login/oauth/access_token" && method == Method::POST {
        return refresh_route(&forge, body).await;
    }
    if path == "/user" && method == Method::GET {
        let token = auth.as_deref().and_then(|a| a.strip_prefix("Bearer "));
        return match token.and_then(|t| forge.users.lock().unwrap().get(t).cloned()) {
            Some((id, login)) => ok(json!({"id": id, "login": login})),
            None => (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"message": "Bad credentials"})),
            )
                .into_response(),
        };
    }
    if path.starts_with("/applications/") && path.ends_with("/token") && method == Method::DELETE {
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let token = body["access_token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        forge.revokes.lock().unwrap().push((auth, token.clone()));
        if forge.revoke_fails.load(Ordering::SeqCst) {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({"message": "unavailable"})),
            )
                .into_response();
        }
        forge.users.lock().unwrap().remove(&token);
        return StatusCode::NO_CONTENT.into_response();
    }
    if path == "/app" && method == Method::GET {
        return ok(json!({
            "id": 7,
            "slug": SLUG,
            "owner": {"login": "acme"},
            "html_url": format!("https://github.test/apps/{SLUG}"),
        }));
    }
    if let Some(rest) = path.strip_prefix("/app/installations/")
        && let Some(id) = rest.strip_suffix("/access_tokens")
        && method == Method::POST
    {
        let id: u64 = id.parse().unwrap();
        let live = forge.installed.lock().unwrap().values().any(|i| *i == id);
        if !live {
            return not_found();
        }
        forge.mints.lock().unwrap().push(id);
        let expires = chrono::Utc::now() + chrono::Duration::hours(1);
        return created(json!({
            "token": forge.tokens[&id],
            "expires_at": expires.to_rfc3339(),
        }));
    }
    if let Some(login) = path.strip_prefix("/users/")
        && method == Method::GET
    {
        let login = percent_encoding::percent_decode_str(login)
            .decode_utf8_lossy()
            .into_owned();
        return ok(json!({"login": login, "id": 41898282, "type": "Bot"}));
    }
    let Some(repo) = hit.repo() else {
        return not_found();
    };
    let tail = path
        .strip_prefix(&format!("/repos/{repo}"))
        .unwrap_or_default()
        .to_string();
    if tail == "/installation" {
        return match forge.installed.lock().unwrap().get(&repo) {
            Some(id) => ok(json!({"id": id})),
            None => not_found(),
        };
    }
    let Some(mock) = forge.repos.get(&repo).cloned() else {
        return not_found();
    };
    if !forge.authorized(&repo, auth.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"message": "Bad credentials"})),
        )
            .into_response();
    }
    let origin = OriginSpec {
        repo: repo.clone(),
        subpath: None,
        branch: "main".to_string(),
    };
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    repo_route(&forge, &mock, &origin, &method, &uri, &tail, body).await
}

/// `POST /login/oauth/access_token` with `grant_type=refresh_token`: answers
/// the next pair for a refresh token the forge knows and spends it, and
/// `bad_refresh_token` for any other. The pair is rotated before the answer
/// is sent, and the answer waits: 200 ms, or until a test opens
/// `refresh_gate`.
async fn refresh_route(forge: &Forge, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let presented = body["refresh_token"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // Spent and rotated at once, the way GitHub does it: whatever happens to
    // the caller after this, the old refresh token no longer works.
    let next = forge.refreshable.lock().unwrap().remove(&presented);
    if let Some((access, _, owner)) = &next {
        forge
            .users
            .lock()
            .unwrap()
            .insert(access.clone(), owner.clone());
    }
    forge.refreshes.lock().unwrap().push(presented.clone());
    let gate = forge.refresh_gate.lock().unwrap().clone();
    match gate {
        Some(gate) => drop(gate.acquire().await.unwrap()),
        None => tokio::time::sleep(Duration::from_millis(200)).await,
    }
    match next {
        Some((access, refresh, _owner)) => ok(json!({
            "access_token": access,
            "token_type": "bearer",
            "expires_in": 28800,
            "refresh_token": refresh,
            "refresh_token_expires_in": 15897600,
        })),
        None => ok(json!({
            "error": "bad_refresh_token",
            "error_description": "The refresh token passed is incorrect or expired.",
        })),
    }
}

async fn repo_route(
    forge: &Forge,
    mock: &MockProvider,
    origin: &OriginSpec,
    method: &Method,
    uri: &Uri,
    tail: &str,
    body: Value,
) -> Response {
    let get = *method == Method::GET;
    let post = *method == Method::POST;
    if tail.is_empty() && get {
        let private = !forge.public.lock().unwrap().contains(&origin.repo);
        return match mock.default_branch(&origin.repo).await {
            Ok(branch) => ok(json!({"default_branch": branch, "private": private})),
            Err(e) => failed(e),
        };
    }
    if tail == "/collaborators" && get {
        if forge.collaborators_down.load(Ordering::SeqCst) {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({"message": "unavailable"})),
            )
                .into_response();
        }
        let people = forge
            .collaborators
            .lock()
            .unwrap()
            .get(&origin.repo)
            .cloned()
            .unwrap_or_default();
        return ok(Value::Array(people));
    }
    if let Some(branch) = tail.strip_prefix("/git/ref/heads/")
        && get
    {
        return match mock.branch_ref(origin, branch).await {
            Ok(Some(sha)) => ok(json!({"object": {"sha": sha}})),
            Ok(None) => not_found(),
            Err(e) => failed(e),
        };
    }
    if let Some(range) = tail.strip_prefix("/compare/")
        && get
    {
        if query_param(uri, "page").as_deref() != Some("1") {
            return ok(json!({"files": []}));
        }
        let (base, head) = range.split_once("...").unwrap();
        return match mock.compare(origin, base, head).await {
            Ok(result) => {
                let files: Vec<Value> = result
                    .files
                    .into_iter()
                    .map(|f| {
                        let (status, previous) = match f.kind {
                            ChangeKind::Added => ("added", None),
                            ChangeKind::Modified => ("modified", None),
                            ChangeKind::Removed => ("removed", None),
                            ChangeKind::Renamed { previous } => ("renamed", Some(previous)),
                        };
                        json!({
                            "filename": f.path,
                            "status": status,
                            "sha": f.blob_sha.unwrap_or_default(),
                            "previous_filename": previous,
                        })
                    })
                    .collect();
                ok(json!({"files": files}))
            }
            Err(e) => failed(e),
        };
    }
    if let Some(sha) = tail.strip_prefix("/git/blobs/")
        && get
    {
        return match mock.blob(origin, sha).await {
            Ok(bytes) => ok(json!({"content": BASE64.encode(bytes), "encoding": "base64"})),
            Err(e) => failed(e),
        };
    }
    if tail == "/git/blobs" && post {
        let content = BASE64.decode(body["content"].as_str().unwrap()).unwrap();
        return match mock.create_blob(origin, &content).await {
            Ok(sha) => created(json!({"sha": sha})),
            Err(e) => failed(e),
        };
    }
    if let Some(file) = tail.strip_prefix("/contents/")
        && get
    {
        let file = percent_encoding::percent_decode_str(file)
            .decode_utf8_lossy()
            .into_owned();
        let reference = query_param(uri, "ref").unwrap_or_default();
        return match mock.read_file(origin, &reference, &file).await {
            Ok(Some(bytes)) => (StatusCode::OK, bytes).into_response(),
            Ok(None) => not_found(),
            Err(e) => failed(e),
        };
    }
    if let Some(commit) = tail.strip_prefix("/tarball/")
        && get
    {
        return match mock.tarball(origin, commit).await {
            Ok(bytes) => (StatusCode::OK, bytes).into_response(),
            Err(e) => failed(e),
        };
    }
    if let Some(sha) = tail.strip_prefix("/git/commits/")
        && get
    {
        // The in-memory forge names a tree by the commit it was made on, so
        // the tree of a commit is the commit itself.
        return ok(json!({"sha": sha, "tree": {"sha": sha}}));
    }
    if tail == "/git/trees" && post {
        if forge
            .trees_unauthorized
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"message": "Bad credentials"})),
            )
                .into_response();
        }
        let writes: Vec<TreeWrite> = body["tree"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| TreeWrite {
                path: entry["path"].as_str().unwrap().to_string(),
                blob_sha: entry["sha"].as_str().map(str::to_string),
            })
            .collect();
        let tree = match body["base_tree"].as_str() {
            Some(base) => mock.create_tree(origin, base, &writes).await,
            None => mock.create_root_tree(origin, &writes).await,
        };
        return match tree {
            Ok(sha) => created(json!({"sha": sha})),
            Err(e) => failed(e),
        };
    }
    if tail == "/git/commits" && post {
        forge.commits.lock().unwrap().push(body.clone());
        let parents: Vec<String> = body["parents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect();
        return match mock
            .create_commit(
                origin,
                body["message"].as_str().unwrap(),
                body["tree"].as_str().unwrap(),
                &parents,
            )
            .await
        {
            Ok(sha) => created(json!({"sha": sha})),
            Err(e) => failed(e),
        };
    }
    if tail == "/git/refs" && post {
        let name = body["ref"]
            .as_str()
            .unwrap()
            .trim_start_matches("refs/heads/");
        return match mock
            .create_branch(origin, name, body["sha"].as_str().unwrap())
            .await
        {
            Ok(()) => created(json!({})),
            Err(e) => failed(e),
        };
    }
    if let Some(name) = tail.strip_prefix("/git/refs/heads/") {
        let result = match *method {
            Method::PATCH => {
                mock.update_branch(
                    origin,
                    name,
                    body["sha"].as_str().unwrap(),
                    body["force"].as_bool().unwrap_or(false),
                )
                .await
            }
            Method::DELETE => mock.delete_branch(origin, name).await,
            _ => return not_found(),
        };
        return match result {
            Ok(()) => ok(json!({})),
            Err(e) => failed(e),
        };
    }
    if tail == "/pulls" && get {
        if query_param(uri, "page").as_deref() != Some("1") {
            return ok(json!([]));
        }
        return match mock.list_open_proposals(origin).await {
            Ok(open) => ok(Value::Array(
                open.into_iter()
                    .map(|p| {
                        json!({
                            "number": p.number,
                            "head": {"ref": p.branch, "sha": p.head_sha,
                                     "repo": p.head_repo.map(|r| json!({"full_name": r}))},
                            "base": {"ref": p.base},
                        })
                    })
                    .collect(),
            )),
            Err(e) => failed(e),
        };
    }
    if tail == "/pulls" && post {
        let req = ProposalRequest {
            title: body["title"].as_str().unwrap().to_string(),
            body: body["body"].as_str().unwrap().to_string(),
            branch: body["head"].as_str().unwrap().to_string(),
            base_branch: body["base"].as_str().unwrap().to_string(),
        };
        return match mock.create_proposal(origin, &req).await {
            Ok(handle) => created(json!({"number": handle.number, "html_url": handle.url})),
            Err(e) => failed(e),
        };
    }
    if tail.ends_with("/reviews") || tail.ends_with("/comments") {
        return ok(json!([]));
    }
    if let Some(number) = tail.strip_prefix("/pulls/")
        && let Ok(number) = number.parse::<u64>()
    {
        return match *method {
            Method::GET => match mock.proposal_state(origin, number).await {
                Ok(state) => ok(json!({
                    "state": if state == ProposalState::Open { "open" } else { "closed" },
                    "merged": state == ProposalState::Merged,
                })),
                Err(e) => failed(e),
            },
            Method::PATCH if body["state"] == "closed" => {
                match mock.close_proposal(origin, number).await {
                    Ok(()) => ok(json!({})),
                    Err(e) => failed(e),
                }
            }
            Method::PATCH => match mock
                .update_proposal(
                    origin,
                    number,
                    body["title"].as_str(),
                    body["body"].as_str(),
                    body["base"].as_str(),
                )
                .await
            {
                Ok(()) => ok(json!({})),
                Err(e) => failed(e),
            },
            _ => not_found(),
        };
    }
    // Stacks and anything else: a forge without them.
    not_found()
}

// --- the engine --------------------------------------------------------------

pub(crate) fn manifest(extra: &str) -> Vec<u8> {
    format!(
        "---\ntype: manifest\ntitle: Team\nteam: github\npermalink: manifest\n{extra}tags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n"
    )
    .into_bytes()
}

pub(crate) fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
    .into_bytes()
}

pub(crate) fn files(pairs: &[(&str, Vec<u8>)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(path, bytes)| (path.to_string(), bytes.clone()))
        .collect()
}

/// A repository holding only its MANIFEST (plus `extra` MANIFEST lines) on
/// `main`.
pub(crate) fn repo_with_manifest(extra: &str) -> Arc<MockProvider> {
    let mock = Arc::new(MockProvider::new());
    let head = mock.add_commit(files(&[("MANIFEST.md", manifest(extra))]));
    mock.set_branch("main", &head);
    mock
}

/// The environment of an instance with an App: the instance token too, so a
/// test can tell which of the two a request carried.
pub(crate) fn app_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN),
        ("CRYSTALLINE_GITHUB_APP_ID", "7"),
        ("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.test"),
        ("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", PEM),
    ]
}

/// An engine pointed at the forge, with every state directory in `tmp` and
/// the given environment.
pub(crate) async fn engine(tmp: &Path, api_url: &str, env: &[(&str, &str)]) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(api_url.to_string()),
            ..GitHubConfig::default()
        }),
        service: Some(ServiceConfig {
            public_url: Some(HOME.into()),
            ..Default::default()
        }),
        ..GlobalConfig::default()
    };
    let overlay = EnvOverlay::from_vars(
        env.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(tmp.join("config.yaml")),
    )
    .with_origins_dir(tmp.join("origins"))
    .with_state_dir(tmp.to_path_buf())
    .with_token_store_dir(tmp.join("tokens"))
    .with_env_overlay(overlay)
}

pub(crate) async fn connect(eng: &Engine, repo: &str, name: &str, root: &Path) -> Value {
    eng.origin_add(
        repo,
        Some(name),
        None,
        None,
        Some(root.to_str().unwrap()),
        Some(Readers::Shared),
    )
    .await
    .unwrap()
}

/// Every `Authorization` value the repository calls of `repo` carried.
pub(crate) fn repo_auths(forge: &Forge, repo: &str) -> Vec<Option<String>> {
    forge
        .hits()
        .into_iter()
        .filter(|h| h.is_repo_call() && h.repo().as_deref() == Some(repo))
        .map(|h| h.auth)
        .collect()
}

pub(crate) fn bearer(token: &str) -> Option<String> {
    Some(format!("Bearer {token}"))
}

// --- the tests ---------------------------------------------------------------

#[tokio::test]
async fn pull_uses_the_installation_token_when_an_app_is_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) = spawn_forge(
        &[("acme/kb", mock.clone())],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = engine(tmp.path(), &url, &app_env()).await;
    let root = tmp.path().join("kb");
    connect(&eng, "acme/kb", "kb", &root).await;

    let head = mock.add_commit(files(&[
        ("MANIFEST.md", manifest("")),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &head);
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(root.join("notes/new.md").exists(), "the pull landed");

    let auths = repo_auths(&forge, "acme/kb");
    assert!(
        auths.len() > 2,
        "the connect and the pull reached the forge"
    );
    assert!(
        auths.iter().all(|a| *a == bearer("ghs_test")),
        "every repository call carried the installation token: {auths:?}"
    );
    assert!(
        forge
            .hits()
            .iter()
            .all(|h| h.auth != bearer(INSTANCE_TOKEN)),
        "the instance token never left the machine"
    );
    assert_eq!(forge.mints(), vec![1], "one token, reused for every call");
}

#[tokio::test]
async fn pull_uses_the_instance_token_without_an_app() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) = spawn_forge(
        &[("acme/kb", mock.clone())],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = engine(
        tmp.path(),
        &url,
        &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
    )
    .await;
    let root = tmp.path().join("kb");
    connect(&eng, "acme/kb", "kb", &root).await;
    let head = mock.add_commit(files(&[
        ("MANIFEST.md", manifest("")),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &head);
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(root.join("notes/new.md").exists());

    let hits = forge.hits();
    assert!(
        hits.iter()
            .all(|h| h.is_repo_call() && h.auth == bearer(INSTANCE_TOKEN)),
        "only repository calls, all on the instance token: {hits:?}"
    );
    assert!(forge.mints().is_empty());
    assert!(eng.github_app().is_none());
}

#[tokio::test]
async fn a_repository_without_the_app_is_refused_naming_the_install_url() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) = spawn_forge(&[("acme/kb", mock)], &[], &[(1, "ghs_test")]).await;
    let eng = engine(tmp.path(), &url, &app_env()).await;
    let root = tmp.path().join("kb");

    let err = eng
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(root.to_str().unwrap()),
            Some(Readers::Shared),
        )
        .await
        .unwrap_err();
    let install_url = format!("{url}/apps/{SLUG}/installations/new");
    assert!(
        matches!(
            &err,
            EngineError::Remote(RemoteError::AppNotInstalled { repo, install_url: u })
                if repo == "acme/kb" && *u == install_url
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains(&install_url), "{err}");

    let hits = forge.hits();
    assert!(
        hits.iter().all(|h| h.auth != bearer(INSTANCE_TOKEN)),
        "no request fell back to the instance token: {hits:?}"
    );
    assert!(
        !hits.iter().any(Hit::is_repo_call),
        "nothing was read from the repository: {hits:?}"
    );
    assert!(forge.mints().is_empty());
}

#[tokio::test]
async fn origin_status_says_the_app_is_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) =
        spawn_forge(&[("acme/kb", mock)], &[("acme/kb", 1)], &[(1, "ghs_test")]).await;
    let eng = engine(tmp.path(), &url, &app_env()).await;
    connect(&eng, "acme/kb", "kb", &tmp.path().join("kb")).await;

    let status = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["app"],
        json!({"installed": true, "install_url": format!("{url}/apps/{SLUG}/installations/new")}),
        "{status}"
    );

    forge.uninstall("acme/kb");
    let status = eng
        .origin_status(Some("kb"), false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["errors"], json!([]), "{status}");
    let domain = &status["domains"][0];
    assert_eq!(
        domain["app"],
        json!({"installed": false, "install_url": format!("{url}/apps/{SLUG}/installations/new")}),
        "{status}"
    );
    assert!(
        domain["probe_error"]
            .as_str()
            .is_some_and(|e| e.contains("/installations/new")),
        "{status}"
    );
    assert!(
        forge
            .hits()
            .iter()
            .all(|h| h.auth != bearer(INSTANCE_TOKEN))
    );
}

#[tokio::test]
async fn a_401_mid_share_renews_the_installation_token_once() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) = spawn_forge(
        &[("acme/kb", mock.clone())],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = engine(tmp.path(), &url, &app_env()).await;
    let root = tmp.path().join("kb");
    connect(&eng, "acme/kb", "kb", &root).await;
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();

    forge.trees_unauthorized.store(1, Ordering::SeqCst);
    let result = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(forge.mints(), vec![1, 1], "one renewal after the 401");
    let proposals = mock
        .calls()
        .iter()
        .filter(|c| c.starts_with("create_proposal:"))
        .count();
    assert_eq!(proposals, 1, "{:?}", mock.calls());
    let trees = forge
        .hits()
        .iter()
        .filter(|h| h.method == "POST" && h.path.ends_with("/git/trees"))
        .count();
    assert_eq!(trees, 2, "the refused tree was written once more");
    assert!(
        forge
            .hits()
            .iter()
            .all(|h| h.auth != bearer(INSTANCE_TOKEN))
    );
}

/// A connected `kb` domain on an App instance with one new engram waiting
/// to be shared.
async fn app_share_setup(tmp: &Path) -> (Arc<Forge>, Engine) {
    let mock = repo_with_manifest("");
    let (forge, url) = spawn_forge(
        &[("acme/kb", mock.clone())],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = engine(tmp, &url, &app_env()).await;
    let root = tmp.join("kb");
    connect(&eng, "acme/kb", "kb", &root).await;
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();
    (forge, eng)
}

#[tokio::test]
async fn an_instance_share_with_the_app_names_the_person_as_author_and_the_bot_as_committer() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng) = app_share_setup(tmp.path()).await;
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    auth.add_user(
        "ada",
        "Ada Lovelace",
        Some("ada@example.com"),
        Role::Editor,
        "pw12345678",
    )
    .await
    .unwrap();
    eng.set_domain_access(Arc::new(DomainAccess::new(auth)));

    let result = eng
        .origin_share(
            "kb",
            None,
            None,
            None,
            None,
            ShareActor::Account("ada".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");

    let commits = forge.commits();
    assert_eq!(commits.len(), 1, "{commits:?}");
    let commit = &commits[0];
    assert_eq!(commit["author"]["name"], "Ada Lovelace", "{commit}");
    assert_eq!(commit["author"]["email"], "ada@example.com", "{commit}");
    assert_eq!(
        commit["committer"]["name"], "crystalline-acme[bot]",
        "{commit}"
    );
    assert_eq!(
        commit["committer"]["email"], "41898282+crystalline-acme[bot]@users.noreply.127.0.0.1",
        "{commit}"
    );
    let users: Vec<Hit> = forge
        .hits()
        .into_iter()
        .filter(|h| h.path.starts_with("/users/"))
        .collect();
    assert_eq!(users.len(), 1, "{users:?}");
    assert_eq!(
        users[0].auth,
        bearer("ghs_test"),
        "the bot is looked up on the installation token"
    );
    // The environment names no slug, so the share reads it before it records
    // whose proposal this is.
    let state = OriginState::load(&tmp.path().join("origins/kb"))
        .unwrap()
        .unwrap();
    assert_eq!(
        state.proposals[0].author_login.as_deref(),
        Some("crystalline-acme[bot]")
    );
}

#[tokio::test]
async fn an_owner_share_with_the_app_is_bot_authored() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng) = app_share_setup(tmp.path()).await;

    let result = eng
        .origin_share("kb", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");

    let commits = forge.commits();
    assert_eq!(commits.len(), 1, "{commits:?}");
    let keys: Vec<&String> = commits[0].as_object().unwrap().keys().collect();
    assert!(
        !keys.iter().any(|k| *k == "author" || *k == "committer"),
        "the owner's share names nobody, so the bot authors it: {keys:?}"
    );
    let state = OriginState::load(&tmp.path().join("origins/kb"))
        .unwrap()
        .unwrap();
    assert_eq!(
        state.proposals[0].author_login.as_deref(),
        Some("crystalline-acme[bot]"),
        "the slug is read at the share entry, so the first share records the bot"
    );
}

#[tokio::test]
async fn the_backup_tick_uses_one_token_per_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let reflection = format!("reflection: on\nreflection_home: {HOME}\n");
    let acme = repo_with_manifest(&reflection);
    let beta = repo_with_manifest(&reflection);
    let (forge, url) = spawn_forge(
        &[("acme/kb", acme.clone()), ("beta/kb", beta.clone())],
        &[("acme/kb", 11), ("beta/kb", 22)],
        &[(11, "ghs_eleven"), (22, "ghs_twentytwo")],
    )
    .await;
    let eng = engine(tmp.path(), &url, &app_env()).await;
    let roots: Vec<(&str, &str, PathBuf)> = vec![
        ("acme/kb", "acme", tmp.path().join("acme")),
        ("beta/kb", "beta", tmp.path().join("beta")),
    ];
    for (repo, name, root) in &roots {
        connect(&eng, repo, name, root).await;
        std::fs::create_dir_all(root.join("reflections")).unwrap();
        std::fs::write(root.join("reflections/a.md"), "# A\n\nText.\n").unwrap();
        eng.mark_reflection_dirty(name, BackupCause::Settled);
    }

    let results = eng
        .reflection_backup_tick(Instant::now() + Duration::from_secs(120))
        .await;
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
    assert!(acme.branch_commit(BACKUP_BRANCH).is_some());
    assert!(beta.branch_commit(BACKUP_BRANCH).is_some());

    for (repo, token) in [("acme/kb", "ghs_eleven"), ("beta/kb", "ghs_twentytwo")] {
        let auths = repo_auths(&forge, repo);
        assert!(
            auths.iter().all(|a| *a == bearer(token)),
            "{repo} only ever saw its own installation's token: {auths:?}"
        );
        let wrote = forge.hits().into_iter().any(|h| {
            h.repo().as_deref() == Some(repo)
                && h.method == "POST"
                && h.path.ends_with("/git/commits")
        });
        assert!(wrote, "{repo} got its backup commit");
    }
    assert!(
        forge
            .commits()
            .iter()
            .all(|c| c.get("author").is_none() && c.get("committer").is_none()),
        "the Reflection backup names nobody, so the bot authors it: {:?}",
        forge.commits()
    );
    let mut mints = forge.mints();
    mints.sort_unstable();
    assert_eq!(mints, vec![11, 22], "one token per installation, reused");
}

/// App credentials as `github app set` saves them, for `host`.
fn saved_app(host: &str) -> AppCredentials {
    AppCredentials {
        app_id: 7,
        client_id: "Iv23.test".to_string(),
        client_secret: None,
        private_key: PEM.to_string(),
        webhook_secret: None,
        slug: Some(SLUG.to_string()),
        host: host.to_string(),
    }
}

#[tokio::test]
async fn a_saved_app_is_used_when_the_environment_names_none() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) =
        spawn_forge(&[("acme/kb", mock)], &[("acme/kb", 1)], &[(1, "ghs_test")]).await;
    let host = url.trim_start_matches("http://");
    AppStore::file_fallback(&tmp.path().join("tokens"))
        .save(&saved_app(host))
        .unwrap();
    let eng = engine(
        tmp.path(),
        &url,
        &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
    )
    .await;
    connect(&eng, "acme/kb", "kb", &tmp.path().join("kb")).await;

    assert!(eng.github_app().is_some());
    let auths = repo_auths(&forge, "acme/kb");
    assert!(!auths.is_empty());
    assert!(auths.iter().all(|a| *a == bearer("ghs_test")), "{auths:?}");
    assert!(
        !forge.hits().iter().any(|h| h.path == "/app"),
        "the saved slug names the App without asking GitHub"
    );
}

#[tokio::test]
async fn a_saved_app_for_another_host_is_refused_and_nothing_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) =
        spawn_forge(&[("acme/kb", mock)], &[("acme/kb", 1)], &[(1, "ghs_test")]).await;
    AppStore::file_fallback(&tmp.path().join("tokens"))
        .save(&saved_app("github.com"))
        .unwrap();
    let eng = engine(
        tmp.path(),
        &url,
        &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
    )
    .await;

    let err = eng
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(tmp.path().join("kb").to_str().unwrap()),
            Some(Readers::Shared),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::Remote(RemoteError::AppCredential(detail)) if detail.contains("github.com")),
        "{err:?}"
    );
    assert!(eng.github_app().is_none());
    assert!(
        forge.hits().is_empty(),
        "nothing went out, on the instance token least of all: {:?}",
        forge.hits()
    );

    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let error = status["connection"]["app"]["error"]
        .as_str()
        .unwrap_or_default();
    assert!(error.contains("github.com"), "{status}");
}

/// A clock an engine reads for its App memory, moved by the test instead of
/// by waiting.
#[derive(Clone)]
struct StepClock(Arc<StdMutex<Instant>>);

impl StepClock {
    fn new() -> StepClock {
        StepClock(Arc::new(StdMutex::new(Instant::now())))
    }

    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }

    fn install(&self, eng: Engine) -> Engine {
        let at = self.0.clone();
        eng.with_app_clock(Arc::new(move || *at.lock().unwrap()))
    }
}

/// A lookup that answers like a keychain that failed for the first
/// `failures` reads (with `marker` beside the empty file) and then holds
/// `saved`, counting every read.
fn flaky_keychain(
    file_dir: PathBuf,
    failures: usize,
    marker: Option<String>,
    saved: Option<AppCredentials>,
    reads: Arc<AtomicUsize>,
) -> crystalline_service::engine::AppLookupFn {
    Arc::new(move || {
        let failed = reads.fetch_add(1, Ordering::SeqCst) < failures;
        Ok(AppLookup {
            store: AppStore::file_fallback(&file_dir),
            credentials: if failed { None } else { saved.clone() },
            keychain_failed: failed.then(|| "the keychain is locked".to_string()),
            marker: if failed { marker.clone() } else { None },
        })
    })
}

#[tokio::test]
async fn a_keychain_that_failed_to_answer_is_asked_again_and_not_cached_as_no_app() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) = spawn_forge(
        &[("acme/kb", mock.clone())],
        &[("acme/kb", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let host = url.trim_start_matches("http://").to_string();
    let lookups = Arc::new(AtomicUsize::new(0));
    let clock = StepClock::new();
    // The keychain holds the App but is locked for the first read: the file
    // fallback answers with nothing, and no marker says an App is saved
    // there. From the second read on it answers.
    let eng = clock.install(
        engine(
            tmp.path(),
            &url,
            &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
        )
        .await
        .with_app_store_lookup(flaky_keychain(
            tmp.path().join("tokens"),
            1,
            None,
            Some(saved_app(&host)),
            lookups.clone(),
        )),
    );
    let root = tmp.path().join("kb");
    // The one call the failed read answered: it cannot be told apart from an
    // instance without an App, so it runs as one.
    connect(&eng, "acme/kb", "kb", &root).await;
    let before = forge.hits().len();

    // The failure is remembered for a minute, so the keychain is asked
    // again only once that minute is over.
    clock.advance(Duration::from_secs(61));
    let head = mock.add_commit(files(&[
        ("MANIFEST.md", manifest("")),
        ("notes/new.md", engram("New", "new", "added upstream")),
    ]));
    mock.set_branch("main", &head);
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(root.join("notes/new.md").exists());

    assert_eq!(lookups.load(Ordering::SeqCst), 2, "asked again, then kept");
    assert!(eng.github_app().is_some());
    let after: Vec<Hit> = forge.hits().into_iter().skip(before).collect();
    assert!(after.iter().any(Hit::is_repo_call));
    assert!(
        after
            .iter()
            .filter(|h| h.is_repo_call())
            .all(|h| h.auth == bearer("ghs_test")),
        "once the keychain answered, every call went out on the installation token: {after:?}"
    );
}

#[tokio::test]
async fn a_failed_keychain_read_is_remembered_for_a_minute() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (_forge, url) =
        spawn_forge(&[("acme/kb", mock)], &[("acme/kb", 1)], &[(1, "ghs_test")]).await;
    let reads = Arc::new(AtomicUsize::new(0));
    let clock = StepClock::new();
    let eng = clock.install(
        engine(
            tmp.path(),
            &url,
            &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
        )
        .await
        .with_app_store_lookup(flaky_keychain(
            tmp.path().join("tokens"),
            usize::MAX,
            None,
            None,
            reads.clone(),
        )),
    );
    let root = tmp.path().join("kb");
    connect(&eng, "acme/kb", "kb", &root).await;
    for _ in 0..3 {
        eng.origin_update(Some("kb"), &Scope::Unrestricted)
            .await
            .unwrap();
    }
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "a wedged keychain is asked once a minute, not on every operation"
    );

    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        status["connection"].get("app").is_none(),
        "without a marker nothing is known to be saved there, so the status is today's: {status}"
    );
    let configure = eng.configure_snapshot().await.unwrap();
    assert_eq!(configure["github"]["app"]["configured"], json!(false));
    assert_eq!(
        configure["github"]["app"]["error"],
        Value::Null,
        "{configure}"
    );

    clock.advance(Duration::from_secs(59));
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1, "still inside the minute");
    clock.advance(Duration::from_secs(2));
    eng.origin_update(Some("kb"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 2, "asked again after it");
}

#[tokio::test]
async fn a_failed_keychain_read_with_the_marker_refuses_and_never_uses_the_instance_token() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) =
        spawn_forge(&[("acme/kb", mock)], &[("acme/kb", 1)], &[(1, "ghs_test")]).await;
    let host = url.trim_start_matches("http://").to_string();
    let reads = Arc::new(AtomicUsize::new(0));
    let clock = StepClock::new();
    let eng = clock.install(
        engine(
            tmp.path(),
            &url,
            &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
        )
        .await
        .with_app_store_lookup(flaky_keychain(
            tmp.path().join("tokens"),
            2,
            Some(host.clone()),
            Some(saved_app(&host)),
            reads.clone(),
        )),
    );
    let root = tmp.path().join("kb");

    let err = eng
        .origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(root.to_str().unwrap()),
            Some(Readers::Shared),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "the GitHub App is saved in the keychain, which could not be read; try again or unlock it, or run crystalline github app remove"
    );
    assert!(
        forge.hits().is_empty(),
        "nothing went out, on the instance token least of all: {:?}",
        forge.hits()
    );
    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["connection"]["app"]["keychain_unreadable"],
        json!("the keychain is locked"),
        "{status}"
    );
    assert_eq!(
        status["connection"]["app"]["error"],
        json!(
            "the GitHub App is saved in the keychain, which could not be read; try again or unlock it, or run crystalline github app remove"
        ),
        "{status}"
    );

    // The next read after the minute fails too and still refuses; the one
    // after that answers and the App is used.
    clock.advance(Duration::from_secs(61));
    assert!(
        eng.origin_add(
            "acme/kb",
            Some("kb"),
            None,
            None,
            Some(root.to_str().unwrap()),
            Some(Readers::Shared),
        )
        .await
        .is_err()
    );
    clock.advance(Duration::from_secs(61));
    connect(&eng, "acme/kb", "kb", &root).await;
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    assert!(
        forge
            .hits()
            .iter()
            .filter(|h| h.is_repo_call())
            .all(|h| h.auth == bearer("ghs_test")),
        "{:?}",
        forge.hits()
    );
}

#[tokio::test]
async fn a_marker_for_another_host_does_not_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = repo_with_manifest("");
    let (forge, url) =
        spawn_forge(&[("acme/kb", mock)], &[("acme/kb", 1)], &[(1, "ghs_test")]).await;
    let eng = engine(
        tmp.path(),
        &url,
        &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
    )
    .await
    .with_app_store_lookup(flaky_keychain(
        tmp.path().join("tokens"),
        usize::MAX,
        Some("ghe.other.example".to_string()),
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    connect(&eng, "acme/kb", "kb", &tmp.path().join("kb")).await;
    assert!(
        repo_auths(&forge, "acme/kb")
            .iter()
            .all(|a| *a == bearer(INSTANCE_TOKEN))
    );
}

#[tokio::test]
async fn the_app_slot_follows_a_changed_api_url() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(&[], &[], &[]).await;
    let (_other, other_url) = spawn_forge(&[], &[], &[]).await;
    let host = url.trim_start_matches("http://");
    AppStore::file_fallback(&tmp.path().join("tokens"))
        .save(&saved_app(host))
        .unwrap();
    let eng = engine(tmp.path(), &url, &[]).await;
    assert!(eng.github_app().is_some());

    eng.configure(&ConfigureAction::Set {
        key: "github.api_url".to_string(),
        value: other_url.clone(),
    })
    .await
    .unwrap();
    assert!(
        eng.github_app().is_none(),
        "the App saved for the old host is not kept for the new one"
    );

    eng.configure(&ConfigureAction::Set {
        key: "github.api_url".to_string(),
        value: url.clone(),
    })
    .await
    .unwrap();
    assert!(eng.github_app().is_some(), "and is found again for its own");
}

#[tokio::test]
async fn an_instance_with_only_an_app_reports_connected() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(&[], &[], &[]).await;
    let env: Vec<_> = app_env()
        .into_iter()
        .filter(|(k, _)| *k != "CRYSTALLINE_GITHUB_TOKEN")
        .collect();
    let eng = engine(tmp.path(), &url, &env).await;
    let status = eng
        .origin_status(None, false, false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["connection"]["connected"], json!(true), "{status}");
    let configure = eng.configure_snapshot().await.unwrap();
    assert_eq!(configure["github"]["connected"], json!(true), "{configure}");
    let settings = eng.github_connection().await.unwrap();
    assert!(settings.connected, "{settings:?}");
    assert_eq!(settings.token_store.as_deref(), Some("GitHub App"));
}

/// The App credentials `github app set` hands the engine: no slug, which
/// the validation reads from GitHub.
fn app_to_set() -> AppCredentials {
    AppCredentials {
        app_id: 7,
        client_id: "Iv23.test".to_string(),
        client_secret: Some("CLIENTSECRETVALUE".to_string()),
        private_key: PEM.to_string(),
        webhook_secret: Some("WEBHOOKSECRETVALUE".to_string()),
        slug: None,
        host: String::new(),
    }
}

#[tokio::test]
async fn set_github_app_validates_saves_and_swaps_the_app_and_remove_drops_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = spawn_forge(&[], &[], &[]).await;
    let eng = engine(
        tmp.path(),
        &url,
        &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
    )
    .await;
    assert!(eng.github_app().is_none(), "no App yet, and that is kept");

    let status = eng.set_github_app(app_to_set()).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.slug.as_deref(), Some(SLUG), "read from GitHub");
    assert_eq!(status.app_id, Some(7));
    assert!(status.client_secret_set && status.webhook_secret_set);
    assert!(forge.hits().iter().any(|h| h.path == "/app"), "validated");
    let saved = AppStore::file_fallback(&tmp.path().join("tokens"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.slug.as_deref(), Some(SLUG));
    assert_eq!(saved.host, url.trim_start_matches("http://"));
    assert!(eng.github_app().is_some(), "the slot took the new App");

    eng.remove_github_app().await.unwrap();
    assert!(eng.github_app().is_none());
    assert_eq!(
        AppStore::file_fallback(&tmp.path().join("tokens"))
            .load()
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn set_github_app_refuses_an_app_github_does_not_know_by_that_id() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = spawn_forge(&[], &[], &[]).await;
    let eng = engine(tmp.path(), &url, &[]).await;
    let mut wrong = app_to_set();
    wrong.app_id = 8;
    let err = eng.set_github_app(wrong).await.unwrap_err().to_string();
    assert!(err.contains('7') && err.contains('8'), "{err}");
    assert_eq!(
        AppStore::file_fallback(&tmp.path().join("tokens"))
            .load()
            .unwrap(),
        None,
        "nothing saved"
    );
}

#[tokio::test]
async fn set_and_remove_refuse_while_the_environment_supplies_the_app() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = spawn_forge(&[], &[], &[]).await;
    let eng = engine(tmp.path(), &url, &app_env()).await;
    let err = eng.set_github_app(app_to_set()).await.unwrap_err();
    assert!(err.to_string().contains("CRYSTALLINE_GITHUB_APP_"), "{err}");
    let err = eng.remove_github_app().await.unwrap_err();
    assert!(err.to_string().contains("CRYSTALLINE_GITHUB_APP_"), "{err}");
    assert!(forge.hits().is_empty(), "refused before asking GitHub");
}

/// An `AppHome` whose keychain cannot be read, with the marker for `host`
/// beside the file store when `marker` is set, and a keychain delete that
/// answers `delete_works`.
fn unreadable_home(dir: &Path, host: &str, marker: bool, delete_works: bool) -> AppHome {
    if marker {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("github-app.present"), format!("{host}\n")).unwrap();
    }
    let file_dir = dir.to_path_buf();
    let marker = marker.then(|| host.to_string());
    AppHome::new(Some(format!("http://{host}")), dir.to_path_buf(), false).with_keychain(
        AppKeychainSeam {
            lookup: Arc::new(move || {
                let store = AppStore::file_fallback(&file_dir);
                Ok(AppLookup {
                    credentials: store.load()?,
                    store,
                    keychain_failed: Some("no keychain daemon".to_string()),
                    marker: marker.clone(),
                })
            }),
            delete: Arc::new(move || {
                if delete_works {
                    Ok(())
                } else {
                    Err(RemoteError::Credential {
                        detail: "no keychain daemon".to_string(),
                    })
                }
            }),
        },
    )
}

fn no_env() -> EnvOverlay {
    EnvOverlay::from_vars(Vec::<(String, String)>::new()).unwrap()
}

#[tokio::test]
async fn remove_on_an_unreadable_keychain_drops_the_file_copy_and_the_marker() {
    for marker in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tokens");
        let host = "127.0.0.1:9";
        let home = unreadable_home(&dir, host, marker, false);
        AppStore::file_fallback(&dir)
            .save(&saved_app(host))
            .unwrap();

        let removal = home.remove(&no_env()).unwrap();
        assert_eq!(
            removal.keychain_unreadable.as_deref(),
            Some("no keychain daemon"),
            "the answer says a keychain item may remain"
        );
        assert!(
            !dir.join("github-app.json").exists(),
            "the file copy is gone"
        );
        assert!(
            !dir.join("github-app.present").exists(),
            "the marker is gone, so origin operations are no longer refused"
        );
    }
}

#[tokio::test]
async fn remove_on_an_unreadable_keychain_that_takes_the_delete_says_nothing_remains() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("tokens");
    let home = unreadable_home(&dir, "127.0.0.1:9", true, true);
    assert_eq!(home.remove(&no_env()).unwrap().keychain_unreadable, None);
    assert!(!dir.join("github-app.present").exists());
}

#[tokio::test]
async fn set_behind_the_marker_on_an_unreadable_keychain_is_refused_before_github_is_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = spawn_forge(&[], &[], &[]).await;
    let host = url.trim_start_matches("http://");
    let dir = tmp.path().join("tokens");
    let home = unreadable_home(&dir, host, true, false);

    let err = home.save(&no_env(), app_to_set()).await.unwrap_err();
    assert!(
        err.to_string().contains("keychain could not be read"),
        "{err}"
    );
    assert!(
        err.to_string().contains("crystalline github app remove"),
        "the way out is named: {err}"
    );
    assert!(
        !dir.join("github-app.json").exists(),
        "nothing saved to the file"
    );
    assert!(forge.hits().is_empty(), "{:?}", forge.hits());
}
