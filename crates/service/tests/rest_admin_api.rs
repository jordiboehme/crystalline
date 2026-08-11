//! Endpoint tests for the admin settings surface: the GitHub connection
//! status and its device-flow poll, the two connect paths and disconnect.
//!
//! A fresh, smaller fixture rather than a share of `rest_write_api.rs`'s: the
//! engine here is built with the token store pinned into the temp dir and a
//! stub [`support::StubConnectAuth`], so nothing in this suite can read, write
//! or delete the developer's real GitHub credential, and `github.enabled` is
//! a per-test option because the settings screen's own contract is that
//! connecting turns the feature on.

mod support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::engine::ConnectAuth;
use crystalline_service::rest::{AuthStore, Role};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// What an admin-test server varies.
#[derive(Default)]
struct Options {
    /// Serve read-only: every mutation is refused, reads are not.
    read_only: bool,
    /// Start with `github.enabled` already on.
    github: bool,
    /// The connect double this engine authenticates through. Defaults to
    /// `StubConnectAuth::accepting("octo")`, which validates any token and
    /// runs no device flow.
    connect_auth: Option<Arc<dyn ConnectAuth>>,
}

struct Fixture {
    addr: std::net::SocketAddr,
    _tmp: tempfile::TempDir,
}

async fn serve(opts: Options) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        // Pin the domains root inside the temp dir so nothing this suite
        // creates can land in a real home folder.
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig {
            trusted_header: None,
            anonymous: Some(false),
            max_users: None,
        }),
        github: opts.github.then(|| GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
    )
    .unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        read_only: Some(opts.read_only),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let connect_auth: Arc<dyn ConnectAuth> = opts
        .connect_auth
        .unwrap_or_else(|| Arc::new(support::StubConnectAuth::accepting("octo")));
    // Both overrides are load-bearing, not tidiness: without the token-store
    // dir a disconnect here would delete the developer's REAL keychain GitHub
    // token, and without the stub a connect would dial github.com.
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_read_only(opts.read_only)
            .with_token_store_dir(root.join("tokens"))
            .with_connect_auth(connect_auth),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    // One account per role, so every gate has a caller to refuse.
    auth.add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();
    auth.add_user("eddy", "Eddy", None, Role::Editor, "eddypw")
        .await
        .unwrap();
    auth.add_user("vera", "Vera", None, Role::Viewer, "verapw")
        .await
        .unwrap();

    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth.clone()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(listener, router).await.unwrap();
    });
    Fixture { addr, _tmp: tmp }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Log in, returning (session cookie value, csrf token).
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({"name": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login as {name} must succeed");
    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .and_then(|v| v.split(';').next())
        .and_then(|v| v.strip_prefix("fluid_session="))
        .unwrap()
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    (cookie, body["csrf"].as_str().unwrap().to_string())
}

/// A request carrying a session and its token, the browser shape.
fn as_session(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    session: &(String, String),
) -> reqwest::RequestBuilder {
    client()
        .request(method, format!("http://{addr}{path}"))
        .header("cookie", format!("fluid_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

/// The admin gate on the whole settings surface: viewer and editor are 403
/// on the GET too (the write matrix only covers unsafe methods). read_only
/// serves the status read - a pure read - and refuses the mutations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn github_settings_are_admin_only_and_read_only_refuses_mutations() {
    let fx = serve(Options {
        github: true,
        ..Options::default()
    })
    .await;
    for (name, pw) in [("eddy", "eddypw"), ("vera", "verapw")] {
        let session = login(fx.addr, name, pw).await;
        let resp = as_session(
            fx.addr,
            reqwest::Method::GET,
            "/api/v1/settings/github",
            &session,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), 403, "{name} must not read the connection");
    }
    let ro = serve(Options {
        read_only: true,
        github: true,
        ..Options::default()
    })
    .await;
    let admin = login(ro.addr, "root", "rootpw").await;
    let resp = as_session(
        ro.addr,
        reqwest::Method::GET,
        "/api/v1/settings/github",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "the status is a pure read; read_only serves it"
    );
    let resp = as_session(
        ro.addr,
        reqwest::Method::POST,
        "/api/v1/settings/github/connect",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 403, "read_only refuses the mutations");
}

/// The other two mutations under read_only, at the route level rather than by
/// analogy: a PAT connect and a disconnect are refused by this layer before
/// the engine is asked, so a read-only instance can neither acquire nor
/// forget a credential over HTTP.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_refuses_the_token_and_disconnect_routes() {
    let ro = serve(Options {
        read_only: true,
        github: true,
        ..Options::default()
    })
    .await;
    let admin = login(ro.addr, "root", "rootpw").await;

    let stored = as_session(
        ro.addr,
        reqwest::Method::POST,
        "/api/v1/settings/github/token",
        &admin,
    )
    .json(&serde_json::json!({"token": "pat-secret-123"}))
    .send()
    .await
    .unwrap();
    assert_eq!(stored.status(), 403, "read_only stores no credential");

    let gone = as_session(
        ro.addr,
        reqwest::Method::DELETE,
        "/api/v1/settings/github",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(gone.status(), 403, "read_only forgets none either");
}

/// PAT connect end to end: the status flips to connected, names the account
/// and the store kind, and the token appears in no response body. Disconnect
/// flips it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pat_connects_and_disconnect_forgets_it() {
    let fx = serve(Options {
        github: true,
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let before = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/settings/github",
        &admin,
    )
    .send()
    .await
    .unwrap();
    let before: serde_json::Value = before.json().await.unwrap();
    assert_eq!(before["connected"], false);

    let connected = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/settings/github/token",
        &admin,
    )
    .json(&serde_json::json!({"token": "pat-secret-123"}))
    .send()
    .await
    .unwrap();
    assert_eq!(connected.status(), 200);
    let body = connected.text().await.unwrap();
    assert!(
        !body.contains("pat-secret-123"),
        "write-only means write-only: {body}"
    );
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["connected"], true);
    assert_eq!(body["user"], "octo");
    assert_eq!(body["token_store"], "file");
    assert_eq!(
        body["enabled"], true,
        "the Connect intent enables the feature"
    );

    let gone = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/settings/github",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(gone.status(), 200);
    let gone: serde_json::Value = gone.json().await.unwrap();
    assert_eq!(gone["connected"], false);
}

