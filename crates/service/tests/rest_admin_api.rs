//! Endpoint tests for the admin settings surface: the GitHub connection
//! status and its device-flow poll, the two connect paths and disconnect,
//! plus domain lifecycle - create in all three modes and unregister.
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
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, OriginConfig, ResponseFormat,
    ServiceConfig,
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
    /// The forge a team-domain registration downloads from. Set for the
    /// github-mode tests so nothing here dials github.com; unset means no
    /// override, which is fine for every test that never reaches a provider.
    origin_provider: Option<Arc<support::MockProvider>>,
    /// Register a domain `kb` that carries a GitHub origin in the config
    /// without ever downloading one. The only way to address a team domain on
    /// an instance where `github.enabled` is off, which is the state the sync
    /// endpoints' 409 exists for.
    origin_domain: bool,
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
    if opts.origin_domain {
        let team = root.join("kb");
        std::fs::create_dir_all(&team).unwrap();
        std::fs::write(
            team.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# kb\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- Route here for team questions\n",
        )
        .unwrap();
        cfg.domains.insert(
            "kb".to_string(),
            DomainEntry {
                origin: Some(OriginConfig {
                    repo: "acme/kb".to_string(),
                    path: None,
                    branch: None,
                    poll_secs: None,
                }),
                ..DomainEntry::file(team)
            },
        );
    }
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
    // The origins dir is pinned for the same reason as the token store: a
    // team-domain registration writes origin state, and it must land in the
    // temp dir rather than in the developer's real state directory.
    let mut engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
        .with_read_only(opts.read_only)
        .with_token_store_dir(root.join("tokens"))
        .with_connect_auth(connect_auth)
        .with_origins_dir(root.join("origins"));
    if let Some(provider) = opts.origin_provider {
        engine = engine.with_origin_provider(provider);
    }
    let engine = Arc::new(engine);
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

/// Local create is name-only: the domain lands under the server's domains
/// root, scaffolded and listed; the response says where. Registration
/// refusals are honest statuses: 409 for a taken name, 422 for a name that
/// could escape the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_domain_is_created_under_the_domains_root() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let resp = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "local", "name": "notes"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["domain"], "notes");
    let root = body["root"].as_str().unwrap();
    assert!(
        root.contains("domains-root"),
        "under the pinned root: {root}"
    );
    assert!(std::path::Path::new(root).join("MANIFEST.md").exists());

    // Listed for everyone now.
    let listing = as_session(fx.addr, reqwest::Method::GET, "/api/v1/domains", &admin)
        .send()
        .await
        .unwrap();
    assert!(listing.text().await.unwrap().contains("notes"));

    // Same name again: conflict, not a stack trace.
    let dup = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "virtual", "name": "notes"}))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);

    // Names that could escape or break Windows are refused up front.
    for bad in ["../up", "a/b", "a\\b", "a:b", ".hidden", "a b", "", "   "] {
        let resp = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
            .json(&serde_json::json!({"mode": "local", "name": bad}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 422, "{bad:?} must be refused");
    }
}

/// Virtual create works with a name alone; mode github without a connection
/// is a 409 that points at the settings screen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn virtual_creates_and_disconnected_github_mode_is_a_conflict() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let resp = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "virtual", "name": "scratchpad"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let team = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "github", "repo": "acme/kb"}))
        .send()
        .await
        .unwrap();
    assert_eq!(team.status(), 409);
    let problem: serde_json::Value = team.json().await.unwrap();
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("settings"),
        "the refusal points at the fix: {problem}"
    );

    let nonsense = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "zeppelin", "name": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(nonsense.status(), 422);
}

/// A team domain registers under the name the validator handed back, not the
/// raw one the body carried: `origin_add` uses what it is given verbatim as
/// the config key AND as the folder segment under the domains root, so a
/// padded name would otherwise register a domain called "\tteam " with an
/// on-disk folder to match - the very thing the name check exists to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_team_domain_registers_under_the_trimmed_name() {
    let mock = Arc::new(support::MockProvider::new());
    let commit = mock.add_commit(std::collections::BTreeMap::from([(
        "MANIFEST.md".to_string(),
        b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- Route here for team questions\n".to_vec(),
    )]));
    mock.set_branch("main", &commit);
    let fx = serve(Options {
        github: true,
        origin_provider: Some(mock),
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;

    // A credential has to be on file for `github_ready` to let the mode
    // through; the stub validates any token without leaving the machine.
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

    let created = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "github", "repo": "acme/kb", "name": "\tteam "}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let body: serde_json::Value = created.json().await.unwrap();
    assert_eq!(
        body["domain"], "team",
        "the registered name is the trimmed one"
    );
    let root = body["root"].as_str().unwrap();
    assert!(root.ends_with("team"), "and so is the folder: {root}");
    assert!(
        fx._tmp
            .path()
            .join("domains-root/team/MANIFEST.md")
            .exists(),
        "the download landed under the pinned domains root"
    );

    // And the listing knows it by the trimmed name only.
    let listing = as_session(fx.addr, reqwest::Method::GET, "/api/v1/domains", &admin)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(listing.contains("\"team\""), "{listing}");
    assert!(
        !listing.contains("\\tteam"),
        "no padded name was ever registered: {listing}"
    );
}

