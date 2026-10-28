//! Offline mock-server tests for the GitHub App web flow in `github::web`.
//! Same pattern as `tests/github_auth.rs`: a throwaway axum server on an
//! ephemeral localhost port, never the real network.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, post};
use crystalline_remote::RemoteError;
use crystalline_remote::github::auth::auth_base;
use crystalline_remote::github::web::{exchange_code, refresh, revoke_grant};
use tokio::net::TcpListener;

async fn spawn(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

type Captured = Arc<Mutex<Vec<serde_json::Value>>>;

fn token_endpoint(reply: serde_json::Value) -> (Router, Captured) {
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/login/oauth/access_token",
            post(
                move |State((captured, reply)): State<(Captured, serde_json::Value)>,
                      headers: HeaderMap,
                      body: String| async move {
                    assert_eq!(
                        headers.get("accept").and_then(|v| v.to_str().ok()),
                        Some("application/json")
                    );
                    captured
                        .lock()
                        .unwrap()
                        .push(serde_json::from_str(&body).unwrap());
                    Json(reply)
                },
            ),
        )
        .with_state((captured.clone(), reply));
    (app, captured)
}

#[tokio::test]
async fn exchange_code_sends_the_code_secret_and_verifier_and_reads_the_expiring_pair() {
    let (app, captured) = token_endpoint(serde_json::json!({
        "access_token": "ghu_access",
        "expires_in": 28800,
        "refresh_token": "ghr_refresh",
        "refresh_token_expires_in": 15811200,
        "token_type": "bearer",
        "scope": "",
    }));
    let base = spawn(app).await;

    let before = chrono::Utc::now();
    let tokens = exchange_code(
        &base,
        "Iv1.client",
        "s3cret",
        "the-code",
        "https://kb.example.com/api/v1/auth/github/callback",
        "the-verifier",
    )
    .await
    .unwrap();

    let sent = captured.lock().unwrap()[0].clone();
    assert_eq!(sent["client_id"], "Iv1.client");
    assert_eq!(sent["client_secret"], "s3cret");
    assert_eq!(sent["code"], "the-code");
    assert_eq!(sent["code_verifier"], "the-verifier");
    assert_eq!(
        sent["redirect_uri"],
        "https://kb.example.com/api/v1/auth/github/callback"
    );

    assert_eq!(tokens.access_token, "ghu_access");
    assert_eq!(tokens.refresh_token.as_deref(), Some("ghr_refresh"));
    let expires = tokens.expires_at.unwrap();
    assert!(expires >= before + chrono::Duration::seconds(28800));
    assert!(expires <= chrono::Utc::now() + chrono::Duration::seconds(28800));
    let refresh_expires = tokens.refresh_token_expires_at.unwrap();
    assert!(refresh_expires >= before + chrono::Duration::seconds(15811200));
}

#[tokio::test]
async fn exchange_code_accepts_a_non_expiring_token() {
    let (app, _) = token_endpoint(serde_json::json!({"access_token": "gho_forever"}));
    let base = spawn(app).await;

    let tokens = exchange_code(&base, "c", "s", "code", "https://r", "v")
        .await
        .unwrap();

    assert_eq!(tokens.access_token, "gho_forever");
    assert_eq!(tokens.refresh_token, None);
    assert_eq!(tokens.expires_at, None);
    assert_eq!(tokens.refresh_token_expires_at, None);
}