/// A PAT connect on an instance where the feature was never turned on: the
/// Connect intent enables it, so the screen never has to ask twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connecting_turns_the_feature_on() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let before = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/settings/github",
        &admin,
    )
    .send()
    .await
    .unwrap();
    let before: serde_json::Value = before.json().await.unwrap();
    assert_eq!(before["enabled"], false, "off until someone connects");

    let connected = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/settings/github/token",
        &admin,
    )
    .json(&serde_json::json!({"token": "pat-secret-123"}))
    .send()
    .await
    .unwrap();
    assert_eq!(connected.status(), 200);
    let connected: serde_json::Value = connected.json().await.unwrap();
    assert_eq!(connected["enabled"], true);
    assert_eq!(connected["connected"], true);
}

/// An empty token is refused before anything is validated or stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_token_is_refused() {
    let fx = serve(Options {
        github: true,
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let resp = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/settings/github/token",
        &admin,
    )
    .json(&serde_json::json!({"token": "   "}))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 422);

    let after = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/settings/github",
        &admin,
    )
    .send()
    .await
    .unwrap();
    let after: serde_json::Value = after.json().await.unwrap();
    assert_eq!(after["connected"], false, "nothing was stored");
}

/// Disconnecting an instance that was never connected is a success, not a
/// 404: `DELETE` says "make it so there is no connection", and there is not
/// one. The settings screen can therefore offer the button without first
/// having to decide whether it would be legal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnecting_twice_is_a_success_both_times() {
    let fx = serve(Options {
        github: true,
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;

    for round in 0..2 {
        let gone = as_session(
            fx.addr,
            reqwest::Method::DELETE,
            "/api/v1/settings/github",
            &admin,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(gone.status(), 200, "round {round}");
        let gone: serde_json::Value = gone.json().await.unwrap();
        assert_eq!(gone["connected"], false, "round {round}");
        assert!(gone["user"].is_null(), "round {round}");
        assert!(gone["token_store"].is_null(), "round {round}");
    }
}

/// The device flow over REST: 202 with the short code and verification URL,
/// GET is the poll, and a failed flow's error is reported exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_device_flow_polls_over_get_and_reports_failure_once() {
    // The stub's device arm hands out code "ABCD-1234" at
    // https://github.example/device; its background wait BLOCKS on a Notify
    // until the test releases it, then fails with "authorization denied".
    // The gate is load-bearing: start_device_connect SPAWNS the flow task,
    // so an instantly-failing stub could land (and clear) the outcome
    // before the 202 body is even read.
    let (auth, release) = support::StubConnectAuth::denying("authorization denied");
    let fx = serve(Options {
        github: true,
        connect_auth: Some(std::sync::Arc::new(auth)),
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let started = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/settings/github/connect",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(started.status(), 202);
    let started: serde_json::Value = started.json().await.unwrap();
    assert_eq!(started["pending"]["user_code"], "ABCD-1234");
    assert!(
        started["pending"]["verification_url"]
            .as_str()
            .unwrap()
            .starts_with("https://")
    );

    // Only now let the background flow fail.
    release.notify_one();

    // Poll until the background flow lands (the stub fails fast; bound it).
    let mut error = None;
    for _ in 0..50 {
        let poll = as_session(
            fx.addr,
            reqwest::Method::GET,
            "/api/v1/settings/github",
            &admin,
        )
        .send()
        .await
        .unwrap();
        let poll: serde_json::Value = poll.json().await.unwrap();
        if poll["pending"].is_null() {
            error = poll["error"].as_str().map(str::to_string);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let error = error.expect("the flow must end");
    assert!(error.contains("denied"), "{error}");

    // Once-reported: the next poll is plain disconnected, no sticky error.
    let again = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/settings/github",
        &admin,
    )
    .send()
    .await
    .unwrap();
    let again: serde_json::Value = again.json().await.unwrap();
    assert!(again["error"].is_null());
    assert_eq!(again["connected"], false);
}
