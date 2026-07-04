//! MCP-layer tests for the six GitHub collaboration tools: the runtime
//! gating matrix over `list_tools`/`get_tool`, the clean refusal a direct
//! call to a hidden tool still gets, the `configure` tool's snapshot, set
//! flow and GitHub connect state machine, and wiring smoke tests for the
//! five origin tools against the engine with an injected `MockProvider`.
//!
//! Every test that touches a GitHub connection injects either
//! `support::MockProvider` (via `Engine::with_origin_provider`) or a local
//! `FakeConnectAuth` (via `Engine::with_connect_auth`), and points token and
//! origin state at a tempdir (`Engine::with_token_store_dir`,
//! `Engine::with_origins_dir`), so nothing here reaches a network, a real
//! GitHub repository, or the developer's actual OS keychain.

mod support;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use crystalline_core::config::{GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_remote::{DeviceFlowStart, RemoteError};
use crystalline_service::Engine;
use crystalline_service::engine::{ConnectAuth, EngineError};
use crystalline_service::mcp::McpServer;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{NotificationContext, Peer, RunningService};
use rmcp::{ClientHandler, RoleClient, RoleServer};
use serde_json::{Value, json};
use support::MockProvider;
use tokio::sync::Mutex;

// --- shared fixtures ---------------------------------------------------------

fn config(github_enabled: bool) -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    if github_enabled {
        cfg.github = Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        });
    }
    cfg
}

/// A bare engine (no origin provider, no connect auth) for gating and
/// refusal tests that never reach `resolve_origin_provider` or a real
/// connect action. `config_path` points `configure`'s `set`/`unset` at a
/// tempdir file instead of the real machine global config.
async fn engine(config_path: &std::path::Path, github_enabled: bool, read_only: bool) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        config(github_enabled),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_read_only(read_only)
}

fn manifest() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
    .into_bytes()
}

fn commit_files(pairs: &[(&str, Vec<u8>)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(p, c)| (p.to_string(), c.clone()))
        .collect()
}

async fn connect(
    engine: Arc<Engine>,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, McpServer>,
) {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_task =
        tokio::spawn(async move { rmcp::serve_server(McpServer::new(engine), server_io).await });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let server = server_task.await.unwrap().unwrap();
    (client, server)
}

