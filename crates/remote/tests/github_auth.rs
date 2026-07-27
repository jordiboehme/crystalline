//! Offline mock-server tests for the GitHub device flow, GHES endpoint
//! derivation and token validation in `github::auth`. Every test spins up a
//! throwaway axum server on an ephemeral localhost port, mirroring
//! `tests/github_client.rs`, so the suite never touches the real network and
//! never touches the real OS keychain.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use crystalline_remote::RemoteError;
use crystalline_remote::github::auth::{
    DeviceFlowStart, DevicePoll, poll_device_flow_once, run_device_flow, start_device_flow,
    validate_token,
};
use tokio::net::TcpListener;

/// Starts `router` on an ephemeral localhost port and returns its base url,
/// same pattern as `tests/github_client.rs`.
async fn spawn(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

fn device_code_body() -> serde_json::Value {
    serde_json::json!({
        "device_code": "devicecode123",
        "user_code": "WDJB-MJHT",
        "verification_uri": "https://github.com/login/device",
        "expires_in": 900,
        "interval": 5,
    })
}

// --- start_device_flow -----------------------------------------------------

#[tokio::test]
async fn start_device_flow_parses_the_documented_fields() {
    let app = Router::new().route(
        "/login/device/code",
        post(|| async { Json(device_code_body()) }),
    );
    let base = spawn(app).await;

    let start = start_device_flow(&base, "client-id-1").await.unwrap();

    assert_eq!(start.device_code, "devicecode123");
    assert_eq!(start.user_code, "WDJB-MJHT");
    assert_eq!(start.verification_url, "https://github.com/login/device");
    assert_eq!(start.interval_secs, 5);
    assert_eq!(start.expires_in_secs, 900);
}

#[tokio::test]
async fn start_device_flow_sends_the_client_id_and_repo_scope_with_a_json_accept_header() {
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let app = Router::new()
        .route(
            "/login/device/code",
            post(
                |State(captured): State<Arc<Mutex<Option<String>>>>,
                 headers: HeaderMap,
                 body: String| async move {
                    assert_eq!(
                        headers.get("accept").and_then(|v| v.to_str().ok()),
                        Some("application/json")
                    );
                    *captured.lock().unwrap() = Some(body);
                    Json(device_code_body())
                },
            ),
        )
        .with_state(captured.clone());
    let base = spawn(app).await;

    start_device_flow(&base, "abc123").await.unwrap();

    let body = captured.lock().unwrap().clone().unwrap();
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["client_id"], "abc123");
    assert_eq!(body["scope"], "repo");
}

#[tokio::test]
async fn start_device_flow_maps_a_non_success_status_to_api() {
    let app = Router::new().route(
        "/login/device/code",
        post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "unsupported_grant_type"})),
            )
        }),
    );
    let base = spawn(app).await;

    let err = start_device_flow(&base, "client").await.unwrap_err();
    match err {
        RemoteError::Api { status, .. } => assert_eq!(status, 400),
        other => panic!("expected Api, got {other:?}"),
    }
}

#[tokio::test]
async fn start_device_flow_maps_connection_refused_to_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let err = start_device_flow(&format!("http://{addr}"), "client")
        .await
        .unwrap_err();
    assert!(matches!(err, RemoteError::Offline), "{err:?}");
}

// --- poll_device_flow_once --------------------------------------------------

#[tokio::test]
async fn poll_device_flow_once_reports_pending_then_a_token() {
    let calls = Arc::new(Mutex::new(0u32));

    let app = Router::new().route(
        "/login/oauth/access_token",
        post(move || {
            let calls = calls.clone();
            async move {
                let mut n = calls.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    Json(serde_json::json!({"error": "authorization_pending"}))
                } else {
                    Json(serde_json::json!({"access_token": "tok-abc"}))
                }
            }
        }),
    );
    let base = spawn(app).await;

    let first = poll_device_flow_once(&base, "client", "devicecode")
        .await
        .unwrap();
    assert_eq!(first, DevicePoll::Pending);

    let second = poll_device_flow_once(&base, "client", "devicecode")
        .await
        .unwrap();
    assert_eq!(second, DevicePoll::Token("tok-abc".to_string()));
}

#[tokio::test]
async fn poll_device_flow_once_maps_slow_down() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async { Json(serde_json::json!({"error": "slow_down"})) }),
    );
    let base = spawn(app).await;

    let outcome = poll_device_flow_once(&base, "client", "devicecode")
        .await
        .unwrap();
    assert_eq!(outcome, DevicePoll::SlowDown);
}

#[tokio::test]
async fn poll_device_flow_once_maps_expired_token_to_auth_expired() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async { Json(serde_json::json!({"error": "expired_token"})) }),
    );
    let base = spawn(app).await;

    let err = poll_device_flow_once(&base, "client", "devicecode")
        .await
        .unwrap_err();
    assert!(matches!(err, RemoteError::AuthExpired), "{err:?}");
}

