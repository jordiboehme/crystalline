//! Offline mock-server tests for the GitHub device flow, GHES endpoint
//! derivation and token validation in `github::auth`. Every test spins up a
//! throwaway axum server on an ephemeral localhost port, mirroring
//! `tests/it/github_client.rs`, so the suite never touches the real network and
//! never touches the real OS keychain.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use crystalline_remote::RemoteError;
use crystalline_remote::github::auth::{
    DeviceFlowStart, DevicePoll, authorize_url, exchange_code, github_user, poll_device_flow_once,
    refresh_user_token, revoke_user_token, run_device_flow, start_device_flow, validate_token,
};
use tokio::net::TcpListener;

/// Starts `router` on an ephemeral localhost port and returns its base url,
/// same pattern as `tests/it/github_client.rs`.
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

    let start = start_device_flow(&base, "client-id-1", Some("repo"))
        .await
        .unwrap();

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

    start_device_flow(&base, "abc123", Some("repo"))
        .await
        .unwrap();

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

    let err = start_device_flow(&base, "client", Some("repo"))
        .await
        .unwrap_err();
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

    let err = start_device_flow(&format!("http://{addr}"), "client", Some("repo"))
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
    match second {
        DevicePoll::Token(grant) => assert_eq!(grant.access_token, "tok-abc"),
        other => panic!("{other:?}"),
    }
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

    assert_eq!(token.access_token, "tok-final");
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

// --- App user tokens ----------------------------------------------------------

fn expiring_body() -> serde_json::Value {
    serde_json::json!({
        "access_token": "ghu_access",
        "expires_in": 28800,
        "refresh_token": "ghr_refresh",
        "refresh_token_expires_in": 15897600,
        "token_type": "bearer",
    })
}

#[tokio::test]
async fn exchange_code_parses_expiring_tokens() {
    let seen = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let recorded = seen.clone();
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(body);
                Json(expiring_body())
            }
        }),
    );
    let base = spawn(app).await;

    let before = chrono::Utc::now();
    let grant = exchange_code(
        &base,
        "Iv1.app",
        "sekret",
        "the-code",
        "http://127.0.0.1/cb",
    )
    .await
    .unwrap();
    let after = chrono::Utc::now();

    assert_eq!(grant.access_token, "ghu_access");
    assert_eq!(grant.refresh_token.as_deref(), Some("ghr_refresh"));
    let eight_hours = chrono::Duration::seconds(28800);
    let expires = grant.expires_at.expect("an expiry");
    assert!(expires >= before + eight_hours && expires <= after + eight_hours);
    let refresh_expires = grant.refresh_expires_at.expect("a refresh expiry");
    let six_months = chrono::Duration::seconds(15897600);
    assert!(refresh_expires >= before + six_months && refresh_expires <= after + six_months);
    let bodies = recorded.lock().unwrap();
    assert_eq!(bodies[0]["client_id"], "Iv1.app");
    assert_eq!(bodies[0]["client_secret"], "sekret");
    assert_eq!(bodies[0]["code"], "the-code");
    assert_eq!(bodies[0]["redirect_uri"], "http://127.0.0.1/cb");
    let debug = format!("{grant:?}");
    assert!(
        !debug.contains("ghu_access") && !debug.contains("ghr_refresh"),
        "{debug}"
    );
}

#[tokio::test]
async fn a_ghes_token_without_expiry_has_no_refresh() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async { Json(serde_json::json!({"access_token": "ghu_plain"})) }),
    );
    let base = spawn(app).await;

    let grant = exchange_code(&base, "Iv1.app", "sekret", "c", "http://127.0.0.1/cb")
        .await
        .unwrap();

    assert_eq!(grant.access_token, "ghu_plain");
    assert!(grant.refresh_token.is_none());
    assert!(grant.expires_at.is_none());
    assert!(grant.refresh_expires_at.is_none());
}