/// A team instance: GitHub on, a stub-validated credential already on file
/// and a mock forge serving `acme/kb` at the tracked branch head, so nothing
/// this fixture's tests do - registering, reading status, pulling - ever
/// leaves the machine or touches the developer's real GitHub connection.
async fn serve_team() -> Fixture {
    let mock = Arc::new(support::MockProvider::new());
    let commit = mock.add_commit(std::collections::BTreeMap::from([
        (
            "MANIFEST.md".to_string(),
            b"---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# kb\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- Route here for team questions\n".to_vec(),
        ),
        (
            "shared.md".to_string(),
            b"---\ntype: engram\ntitle: Shared\npermalink: shared\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Shared\n\nA rule the team agreed on.\n".to_vec(),
        ),
    ]));
    mock.set_branch("main", &commit);
    let fx = serve(Options {
        github: true,
        origin_provider: Some(mock),
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;
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
    assert_eq!(
        connected.status(),
        200,
        "the team fixture needs a credential on file before any registration"
    );
    fx
}

/// The team fixture registers acme/kb through POST /domains, and the sync
/// endpoints serve its status and pull updates. Non-team and unknown
/// domains get the honest statuses the spec pins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn team_sync_status_and_sync_now_walk_the_contract() {
    let fx = serve_team().await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let created = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "github", "repo": "acme/kb"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201, "{}", created.text().await.unwrap());

    let status = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/kb/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(status.status(), 200);
    let status: serde_json::Value = status.json().await.unwrap();
    assert_eq!(status["mode"], "github");
    assert_eq!(status["domain"], "kb");
    assert_eq!(status["repo"], "acme/kb");
    assert!(status["branch"].is_string());
    assert!(
        status.get("local_changes").is_some(),
        "the card's pending count: {status}"
    );
    assert!(
        status.get("domains").is_none(),
        "one domain was asked for, so one domain is answered, flat: {status}"
    );

    let pulled = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/kb/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(pulled.status(), 200, "{}", pulled.text().await.unwrap());
    let pulled: serde_json::Value = pulled.json().await.unwrap();
    assert_eq!(pulled["domain"], "kb");
    assert_eq!(
        pulled["up_to_date"], true,
        "nothing moved on the mock forge since the registration: {pulled}"
    );

    // Non-team domain: the status resource does not exist (GET), the action
    // conflicts (POST). Both details name the reason.
    let non_team_get = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(non_team_get.status(), 404);
    let problem: serde_json::Value = non_team_get.json().await.unwrap();
    assert!(
        problem["detail"].as_str().unwrap().contains("team"),
        "the 404 says why, so the UI can tell it from an unknown domain: {problem}"
    );
    let non_team_post = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(non_team_post.status(), 409);
    let problem: serde_json::Value = non_team_post.json().await.unwrap();
    assert!(
        problem["detail"].as_str().unwrap().contains("team"),
        "and so does the conflict: {problem}"
    );

    // Unknown domain: 404 on both.
    let ghost = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/ghost/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(ghost.status(), 404);
    let ghost_post = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/ghost/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(ghost_post.status(), 404, "no such resource to sync either");

    // The GET is admin-only too (the write matrix covers only the POST).
    for (name, pw) in [("eddy", "eddypw"), ("vera", "verapw")] {
        let session = login(fx.addr, name, pw).await;
        let resp = as_session(
            fx.addr,
            reqwest::Method::GET,
            "/api/v1/domains/kb/sync",
            &session,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), 403, "{name} must not read sync status");
    }
}