#[tokio::test]
async fn poll_device_flow_once_maps_access_denied_to_a_403_api_error() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async { Json(serde_json::json!({"error": "access_denied"})) }),
    );
    let base = spawn(app).await;

    let err = poll_device_flow_once(&base, "client", "devicecode")
        .await
        .unwrap_err();
    match err {
        RemoteError::Api { status, message } => {
            assert_eq!(status, 403);
            assert!(message.contains("declined"), "{message}");
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_device_flow_once_maps_an_undocumented_error_code_to_a_clear_message() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async {
            Json(serde_json::json!({
                "error": "temporarily_unavailable",
                "error_description": "the server is overloaded",
            }))
        }),
    );
    let base = spawn(app).await;

    let err = poll_device_flow_once(&base, "client", "devicecode")
        .await
        .unwrap_err();
    match err {
        RemoteError::Api { status, message } => {
            assert_eq!(
                status, 200,
                "status 0 must never be user-visible: {message}"
            );
            assert!(message.contains("temporarily_unavailable"), "{message}");
            assert!(message.contains("the server is overloaded"), "{message}");
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_device_flow_once_maps_an_undocumented_error_code_with_no_description() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async { Json(serde_json::json!({"error": "temporarily_unavailable"})) }),
    );
    let base = spawn(app).await;

    let err = poll_device_flow_once(&base, "client", "devicecode")
        .await
        .unwrap_err();
    match err {
        RemoteError::Api { status, message } => {
            assert_eq!(
                status, 200,
                "status 0 must never be user-visible: {message}"
            );
            assert!(message.contains("temporarily_unavailable"), "{message}");
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_device_flow_once_maps_connection_refused_to_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let err = poll_device_flow_once(&format!("http://{addr}"), "client", "devicecode")
        .await
        .unwrap_err();
    assert!(matches!(err, RemoteError::Offline), "{err:?}");
}

// --- run_device_flow ---------------------------------------------------------

// One end-to-end test that the loop really drives the HTTP endpoint: it
// polls repeatedly with the configured host, client id and device code
// until the token arrives. The cadence itself (the start interval, the
// `slow_down` backoff growth, the expiry deadline) is covered by the
// paused-clock unit tests next to the loop in `github::auth`, so nothing
// here races a wall-clock deadline: `interval_secs: 0` makes the waits
// free and the expiry window is far larger than any round-trip a loaded
// machine can produce. A paused clock cannot be used here, since the mock
// axum server and the client run real loopback round-trips on the same
// runtime and the auto-advanced clock fires the request timeout before they
// complete.
#[tokio::test]
async fn run_device_flow_polls_the_endpoint_until_a_token_arrives() {
    let seen = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let recorded = seen.clone();

    let app = Router::new().route(
        "/login/oauth/access_token",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen.clone();
            async move {
                let mut seen = seen.lock().unwrap();
                seen.push(body);
                if seen.len() < 3 {
                    Json(serde_json::json!({"error": "authorization_pending"}))
                } else {
                    Json(serde_json::json!({"access_token": "tok-final"}))
                }
            }
        }),
    );
    let base = spawn(app).await;
    let start = DeviceFlowStart {
        device_code: "devicecode".to_string(),
        user_code: "WDJB-MJHT".to_string(),
        verification_url: "https://github.com/login/device".to_string(),
        interval_secs: 0,
        expires_in_secs: 900,
    };

    let token = run_device_flow(&base, "client", &start).await.unwrap();

    assert_eq!(token, "tok-final");
    let bodies = recorded.lock().unwrap();
    assert_eq!(bodies.len(), 3, "it kept polling until the token arrived");
    for body in bodies.iter() {
        assert_eq!(body["client_id"], "client");
        assert_eq!(body["device_code"], "devicecode");
    }
}

// --- validate_token -----------------------------------------------------------

#[tokio::test]
async fn validate_token_returns_the_login_via_current_user() {
    let app = Router::new().route(
        "/user",
        get(|| async { Json(serde_json::json!({"login": "octocat"})) }),
    );
    let base = spawn(app).await;

    let login = validate_token(Some(&base), "sometoken").await.unwrap();

    assert_eq!(login, "octocat");
}

#[tokio::test]
async fn validate_token_maps_unauthorized_to_auth_expired() {
    let app = Router::new().route(
        "/user",
        get(|| async {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"message": "Bad credentials"})),
            )
        }),
    );
    let base = spawn(app).await;

    let err = validate_token(Some(&base), "badtoken").await.unwrap_err();

    assert!(matches!(err, RemoteError::AuthExpired), "{err:?}");
}
