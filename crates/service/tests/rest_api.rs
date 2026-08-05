//! Drives the REST surface `serve --http` mounts at `/api/v1` over a live TCP
//! listener, through the production router construction
//! (`crystalline_service::daemon::http_router`) rather than a hand-built
//! sub-router, so a regression in the mount point or in the nesting order
//! against the MCP fallback service fails here.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use tokio::sync::Mutex;

/// Build the same kind of engine the other service integration tests use: a
/// real temp-directory domain (files are the source of truth) synced into an
/// in-memory Turso store, response format pinned to plain JSON so assertions
/// don't have to account for TOON framing.
async fn build_engine() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
    )
    .unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// Bind `http_router` on an ephemeral loopback port and serve it on a
/// background task for the duration of the test.
fn serve_test_router(engine: Arc<Engine>) -> std::net::SocketAddr {
    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// Serve the production router over a fixture engine. The returned guard owns
/// the domain's temp directory and must outlive the requests.
async fn serve_test_router_with_fixture() -> (std::net::SocketAddr, tempfile::TempDir) {
    let (tmp, engine) = build_engine().await;
    let addr = serve_test_router(engine);
    (addr, tmp)
}

/// GET a path off the test server. The client disables proxy discovery: the
/// target is loopback, where a system proxy must never be consulted anyway,
/// and reqwest's platform proxy lookup can block for a minute on a machine
/// with a managed network configuration.
async fn get(addr: std::net::SocketAddr, path: &str) -> reqwest::Response {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{addr}{path}"))
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn me_reports_capabilities_when_anonymous() {
    let (addr, _guard) = serve_test_router_with_fixture().await;
    let resp = get(addr, "/api/v1/auth/me").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["user"].is_null());
    assert_eq!(body["anonymous"], true);
    assert_eq!(body["version"], crystalline_core::VERSION);
    assert!(body["read_only"].is_boolean());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_api_path_is_problem_json() {
    let (addr, _guard) = serve_test_router_with_fixture().await;
    let resp = get(addr, "/api/v1/nope").await;
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "not found");
}

/// The REST mount must not shadow what the liveness probe and the MCP
/// transport already own: `/api/v1` nests ahead of the fallback service, and
/// everything outside it keeps its old behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_probe_still_answers_beside_the_rest_mount() {
    let (addr, _guard) = serve_test_router_with_fixture().await;
    let resp = get(addr, "/health").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}