#[tokio::test]
async fn exchange_code_maps_a_bad_verification_code_to_an_api_error() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async {
            Json(serde_json::json!({"error": "bad_verification_code", "error_description": "The code passed is incorrect or expired."}))
        }),
    );
    let base = spawn(app).await;

    let err = exchange_code(&base, "Iv1.app", "s", "c", "http://127.0.0.1/cb")
        .await
        .unwrap_err();

    assert!(
        matches!(&err, RemoteError::Api { message, .. } if message.contains("bad_verification_code")),
        "{err:?}"
    );
}

#[test]
fn authorize_url_encodes_its_parameters() {
    let url = authorize_url(
        "https://github.com",
        "Iv1.app",
        "http://127.0.0.1:8080/cb?x=1",
        "st ate&1",
    );
    assert!(
        url.starts_with("https://github.com/login/oauth/authorize?"),
        "{url}"
    );
    assert!(url.contains("client_id=Iv1.app"), "{url}");
    assert!(
        url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fcb%3Fx%3D1"),
        "{url}"
    );
    assert!(
        url.contains("state=st+ate%261") || url.contains("state=st%20ate%261"),
        "{url}"
    );
}

#[tokio::test]
async fn an_app_device_flow_sends_no_scope() {
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let c = captured.clone();
    let app = Router::new().route(
        "/login/device/code",
        post(move |Json(body): Json<serde_json::Value>| {
            let c = c.clone();
            async move {
                *c.lock().unwrap() = Some(body);
                Json(device_code_body())
            }
        }),
    );
    let base = spawn(app).await;

    start_device_flow(&base, "Iv1.app", None).await.unwrap();

    let body = captured.lock().unwrap().clone().unwrap();
    assert_eq!(body["client_id"], "Iv1.app");
    assert!(body.get("scope").is_none(), "{body}");
}

#[tokio::test]
async fn the_oauth_app_device_flow_still_sends_repo_scope() {
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let c = captured.clone();
    let app = Router::new().route(
        "/login/device/code",
        post(move |Json(body): Json<serde_json::Value>| {
            let c = c.clone();
            async move {
                *c.lock().unwrap() = Some(body);
                Json(device_code_body())
            }
        }),
    );
    let base = spawn(app).await;

    start_device_flow(&base, "Ov23.oauth", Some("repo"))
        .await
        .unwrap();

    assert_eq!(captured.lock().unwrap().clone().unwrap()["scope"], "repo");
}

#[tokio::test]
async fn refresh_sends_the_refresh_grant_and_parses_the_new_pair() {
    let seen = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let recorded = seen.clone();
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(body);
                Json(expiring_body())
            }
        }),
    );
    let base = spawn(app).await;

    let grant = refresh_user_token(&base, "Iv1.app", Some("sekret"), "ghr_old")
        .await
        .unwrap();
    refresh_user_token(&base, "Iv1.app", None, "ghr_old")
        .await
        .unwrap();

    assert_eq!(grant.refresh_token.as_deref(), Some("ghr_refresh"));
    let bodies = recorded.lock().unwrap();
    assert_eq!(bodies[0]["grant_type"], "refresh_token");
    assert_eq!(bodies[0]["refresh_token"], "ghr_old");
    assert_eq!(bodies[0]["client_secret"], "sekret");
    assert!(
        bodies[1].get("client_secret").is_none(),
        "device tokens refresh without a secret"
    );
}

#[tokio::test]
async fn refresh_maps_bad_refresh_token_to_auth_expired() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async {
            Json(serde_json::json!({"error": "bad_refresh_token", "error_description": "The refresh token passed is incorrect or expired."}))
        }),
    );
    let base = spawn(app).await;

    let err = refresh_user_token(&base, "Iv1.app", Some("s"), "ghr_old")
        .await
        .unwrap_err();

    assert!(matches!(err, RemoteError::AuthExpired), "{err:?}");
}

