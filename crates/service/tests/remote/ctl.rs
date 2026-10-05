//! `POST /api/v1/ctl`: the door, the allow-list and the remote `status`.

use serde_json::json;

use crate::fixture::{Options, RemoteServer};

fn status() -> serde_json::Value {
    json!({ "v": 1, "cmd": "status" })
}

/// There is no anonymous ctl. The open tier serves MCP to anybody who can
/// reach the port, but the control protocol needs a personal token even
/// there, and a token that resolves to nobody gets the same `401`.
#[tokio::test]
async fn the_ctl_route_needs_a_token_even_on_the_open_tier() {
    let server = RemoteServer::start(Options::OPEN).await;
    let (status_code, _) = server.ctl(None, status()).await;
    assert_eq!(status_code, 401, "no token, no ctl");
    let (status_code, _) = server.ctl(Some("cmt_not_a_real_token"), status()).await;
    assert_eq!(
        status_code, 401,
        "a token nobody issued is the same refusal"
    );
    let token = server.token_for("keeper").await;
    let (status_code, reply) = server.ctl(Some(&token), status()).await;
    assert_eq!(status_code, 200, "{reply}");
    assert_eq!(reply["ok"], true, "{reply}");
}

/// A valid token answers `status` as its own account, and what comes back is
/// what that account may know: no process details, no server paths, no
/// private domain it is not a member of.
#[tokio::test]
async fn a_valid_token_reaches_status_under_its_own_account() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server.ctl(Some(&keeper), status()).await;
    let data = &reply["data"];
    assert_eq!(data["account"], "keeper", "{reply}");
    assert_eq!(data["admin"], false, "{reply}");
    assert_eq!(data["version"], crystalline_core::VERSION, "{reply}");
    let names: Vec<&str> = data["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"open") && names.contains(&"lab"),
        "{names:?}"
    );
    for private_fact in ["pid", "runs_in", "allowed_hosts", "sessions", "uptime_secs"] {
        assert!(
            data.get(private_fact).is_none(),
            "{private_fact} leaks: {reply}"
        );
    }
    assert!(
        !reply
            .to_string()
            .contains(server.tmp.path().to_str().unwrap()),
        "no server path in the answer: {reply}"
    );

    let out = server.token_for("out").await;
    let (_, reply) = server.ctl(Some(&out), status()).await;
    assert!(
        !reply.to_string().contains("\"lab\""),
        "an outsider does not learn the private domain's name: {reply}"
    );
}

/// Every command a local operator has is refused remotely, with the one
/// sentence, and refusing does nothing: `shutdown` leaves the server up.
#[tokio::test]
async fn every_command_off_the_allow_list_is_refused_remotely() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let boss = server.token_for("boss").await;
    for cmd in [
        "configure",
        "shutdown",
        "provision",
        "reindex",
        "sync",
        "sessions",
        "file_stamps",
        "collect_orphaned_domains",
        "name_report",
        "fix_local_spellings",
        "scaffold_manifest",
        "domain_import",
        "domain_export",
        "domain_remove",
        "domain_review",
        "domain_rename",
        "retag",
        "origin_add",
        "origin_update",
        "origin_share",
        "origin_withdraw",
        "origin_discard",
        "origin_resolve",
        "forget_domain",
        "forget_credential",
        "a_command_from_the_future",
    ] {
        let (code, reply) = server.ctl(Some(&boss), json!({ "v": 1, "cmd": cmd })).await;
        assert_eq!(code, 200, "{cmd}: a refusal is an envelope, not a status");
        assert_eq!(reply["ok"], false, "{cmd}: {reply}");
        assert_eq!(
            reply["error"],
            format!("'{cmd}' is not available over a remote connection"),
            "{cmd}: {reply}"
        );
    }
    let (code, reply) = server.ctl(Some(&boss), status()).await;
    assert_eq!(
        (code, reply["ok"].clone()),
        (200, json!(true)),
        "still up: {reply}"
    );
}