/// Call a tool, returning its JSON body on success or the error message on
/// failure.
async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Result<Value, String> {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    match peer.call_tool(params).await {
        Ok(result) => {
            let v = serde_json::to_value(&result).unwrap();
            let text = v
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(serde_json::from_str(text).unwrap_or(Value::String(text.to_string())))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Polls `f` until it returns `Some`, up to two seconds, panicking otherwise.
/// Used to wait for the connect background task to land its outcome.
async fn wait_until<F, Fut, T>(mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    for _ in 0..200 {
        if let Some(v) = f().await {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition was not met within two seconds");
}

/// The six collaboration tool names, in the order the task brief lists them.
const ALL_SIX: [&str; 6] = [
    "configure",
    "add_domain",
    "share_changes",
    "update_domain",
    "origin_status",
    "resolve_conflict",
];

// --- gating matrix -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gating_matrix_over_list_tools() {
    let cases: [(bool, bool, &[&str]); 4] = [
        (false, false, &["configure"]),
        (false, true, &[]),
        (true, false, &ALL_SIX),
        (true, true, &["update_domain", "origin_status"]),
    ];
    for (github_enabled, read_only, visible) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let eng =
            Arc::new(engine(&tmp.path().join("config.yaml"), github_enabled, read_only).await);
        let (client, _server) = connect(eng).await;
        let tools = client.peer().list_tools(Default::default()).await.unwrap();
        let names: Vec<String> = tools.tools.iter().map(|t| t.name.to_string()).collect();
        for name in ALL_SIX {
            let should_be_visible = visible.contains(&name);
            assert_eq!(
                names.contains(&name.to_string()),
                should_be_visible,
                "github_enabled={github_enabled} read_only={read_only} name={name} names={names:?}"
            );
        }
    }
}

#[tokio::test]
async fn gating_matrix_over_get_tool() {
    use rmcp::ServerHandler;

    let cases: [(bool, bool, &[&str]); 4] = [
        (false, false, &["configure"]),
        (false, true, &[]),
        (true, false, &ALL_SIX),
        (true, true, &["update_domain", "origin_status"]),
    ];
    for (github_enabled, read_only, visible) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let eng =
            Arc::new(engine(&tmp.path().join("config.yaml"), github_enabled, read_only).await);
        let server = McpServer::new(eng);
        for name in ALL_SIX {
            let should_be_visible = visible.contains(&name);
            assert_eq!(
                server.get_tool(name).is_some(),
                should_be_visible,
                "github_enabled={github_enabled} read_only={read_only} name={name}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flipping_github_enabled_mid_session_changes_the_next_list_tools_result() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), false, false).await);
    let (client, _server) = connect(eng.clone()).await;
    let peer = client.peer();

    let tools = peer.list_tools(Default::default()).await.unwrap();
    assert!(!tools.tools.iter().any(|t| t.name == "add_domain"));

    call(
        peer,
        "configure",
        json!({"set": {"github.enabled": "true"}}),
    )
    .await
    .unwrap();

    let tools = peer.list_tools(Default::default()).await.unwrap();
    assert!(
        tools.tools.iter().any(|t| t.name == "add_domain"),
        "add_domain must appear once github.enabled flips to true"
    );
}

// --- hidden tools still route to the clean engine refusal -------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hidden_collab_tools_route_to_not_enabled_when_github_is_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), false, false).await);
    let (client, _server) = connect(eng).await;
    let peer = client.peer();

    let cases: [(&str, Value); 5] = [
        ("add_domain", json!({"repo": "acme/brand-knowledge"})),
        ("share_changes", json!({"domain": "eng"})),
        ("update_domain", json!({})),
        ("origin_status", json!({})),
        (
            "resolve_conflict",
            json!({"domain": "eng", "path": "a.md", "resolution": "mine"}),
        ),
    ];
    for (tool, args) in cases {
        let err = call(peer, tool, args).await.unwrap_err();
        assert!(
            err.contains("not enabled"),
            "{tool} should refuse with the not-enabled message, got: {err}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hidden_write_collab_tools_route_to_read_only_when_enabled_and_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), true, true).await);
    let (client, _server) = connect(eng).await;
    let peer = client.peer();

    let err = call(peer, "configure", json!({})).await.unwrap_err();
    assert!(err.contains("read-only"), "{err}");

    let err = call(peer, "add_domain", json!({"repo": "acme/brand-knowledge"}))
        .await
        .unwrap_err();
    assert!(err.contains("read-only"), "{err}");

    let err = call(peer, "share_changes", json!({"domain": "eng"}))
        .await
        .unwrap_err();
    assert!(err.contains("read-only"), "{err}");

    let err = call(
        peer,
        "resolve_conflict",
        json!({"domain": "eng", "path": "a.md", "resolution": "mine"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("read-only"), "{err}");
}

// --- configure: snapshot shape and set flow ---------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_with_no_args_reports_the_settings_snapshot_and_github_block() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), false, false).await);
    let (client, _server) = connect(eng).await;
    let peer = client.peer();

    let out = call(peer, "configure", json!({})).await.unwrap();
    let settings = out["settings"].as_array().unwrap();
    assert_eq!(settings.len(), 4, "{settings:?}");
    assert!(settings.iter().any(|s| s["key"] == "github.enabled"));
    assert_eq!(out["github"]["connected"], json!(false));
    assert!(out["github"]["pending_connect"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_set_multiple_keys_applies_in_order_and_returns_the_fresh_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), false, false).await);
    let (client, _server) = connect(eng).await;
    let peer = client.peer();

    let out = call(
        peer,
        "configure",
        json!({"set": {"github.enabled": "true", "github.poll_secs": "120"}}),
    )
    .await
    .unwrap();
    let settings = out["settings"].as_array().unwrap();
    let enabled = settings
        .iter()
        .find(|s| s["key"] == "github.enabled")
        .unwrap();
    assert_eq!(enabled["value"], json!("true"));
    let poll = settings
        .iter()
        .find(|s| s["key"] == "github.poll_secs")
        .unwrap();
    assert_eq!(poll["value"], json!("120"));

    let out = call(peer, "configure", json!({"unset": ["github.poll_secs"]}))
        .await
        .unwrap();
    let settings = out["settings"].as_array().unwrap();
    let poll = settings
        .iter()
        .find(|s| s["key"] == "github.poll_secs")
        .unwrap();
    assert_eq!(poll["is_default"], json!(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_set_stops_at_the_first_bad_key_and_reports_what_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), false, false).await);
    let (client, _server) = connect(eng.clone()).await;
    let peer = client.peer();

    // `set` is a map, applied in ascending key order: "github.enabled"
    // sorts before "zzz.bogus", so the valid key lands before the invalid
    // one is reached.
    let err = call(
        peer,
        "configure",
        json!({"set": {"github.enabled": "true", "zzz.bogus": "x"}}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("zzz.bogus"), "{err}");
    assert!(err.contains("applied"), "{err}");
    assert!(err.contains("github.enabled"), "{err}");
    // The valid key before the bad one was already applied.
    assert!(eng.config().github_enabled());
}

// --- configure: tools/list_changed -------------------------------------------

/// A client handler that records whether it ever received
/// `notifications/tools/list_changed`.
#[derive(Clone, Default)]
struct NotifyClient {
    got_list_changed: Arc<tokio::sync::Notify>,
}

impl ClientHandler for NotifyClient {
    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        let notify = self.got_list_changed.clone();
        async move {
            notify.notify_one();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_flipping_github_enabled_pushes_a_tool_list_changed_notification() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), false, false).await);
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_task =
        tokio::spawn(async move { rmcp::serve_server(McpServer::new(eng), server_io).await });
    let handler = NotifyClient::default();
    let client = rmcp::serve_client(handler.clone(), client_io)
        .await
        .unwrap();
    let _server = server_task.await.unwrap().unwrap();
    let peer = client.peer();

    call(
        peer,
        "configure",
        json!({"set": {"github.enabled": "true"}}),
    )
    .await
    .unwrap();

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        handler.got_list_changed.notified(),
    )
    .await
    .expect("expected a tools/list_changed notification after configure flipped github.enabled");
}

// --- configure: GitHub connect state machine (engine-level) -----------------

/// A fake [`ConnectAuth`] whose three outcomes are set once at construction
/// and consumed once each, with `run_device_flow` blockable on a `Notify` so
/// a test can observe the "still waiting on the user" state before letting
/// the flow land.
struct FakeConnectAuth {
    start_result: std::sync::Mutex<Option<Result<DeviceFlowStart, RemoteError>>>,
    run_gate: tokio::sync::Notify,
    run_result: std::sync::Mutex<Option<Result<String, RemoteError>>>,
    validate_result: std::sync::Mutex<Option<Result<String, RemoteError>>>,
}

fn fake_auth(
    start: Result<DeviceFlowStart, RemoteError>,
    run: Result<String, RemoteError>,
    validate: Result<String, RemoteError>,
) -> Arc<FakeConnectAuth> {
    Arc::new(FakeConnectAuth {
        start_result: std::sync::Mutex::new(Some(start)),
        run_gate: tokio::sync::Notify::new(),
        run_result: std::sync::Mutex::new(Some(run)),
        validate_result: std::sync::Mutex::new(Some(validate)),
    })
}

fn device_flow_start() -> DeviceFlowStart {
    DeviceFlowStart {
        device_code: "devcode".to_string(),
        user_code: "ABCD-1234".to_string(),
        verification_url: "https://github.com/login/device".to_string(),
        interval_secs: 0,
        expires_in_secs: 900,
    }
}

#[async_trait::async_trait]
impl ConnectAuth for FakeConnectAuth {
    async fn start_device_flow(
        &self,
        _auth_base: &str,
        _client_id: &str,
    ) -> Result<DeviceFlowStart, RemoteError> {
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
        self.validate_result
            .lock()
            .unwrap()
            .take()
            .expect("validate_token result not set")
    }
}

async fn engine_for_connect(auth: Arc<FakeConnectAuth>, dir: &std::path::Path) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(Arc::new(Mutex::new(store)), config(false), None, None)
        .with_connect_auth(auth)
        .with_token_store_dir(dir.to_path_buf())
}

#[tokio::test]
async fn token_connect_validates_saves_and_reports_connected() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = fake_auth(
        Err(RemoteError::NotConnected),
        Err(RemoteError::NotConnected),
        Ok("octocat".to_string()),
    );
    let eng = engine_for_connect(auth, tmp.path()).await;

    let result = eng.connect_with_token("pat-123", None).await.unwrap();
    assert_eq!(result["github"]["connected"], json!(true));
    assert_eq!(result["github"]["user"], json!("octocat"));
    assert_eq!(result["github"]["token_store"], json!("file"));

    // A later snapshot reflects the saved token.
    let snap = eng.configure_snapshot().await.unwrap();
    assert_eq!(snap["github"]["connected"], json!(true));
    assert_eq!(snap["github"]["user"], json!("octocat"));
}

#[tokio::test]
async fn token_connect_refuses_on_a_read_only_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = fake_auth(
        Err(RemoteError::NotConnected),
        Err(RemoteError::NotConnected),
        Ok("octocat".to_string()),
    );
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Engine::new(Arc::new(Mutex::new(store)), config(false), None, None)
        .with_connect_auth(auth)
        .with_token_store_dir(tmp.path().to_path_buf())
        .with_read_only(true);

    let err = eng.connect_with_token("pat-123", None).await.unwrap_err();
    assert!(matches!(err, EngineError::ReadOnly));
}

