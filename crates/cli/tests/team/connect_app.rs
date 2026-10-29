//! `crystalline connect github --personal` on an instance that uses a GitHub
//! App: the person signs in through the App's device flow, the token lands
//! under their personal slot, and `--as` links the account.
//!
//! The binary runs as a child process, so the seams are the ones that reach
//! it: the `CRYSTALLINE_GITHUB_APP_*` variables configure the App,
//! `CRYSTALLINE_TEST_TOKEN_STORE_DIR` keeps every token in a file under a
//! tempdir (no keychain is read or written), and `github.api_url` points the
//! device flow, `GET /user` and the revoke at a stand-in GitHub this file
//! starts on a loopback port.

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use serde_json::{Value, json};

use crate::common::{isolate, isolated_state_dir};

/// The throwaway test key of the remote crate's App tests; it belongs to no
/// App.
const PEM: &str = include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem");
const APP_CLIENT_ID: &str = "Iv23liAPPCLIENT";
const OAUTH_CLIENT_ID: &str = "Ov23liOAUTHCLIENT";
const GITHUB_ID: u64 = 4242;
const LOGIN: &str = "ada-gh";

/// What the stand-in GitHub was asked, and how it answers.
#[derive(Default)]
struct Seen {
    /// Every `POST /login/device/code` body.
    device_codes: Vec<Value>,
    /// Every revoke: the token in the body.
    revoked: Vec<String>,
    /// Answer the device code request with `device_flow_disabled`.
    device_flow_disabled: bool,
    /// Answer a revoke with a server error.
    revoke_fails: bool,
}

type Shared = Arc<Mutex<Seen>>;

/// A stand-in GitHub on a loopback port, served from a runtime of its own
/// for as long as the test holds it.
struct FakeGithub {
    url: String,
    seen: Shared,
    _runtime: tokio::runtime::Runtime,
}

impl FakeGithub {
    fn start() -> FakeGithub {
        let seen: Shared = Arc::default();
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let router = Router::new()
            .route("/login/device/code", post(device_code))
            .route("/login/oauth/access_token", post(access_token))
            .route("/user", get(user))
            .route("/applications/{client_id}/token", delete(revoke))
            .with_state(seen.clone());
        runtime.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(listener, router).await.unwrap();
        });
        FakeGithub {
            url,
            seen,
            _runtime: runtime,
        }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap()
    }
}

async fn device_code(State(seen): State<Shared>, body: String) -> axum::response::Response {
    let mut seen = seen.lock().unwrap();
    seen.device_codes
        .push(serde_json::from_str(&body).unwrap_or(Value::Null));
    if seen.device_flow_disabled {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({
                "error": "device_flow_disabled",
                "error_description": "Device Flow must be explicitly enabled for this App",
            })),
        )
            .into_response();
    }
    axum::Json(json!({
        "device_code": "dc-1",
        "user_code": "WDJB-MJHT",
        "verification_uri": "https://github.com/login/device",
        "expires_in": 900,
        "interval": 0,
    }))
    .into_response()
}

async fn access_token() -> axum::Json<Value> {
    axum::Json(json!({
        "access_token": "ghu_app",
        "token_type": "bearer",
        "expires_in": 28800,
        "refresh_token": "ghr_app",
        "refresh_token_expires_in": 15897600,
    }))
}

async fn user() -> axum::Json<Value> {
    axum::Json(json!({ "id": GITHUB_ID, "login": LOGIN }))
}