#[tokio::test]
async fn revoke_sends_basic_auth_and_the_token_in_the_body() {
    let seen = Arc::new(Mutex::new(Vec::<(Option<String>, serde_json::Value)>::new()));
    let recorded = seen.clone();
    let app = Router::new().route(
        "/applications/Iv1.app/token",
        axum::routing::delete(
            move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let seen = seen.clone();
                async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    assert_eq!(
                        headers
                            .get("x-github-api-version")
                            .and_then(|v| v.to_str().ok()),
                        Some("2022-11-28")
                    );
                    seen.lock().unwrap().push((auth, body));
                    axum::http::StatusCode::NO_CONTENT
                }
            },
        ),
    );
    let base = spawn(app).await;

    revoke_user_token(&base, "Iv1.app", "sekret", "ghu_access")
        .await
        .unwrap();

    let seen = recorded.lock().unwrap();
    // base64("Iv1.app:sekret")
    assert_eq!(seen[0].0.as_deref(), Some("Basic SXYxLmFwcDpzZWtyZXQ="));
    assert_eq!(seen[0].1["access_token"], "ghu_access");
}

#[tokio::test]
async fn revoke_treats_a_missing_token_as_done_and_surfaces_other_failures() {
    let app = Router::new()
        .route(
            "/applications/gone/token",
            axum::routing::delete(|| async { axum::http::StatusCode::NOT_FOUND }),
        )
        .route(
            "/applications/boom/token",
            axum::routing::delete(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
        );
    let base = spawn(app).await;

    revoke_user_token(&base, "gone", "s", "t").await.unwrap();
    let err = revoke_user_token(&base, "boom", "s", "t")
        .await
        .unwrap_err();
    assert!(
        matches!(err, RemoteError::Api { status: 500, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn github_user_reads_id_login_and_email() {
    let app = Router::new().route(
        "/user",
        get(|headers: HeaderMap| async move {
            assert_eq!(
                headers.get("authorization").and_then(|v| v.to_str().ok()),
                Some("Bearer ghu_access")
            );
            assert_eq!(
                headers
                    .get("x-github-api-version")
                    .and_then(|v| v.to_str().ok()),
                Some("2022-11-28")
            );
            Json(serde_json::json!({"id": 583231, "login": "octocat", "email": null}))
        }),
    );
    let base = spawn(app).await;

    let user = github_user(&base, "ghu_access").await.unwrap();

    assert_eq!(user.id, 583231);
    assert_eq!(user.login, "octocat");
    assert!(user.email.is_none());
}

#[tokio::test]
async fn github_user_maps_unauthorized_to_auth_expired() {
    let app = Router::new().route(
        "/user",
        get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
    );
    let base = spawn(app).await;

    let err = github_user(&base, "bad").await.unwrap_err();

    assert!(matches!(err, RemoteError::AuthExpired), "{err:?}");
}

#[tokio::test]
async fn revoke_never_puts_a_reflected_body_into_the_error() {
    let app = Router::new().route(
        "/applications/c/token",
        axum::routing::delete(|body: String| async move {
            (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("<html>bad gateway for {body}</html>"),
            )
        }),
    );
    let base = spawn(app).await;

    let err = revoke_user_token(&base, "c", "s", "ghu_livetoken")
        .await
        .unwrap_err();

    assert!(
        matches!(err, RemoteError::Api { status: 502, .. }),
        "{err:?}"
    );
    assert!(
        !format!("{err:?} {err}").contains("ghu_livetoken"),
        "{err:?}"
    );
}

#[tokio::test]
async fn an_out_of_range_expires_in_reads_as_no_expiry() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async {
            Json(serde_json::json!({
                "access_token": "ghu_x",
                "expires_in": i64::MAX,
                "refresh_token": "ghr_x",
                "refresh_token_expires_in": i64::MAX,
            }))
        }),
    );
    let base = spawn(app).await;

    let grant = exchange_code(&base, "c", "s", "code", "http://127.0.0.1/cb")
        .await
        .unwrap();

    assert_eq!(grant.access_token, "ghu_x");
    assert!(grant.expires_at.is_none());
    assert!(grant.refresh_expires_at.is_none());
}
