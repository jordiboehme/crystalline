//! RFC 7009 revocation, which `crystalline disconnect` uses to end the grant
//! it signed in with.

use serde_json::json;

use crate::fixture::{Options, RemoteServer};

#[tokio::test]
async fn revoking_the_refresh_token_ends_the_whole_grant() {
    let server = RemoteServer::start(Options::OAUTH).await;
    let pair = server.oauth_pair("keeper").await;
    let (code, _) = server
        .ctl(Some(&pair.access), json!({ "v": 1, "cmd": "status" }))
        .await;
    assert_eq!(code, 200, "the grant works before");

    let revoked = server
        .revoke(&[
            ("token", &pair.refresh),
            ("token_type_hint", "refresh_token"),
            ("client_id", &pair.client_id),
        ])
        .await;
    assert_eq!(revoked.status(), 200);
    assert_eq!(
        revoked
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let (code, _) = server
        .ctl(Some(&pair.access), json!({ "v": 1, "cmd": "status" }))
        .await;
    assert_eq!(code, 401, "both tokens of the grant stopped at once");
    assert!(
        server
            .auth
            .list_oauth_grants("keeper")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_unknown_or_foreign_token_is_answered_the_same_and_changes_nothing() {
    let server = RemoteServer::start(Options::OAUTH).await;
    let pair = server.oauth_pair("keeper").await;
    for form in [
        vec![
            ("token", "cor_nothing_like_it"),
            ("client_id", pair.client_id.as_str()),
        ],
        vec![
            ("token", pair.refresh.as_str()),
            ("client_id", "coc_somebody_else"),
        ],
        vec![
            ("token", "cmt_a_personal_token"),
            ("client_id", pair.client_id.as_str()),
        ],
    ] {
        let response = server.revoke(&form).await;
        assert_eq!(response.status(), 200, "{form:?}");
    }
    assert_eq!(
        server.auth.list_oauth_grants("keeper").await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_revocation_without_a_token_or_a_client_is_an_invalid_request() {
    let server = RemoteServer::start(Options::OAUTH).await;
    for form in [vec![("client_id", "coc_x")], vec![("token", "cor_x")]] {
        let response = server.revoke(&form).await;
        assert_eq!(response.status(), 400, "{form:?}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"], "invalid_request");
    }
}

#[tokio::test]
async fn revocation_is_gone_where_oauth_is_off() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let response = server
        .revoke(&[("token", "cor_x"), ("client_id", "coc_x")])
        .await;
    assert_eq!(response.status(), 404);
}