async fn revoke(State(seen): State<Shared>, body: String) -> StatusCode {
    let mut seen = seen.lock().unwrap();
    let token: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    seen.revoked.push(
        token["access_token"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    if seen.revoke_fails {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::NO_CONTENT
    }
}

/// One test's world: an isolated home, a token directory and a config that
/// shares personally against the stand-in GitHub.
struct World {
    home: tempfile::TempDir,
    _work: tempfile::TempDir,
    tokens: PathBuf,
    config: PathBuf,
    github: FakeGithub,
}

impl World {
    fn new() -> World {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let tokens = work.path().join("tokens");
        std::fs::create_dir_all(&tokens).unwrap();
        let github = FakeGithub::start();
        let config = work.path().join("config.yaml");
        std::fs::write(
            &config,
            format!(
                "github:\n  enabled: true\n  share_identity: personal\n  oauth_client_id: {OAUTH_CLIENT_ID}\n  api_url: \"{}\"\n",
                github.url
            ),
        )
        .unwrap();
        World {
            home,
            _work: work,
            tokens,
            config,
            github,
        }
    }

    /// A `crystalline` child in this world, without the App.
    fn bare(&self) -> Command {
        let mut cmd = crate::common::crystalline();
        isolate(&mut cmd, self.home.path());
        cmd.env("CRYSTALLINE_TEST_TOKEN_STORE_DIR", &self.tokens);
        cmd
    }

    /// A `crystalline` child in this world, with the App in the environment.
    fn bin(&self) -> Command {
        let mut cmd = self.bare();
        cmd.env("CRYSTALLINE_GITHUB_APP_ID", "7")
            .env("CRYSTALLINE_GITHUB_APP_CLIENT_ID", APP_CLIENT_ID)
            .env("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", PEM)
            .env("CRYSTALLINE_GITHUB_APP_CLIENT_SECRET", "CLIENTSECRETVALUE");
        cmd
    }

    /// `connect github --personal` plus `args`, with the App.
    fn connect(&self, args: &[&str]) -> std::process::Output {
        self.bin()
            .args(["connect", "github", "--personal"])
            .args(args)
            .arg("--config")
            .arg(&self.config)
            .output()
            .unwrap()
    }

    fn token_file(&self, name: &str) -> PathBuf {
        self.tokens
            .join(format!("github-token-personal-{name}.json"))
    }

    fn add_account(&self, name: &str) {
        let out = self
            .bare()
            .args(["users", "add", name, "--mcp-token", "cli"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    fn links(&self) -> Vec<(String, String, u64, String)> {
        links_in(&isolated_state_dir(self.home.path()).join("web-auth.db"))
    }
}

/// Every GitHub link in the auth database at `db`, as `(account, host, id,
/// login)`.
fn links_in(db: &Path) -> Vec<(String, String, u64, String)> {
    if !db.exists() {
        return Vec::new();
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = crystalline_service::rest::AuthStore::open(db)
            .await
            .unwrap();
        store
            .github_links()
            .await
            .unwrap()
            .into_iter()
            .map(|l| (l.account, l.host, l.github_id, l.login))
            .collect()
    })
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn connect_personal_with_an_app_uses_the_app_client_id_and_no_scope() {
    let world = World::new();
    let out = world.connect(&[]);
    assert!(out.status.success(), "{out:?}");

    let codes = world.github.seen().device_codes.clone();
    assert_eq!(codes.len(), 1, "{codes:?}");
    assert_eq!(codes[0]["client_id"], APP_CLIENT_ID, "never the OAuth id");
    assert!(
        codes[0].get("scope").is_none(),
        "a GitHub App has no scopes: {}",
        codes[0]
    );

    // The whole grant is kept, so a share can renew the token.
    let saved = read_json(&world.token_file("owner"));
    assert_eq!(saved["access_token"], "ghu_app");
    assert_eq!(saved["refresh_token"], "ghr_app");
    assert!(saved["expires_at"].is_string(), "{saved}");
    assert!(saved["refresh_expires_at"].is_string(), "{saved}");
    assert_eq!(saved["user"], LOGIN);
    let stdout = text(&out.stdout);
    assert!(stdout.contains(LOGIN), "{stdout}");
}

#[test]
fn connect_personal_as_an_account_writes_its_link() {
    let world = World::new();
    world.add_account("ada");
    let out = world.connect(&["--as", "ada"]);
    assert!(out.status.success(), "{out:?}");

    // The App's host: the bare host of `github.api_url`, port included.
    let host = world.github.url.trim_start_matches("http://").to_string();
    assert_eq!(
        world.links(),
        vec![("ada".to_string(), host, GITHUB_ID, LOGIN.to_string())]
    );
    let saved = read_json(&world.token_file("ada"));
    assert_eq!(saved["access_token"], "ghu_app");
    assert_eq!(saved["refresh_token"], "ghr_app");

    // The disconnect revokes the token at GitHub, removes the link and the
    // token.
    let out = world.connect(&["--as", "ada", "--disconnect"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(world.github.seen().revoked, vec!["ghu_app".to_string()]);
    assert!(world.links().is_empty(), "the link is gone");
    assert!(!world.token_file("ada").exists(), "the token is gone");
}

#[test]
fn connect_personal_as_owner_writes_no_link() {
    let world = World::new();
    world.add_account("ada");
    let out = world.connect(&[]);
    assert!(out.status.success(), "{out:?}");
    assert!(world.token_file("owner").exists());
    assert!(world.links().is_empty(), "the owner has no account to link");

    // The owner's disconnect still revokes the App token it holds.
    let out = world.connect(&["--disconnect"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(world.github.seen().revoked, vec!["ghu_app".to_string()]);
    assert!(!world.token_file("owner").exists());
}

#[test]
fn device_flow_disabled_names_the_setting() {
    let world = World::new();
    world.github.seen().device_flow_disabled = true;
    let out = world.connect(&[]);
    assert!(!out.status.success(), "{out:?}");
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("enable Device Flow in the GitHub App's settings"),
        "{stderr}"
    );
    assert!(!world.token_file("owner").exists());
}

#[test]
fn a_pasted_personal_token_is_refused_with_an_app() {
    let world = World::new();
    let out = world.connect(&["--token", "ghp_pasted"]);
    assert!(!out.status.success(), "{out:?}");
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("this instance uses a GitHub App: connect GitHub from your profile"),
        "{stderr}"
    );
    assert!(world.github.seen().device_codes.is_empty());
    assert!(!world.token_file("owner").exists());
}

#[test]
fn an_unknown_account_is_refused_before_the_device_flow() {
    let world = World::new();
    let out = world.connect(&["--as", "nobody"]);
    assert!(!out.status.success(), "{out:?}");
    let stderr = text(&out.stderr);
    assert!(stderr.contains("nobody"), "{stderr}");
    assert!(
        world.github.seen().device_codes.is_empty(),
        "no sign-in for an account that cannot be linked"
    );
}

#[test]
fn a_disconnect_github_could_not_revoke_says_so() {
    let world = World::new();
    world.add_account("ada");
    assert!(world.connect(&["--as", "ada"]).status.success());
    world.github.seen().revoke_fails = true;
    let out = world.connect(&["--as", "ada", "--disconnect"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("GitHub could not be reached to revoke the token"),
        "{stdout}"
    );
    assert!(world.links().is_empty());
    assert!(!world.token_file("ada").exists());
}

#[test]
fn without_an_app_the_personal_connect_uses_the_oauth_client() {
    let world = World::new();
    // The OAuth device flow, which validates the token with `GET /user`.
    let out = world
        .bare()
        .args(["connect", "github", "--personal", "--config"])
        .arg(&world.config)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let codes = world.github.seen().device_codes.clone();
    assert_eq!(codes[0]["client_id"], OAUTH_CLIENT_ID);
    assert_eq!(codes[0]["scope"], "repo");
}