#[tokio::test]
async fn device_flow_second_connect_reports_the_same_pending_code_then_lands_connected() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = fake_auth(
        Ok(device_flow_start()),
        Ok("device-token".to_string()),
        Ok("octocat".to_string()),
    );
    let eng = engine_for_connect(auth.clone(), tmp.path()).await;

    let first = eng.start_device_connect(None).await.unwrap();
    assert_eq!(first["github"]["connected"], json!(false));
    assert_eq!(first["github"]["pending_connect"]["pending"], json!(true));
    assert_eq!(
        first["github"]["pending_connect"]["user_code"],
        json!("ABCD-1234")
    );

    // A second connect call while the flow is still waiting on the user
    // reports the same pending code rather than starting a second flow.
    let second = eng.start_device_connect(None).await.unwrap();
    assert_eq!(
        second["github"]["pending_connect"]["user_code"],
        json!("ABCD-1234")
    );

    // Let the background task's run_device_flow complete.
    auth.run_gate.notify_one();

    let landed = wait_until(|| async {
        let snap = eng.configure_snapshot().await.unwrap();
        (snap["github"]["connected"] == json!(true)).then_some(snap)
    })
    .await;
    assert_eq!(landed["github"]["user"], json!("octocat"));
    assert!(landed["github"]["pending_connect"].is_null());

    // The slot cleared: the connection stays reported without a stale
    // pending block.
    let after = eng.configure_snapshot().await.unwrap();
    assert_eq!(after["github"]["connected"], json!(true));
    assert!(after["github"]["pending_connect"].is_null());
}