/// GitHub switched off on an instance that still has a team domain
/// registered: both endpoints answer 409 naming the settings screen, rather
/// than the bare 422 the engine's own NotEnabled would produce - the card
/// shows that sentence in place of its rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_github_names_the_fix_on_both_sync_endpoints() {
    let fx = serve(Options {
        origin_domain: true,
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;

    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        let resp = as_session(fx.addr, method.clone(), "/api/v1/domains/kb/sync", &admin)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 409, "{method} on a disabled instance");
        let problem: serde_json::Value = resp.json().await.unwrap();
        assert!(
            problem["detail"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("settings"),
            "the refusal points at the fix: {problem}"
        );
    }
}

/// The read-only ruling on this pair: the status is a pure read and stays
/// served (a read-only mirror still shows its sync card), the pull is a
/// mutation of this instance's copy and is refused. The GET is asserted as
/// "not 403" rather than as a 200, because it is the 403 that would appear if
/// anyone ever added `refuse_read_only` to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_serves_the_status_and_refuses_the_pull() {
    let ro = serve(Options {
        read_only: true,
        github: true,
        ..Options::default()
    })
    .await;
    let admin = login(ro.addr, "root", "rootpw").await;

    let status = as_session(
        ro.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        status.status(),
        404,
        "the read reached the domain check instead of being refused outright"
    );

    let pull = as_session(
        ro.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/sync",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        pull.status(),
        403,
        "read_only refuses the pull before anything else is decided"
    );
}

/// Unregister: the registration and index rows go, the files stay, and the
/// response carries the files_kept flag the confirmation wording leans on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregister_answers_with_files_kept_and_the_domain_vanishes() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let resp = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["files_kept"], true);
    assert_eq!(body["rooms_closed"], 0, "nobody was co-editing this one");
    assert!(
        fx._tmp.path().join("eng/alpha.md").exists(),
        "files stay on disk"
    );

    let listing = as_session(fx.addr, reqwest::Method::GET, "/api/v1/domains", &admin)
        .send()
        .await
        .unwrap();
    assert!(!listing.text().await.unwrap().contains("\"eng\""));

    let again = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(again.status(), 404);

    // The virtual arm: a database-backed domain's rows ARE its knowledge, so
    // its unregistration says files_kept: false and the confirmation wording
    // the UI shows has to differ. Same route, opposite flag.
    let made = as_session(fx.addr, reqwest::Method::POST, "/api/v1/domains", &admin)
        .json(&serde_json::json!({"mode": "virtual", "name": "ephemeral"}))
        .send()
        .await
        .unwrap();
    assert_eq!(made.status(), 201);
    let gone = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/ephemeral",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(gone.status(), 200);
    let gone: serde_json::Value = gone.json().await.unwrap();
    assert_eq!(gone["files_kept"], false, "there are no files to keep");
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

/// The download IS the backup story: a zip whose entries reproduce the
/// domain's files byte for byte, fetched with plain cookie auth (an anchor
/// click), admin-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_archive_download_is_a_faithful_zip() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/archive",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "application/zip");
    assert!(
        resp.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("eng-archive.zip")
    );
    let bytes = resp.bytes().await.unwrap();

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_ref())).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["MANIFEST.md", "alpha.md"]);
    let mut alpha = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("alpha.md").unwrap(), &mut alpha).unwrap();
    assert_eq!(alpha, ALPHA);

    // Editor and viewer are 403 on this GET.
    for (name, pw) in [("eddy", "eddypw"), ("vera", "verapw")] {
        let session = login(fx.addr, name, pw).await;
        let resp = as_session(
            fx.addr,
            reqwest::Method::GET,
            "/api/v1/domains/eng/archive",
            &session,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), 403, "{name}");
    }

    // The UI downloads this with an anchor click, which carries the cookie and
    // no CSRF header at all: a safe method, so the guard exempts it.
    let anchor = client()
        .get(format!("http://{}/api/v1/domains/eng/archive", fx.addr))
        .header("cookie", format!("fluid_session={}", admin.0))
        .send()
        .await
        .unwrap();
    assert_eq!(anchor.status(), 200, "an anchor click sends no csrf header");

    // An unknown domain is an honest 404, not an empty archive.
    let ghost = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/ghost/archive",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(ghost.status(), 404);
}

/// The archive download is exactly where a read-only mirror wants it: a pure
/// read, and the instance's backup story, so read_only serves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_still_serves_the_archive_download() {
    let ro = serve(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    let admin = login(ro.addr, "root", "rootpw").await;

    let resp = as_session(
        ro.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/archive",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200, "the backup of a read-only mirror");
}