#[tokio::test]
async fn exchange_code_maps_an_http_200_error_body_to_api() {
    let (app, _) = token_endpoint(serde_json::json!({
        "error": "bad_verification_code",
        "error_description": "The code passed is incorrect or expired.",
    }));
    let base = spawn(app).await;

    let err = exchange_code(&base, "c", "hunter2", "code", "https://r", "v")
        .await
        .unwrap_err();

    match err {
        RemoteError::Api { status, message } => {
            assert_eq!(status, 200);
            assert!(message.contains("bad_verification_code"), "{message}");
            assert!(message.contains("incorrect or expired"), "{message}");
            assert!(!message.contains("hunter2"), "{message}");
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

#[tokio::test]
async fn exchange_code_maps_a_non_success_status_to_api() {
    let app = Router::new().route(
        "/login/oauth/access_token",
        post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
    );
    let base = spawn(app).await;

    let err = exchange_code(&base, "c", "s", "code", "https://r", "v")
        .await
        .unwrap_err();

    assert!(
        matches!(err, RemoteError::Api { status: 500, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn exchange_code_maps_connection_refused_to_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let err = exchange_code(
        &format!("http://{addr}"),
        "c",
        "s",
        "code",
        "https://r",
        "v",
    )
    .await
    .unwrap_err();

    assert!(matches!(err, RemoteError::Offline), "{err:?}");
}

#[tokio::test]
async fn exchange_code_rejects_a_reply_with_neither_a_token_nor_an_error() {
    let (app, _) = token_endpoint(serde_json::json!({}));
    let base = spawn(app).await;

    let err = exchange_code(&base, "c", "s", "code", "https://r", "v")
        .await
        .unwrap_err();

    assert!(matches!(err, RemoteError::Api { .. }), "{err:?}");
}

#[tokio::test]
async fn refresh_returns_the_rotated_refresh_token_and_the_new_expiry() {
    let (app, captured) = token_endpoint(serde_json::json!({
        "access_token": "ghu_new",
        "expires_in": 28800,
        "refresh_token": "ghr_rotated",
        "refresh_token_expires_in": 15811200,
    }));
    let base = spawn(app).await;

    let tokens = refresh(&base, "Iv1.client", "s3cret", "ghr_old")
        .await
        .unwrap();

    let sent = captured.lock().unwrap()[0].clone();
    assert_eq!(sent["grant_type"], "refresh_token");
    assert_eq!(sent["refresh_token"], "ghr_old");
    assert_eq!(sent["client_id"], "Iv1.client");
    assert_eq!(sent["client_secret"], "s3cret");

    assert_eq!(tokens.access_token, "ghu_new");
    assert_eq!(tokens.refresh_token.as_deref(), Some("ghr_rotated"));
    assert!(tokens.expires_at.unwrap() > chrono::Utc::now());
    assert!(tokens.refresh_token_expires_at.unwrap() > tokens.expires_at.unwrap());
}

#[tokio::test]
async fn refresh_maps_a_bad_refresh_token_to_auth_expired() {
    let (app, _) = token_endpoint(serde_json::json!({
        "error": "bad_refresh_token",
        "error_description": "The refresh token passed is incorrect or expired.",
    }));
    let base = spawn(app).await;

    let err = refresh(&base, "c", "s", "ghr_old").await.unwrap_err();

    assert!(matches!(err, RemoteError::AuthExpired), "{err:?}");
}

#[tokio::test]
async fn refresh_maps_other_error_bodies_to_api() {
    let (app, _) = token_endpoint(serde_json::json!({"error": "incorrect_client_credentials"}));
    let base = spawn(app).await;

    let err = refresh(&base, "c", "s", "ghr_old").await.unwrap_err();

    match err {
        RemoteError::Api { message, .. } => {
            assert!(
                message.contains("incorrect_client_credentials"),
                "{message}"
            );
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

#[tokio::test]
async fn the_token_endpoint_of_a_ghes_host_is_the_bare_host() {
    let (app, captured) = token_endpoint(serde_json::json!({"access_token": "t"}));
    let base = spawn(app).await;
    let api_url = format!("{base}/api/v3");

    let tokens = refresh(&auth_base(Some(&api_url)), "c", "s", "r")
        .await
        .unwrap();

    assert_eq!(tokens.access_token, "t");
    assert_eq!(captured.lock().unwrap().len(), 1);
}

#[derive(Clone, Default)]
struct Revoked {
    seen: Arc<Mutex<Vec<(String, String, String)>>>,
}

async fn revoke_server(status: StatusCode) -> (String, Revoked) {
    let revoked = Revoked::default();
    let app = Router::new()
        .route(
            "/applications/{client_id}/grant",
            delete(
                move |State(revoked): State<Revoked>,
                      axum::extract::Path(client_id): axum::extract::Path<String>,
                      headers: HeaderMap,
                      body: String| async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    revoked.seen.lock().unwrap().push((client_id, auth, body));
                    status
                },
            ),
        )
        .with_state(revoked.clone());
    (spawn(app).await, revoked)
}

#[tokio::test]
async fn revoke_grant_deletes_with_basic_auth_and_the_token_in_the_body() {
    let (base, revoked) = revoke_server(StatusCode::NO_CONTENT).await;

    revoke_grant(Some(&base), "Iv1.client", "s3cret", "ghu_access")
        .await
        .unwrap();

    let seen = revoked.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (client_id, auth, body) = &seen[0];
    assert_eq!(client_id, "Iv1.client");
    // base64("Iv1.client:s3cret")
    assert_eq!(auth, "Basic SXYxLmNsaWVudDpzM2NyZXQ=");
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["access_token"], "ghu_access");
}

#[tokio::test]
async fn revoke_grant_treats_a_missing_grant_as_done() {
    let (base, _) = revoke_server(StatusCode::NOT_FOUND).await;

    revoke_grant(Some(&base), "c", "s", "t").await.unwrap();
}

#[tokio::test]
async fn revoke_grant_reports_a_refusal() {
    let (base, _) = revoke_server(StatusCode::UNAUTHORIZED).await;

    let err = revoke_grant(Some(&base), "c", "s", "t").await.unwrap_err();

    assert!(
        matches!(err, RemoteError::Api { status: 401, .. }),
        "{err:?}"
    );
}