#[tokio::test]
async fn device_flow_failure_is_reported_once_as_an_error_then_the_slot_clears() {
    let tmp = tempfile::tempdir().unwrap();
    let auth = fake_auth(
        Ok(device_flow_start()),
        Err(RemoteError::AuthExpired),
        Err(RemoteError::AuthExpired),
    );
    let eng = engine_for_connect(auth.clone(), tmp.path()).await;

    eng.start_device_connect(None).await.unwrap();
    auth.run_gate.notify_one();

    let landed_err = wait_until(|| async { eng.configure_snapshot().await.err() }).await;
    assert!(
        matches!(landed_err, EngineError::Remote(RemoteError::AuthExpired)),
        "{landed_err}"
    );

    // Reported once: the slot is now clear and a plain snapshot no longer
    // errors, reporting the ordinary (never-connected) state.
    let after = eng.configure_snapshot().await.unwrap();
    assert_eq!(after["github"]["connected"], json!(false));
    assert!(after["github"]["pending_connect"].is_null());
}

// --- origin tool wiring (happy path, via the injected MockProvider) ---------

async fn engine_with_provider(
    config_path: &std::path::Path,
    origins_dir: &std::path::Path,
    provider: Arc<MockProvider>,
) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        config(true),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_origin_provider(provider)
    .with_origins_dir(origins_dir.to_path_buf())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_domain_tool_wires_through_to_origin_add() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "turbine notes")),
    ]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = Arc::new(engine_with_provider(&config_path, &origins_dir, mock).await);
    let (client, _server) = connect(eng).await;
    let peer = client.peer();

    let out = call(
        peer,
        "add_domain",
        json!({ "repo": "acme/brand-knowledge", "folder": root.to_str().unwrap() }),
    )
    .await
    .unwrap();
    assert_eq!(out["domain"], json!("brand-knowledge"));
    assert_eq!(out["engrams"], json!(2));
    assert_eq!(out["base_commit"], json!(commit));
    assert!(root.join("MANIFEST.md").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_changes_tool_wires_through_to_origin_share() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = Arc::new(engine_with_provider(&config_path, &origins_dir, mock).await);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();

    let (client, _server) = connect(eng).await;
    let peer = client.peer();
    let out = call(peer, "share_changes", json!({ "domain": "brand" }))
        .await
        .unwrap();
    assert_eq!(out["outcome"], json!("proposed"));
    assert_eq!(out["added"][0], json!("notes/new.md"));
    assert!(out["url"].as_str().unwrap().starts_with("https://"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_domain_tool_wires_through_to_origin_update() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = Arc::new(engine_with_provider(&config_path, &origins_dir, mock).await);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let (client, _server) = connect(eng).await;
    let peer = client.peer();
    let out = call(peer, "update_domain", json!({})).await.unwrap();
    let domains = out["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], json!("brand"));
    assert_eq!(domains[0]["up_to_date"], json!(true));
    assert!(out["errors"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_status_tool_wires_through_to_origin_status() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = Arc::new(engine_with_provider(&config_path, &origins_dir, mock).await);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let (client, _server) = connect(eng).await;
    let peer = client.peer();
    let out = call(peer, "origin_status", json!({})).await.unwrap();
    assert_eq!(out["connection"]["connected"], json!(true));
    let domains = out["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], json!("brand"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_conflict_tool_wires_through_to_origin_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one")),
    ]));
    mock.set_branch("main", &c1);

    let config_path = tmp.path().join("config.yaml");
    let origins_dir = tmp.path().join("origins");
    let root = tmp.path().join("brand-knowledge");
    let eng = Arc::new(engine_with_provider(&config_path, &origins_dir, mock.clone()).await);
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    // A genuine same-line conflict, from a real pull.
    std::fs::write(root.join("notes/a.md"), engram("A", "a", "line one LOCAL")).unwrap();
    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/a.md", engram("A", "a", "line one UPSTREAM")),
    ]));
    mock.set_branch("main", &c2);
    eng.origin_update(Some("brand")).await.unwrap();

    let (client, _server) = connect(eng).await;
    let peer = client.peer();
    let out = call(
        peer,
        "resolve_conflict",
        json!({ "domain": "brand", "path": "notes/a.md", "resolution": "theirs" }),
    )
    .await
    .unwrap();
    assert_eq!(out["remaining"], json!(0));

    let content = std::fs::read_to_string(root.join("notes/a.md")).unwrap();
    assert!(content.contains("line one UPSTREAM"), "{content}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_conflict_merged_without_content_is_a_clean_invalid_params_error() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(engine(&tmp.path().join("config.yaml"), true, false).await);
    let (client, _server) = connect(eng).await;
    let peer = client.peer();

    let err = call(
        peer,
        "resolve_conflict",
        json!({ "domain": "eng", "path": "a.md", "resolution": "merged" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("content"), "{err}");
    assert!(err.contains("merged"), "{err}");
}