/// The route shares `/api/v1` with the JSON API: the API's own routes still
/// answer, and the ctl path is POST only.
#[tokio::test]
async fn the_ctl_route_and_the_rest_api_share_the_prefix() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let me = server
        .http
        .get(format!("{}/api/v1/auth/me", server.origin()))
        .send()
        .await
        .unwrap();
    // `/auth/me` is one of the API's public paths: it answers an anonymous
    // caller with its own JSON, where the MCP transport would refuse it.
    assert_eq!(
        me.status(),
        200,
        "the JSON API still answers under the prefix"
    );
    let me: serde_json::Value = me.json().await.unwrap();
    assert!(me.is_object(), "{me}");
    let get = server
        .http
        .get(format!("{}/api/v1/ctl", server.origin()))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 405, "the control protocol is POST only");
}

/// A body that is not JSON is the control protocol's own envelope error, the
/// same words the socket answers.
#[tokio::test]
async fn a_bad_body_is_an_envelope_error() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = server.token_for("keeper").await;
    let response = server
        .http
        .post(format!("{}/api/v1/ctl", server.origin()))
        .bearer_auth(&token)
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let reply: serde_json::Value = response.json().await.unwrap();
    assert_eq!(reply["ok"], false);
    assert!(
        reply["error"].as_str().unwrap().starts_with("invalid json"),
        "{reply}"
    );
}

/// The two allow-lists, exactly: a command or a tool reaches a connected
/// machine only by being added here on purpose.
#[test]
fn the_remote_allow_lists_are_pinned() {
    use crystalline_service::remote_ctl::{REMOTE_COMMANDS, REMOTE_TOOLS};
    assert_eq!(REMOTE_COMMANDS, &["status", "tool"]);
    assert_eq!(
        REMOTE_TOOLS,
        &[
            "write_engram",
            "read_engram",
            "edit_engram",
            "move_engram",
            "split_engram",
            "delete_engram",
            "search_engrams",
            "build_context",
            "recent_activity",
            "list_domains",
            "browse_domain",
            "validate_engrams",
            "infer_schema",
            "vocabulary",
            "evolve_engrams",
        ]
    );
}

/// An OAuth access token the store really holds is no credential here while
/// `auth.oauth` is off, exactly as at the MCP door.
#[tokio::test]
async fn an_oauth_token_is_refused_at_the_ctl_route_while_oauth_is_off() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let client = server
        .auth
        .register_oauth_client(
            "a hosted client",
            None,
            &["https://claude.ai/api/mcp/auth_callback".to_string()],
        )
        .await
        .unwrap();
    let grant = server
        .auth
        .issue_oauth_grant("keeper", &client.client_id, &server.origin())
        .await
        .unwrap();
    assert!(grant.access_token.starts_with("coa_"));
    let (status_code, _) = server.ctl(Some(&grant.access_token), status()).await;
    assert_eq!(
        status_code, 401,
        "a coa_ token is refused while OAuth is off"
    );
}

/// The body is read only after the bearer check: a caller with no token is
/// refused at the door whatever it sends, and a body past the API's ceiling
/// from a caller with one is the envelope's error, not a buffered request.
#[tokio::test]
async fn the_body_is_read_only_after_the_bearer_check() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let oversized = "x".repeat(crystalline_service::rest::MAX_BODY_BYTES + 1);
    let refused = server
        .http
        .post(format!("{}/api/v1/ctl", server.origin()))
        .body(oversized.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401, "no token, no body read");
    let token = server.token_for("keeper").await;
    let response = server
        .http
        .post(format!("{}/api/v1/ctl", server.origin()))
        .bearer_auth(&token)
        .body(oversized)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let reply: serde_json::Value = response.json().await.unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"].as_str().unwrap().starts_with("invalid body"),
        "{reply}"
    );
}
