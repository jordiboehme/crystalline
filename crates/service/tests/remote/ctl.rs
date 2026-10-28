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
        "holder",
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
        "github_app_status",
        "github_app_changed",
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
    assert_eq!(
        REMOTE_COMMANDS,
        &[
            "status",
            "tool",
            "routing_bullets",
            "hook_status",
            "origin_status",
            "origin_changes",
            "file",
            "automation_status",
            "automation_lease_take",
            "automation_lease_renew",
            "automation_lease_release",
            "automation_run_record",
            "automation_share",
            "automation_runs"
        ]
    );
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
            "reflect",
            "read_reflection",
            "write_reflection",
            "discuss",
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

/// One raw POST to the ctl path with no token that declares `declared` body
/// bytes and sends only a few of them, answered as its status line (or "no
/// answer" when none arrives within three seconds).
async fn declared_post(server: &RemoteServer, declared: usize) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let mut request = format!(
        "POST /api/v1/ctl HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n",
        server.addr
    );
    request.push_str("\r\n{\"v\": 1");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let read = tokio::time::timeout(std::time::Duration::from_secs(3), stream.read(&mut buf))
        .await
        .map(|n| n.unwrap_or(0))
        .unwrap_or(0);
    String::from_utf8_lossy(&buf[..read])
        .lines()
        .next()
        .unwrap_or("no answer")
        .to_string()
}

/// The body is read only after the bearer check: a caller with no token is
/// refused at once, before a byte of the body it announced has to arrive.
#[tokio::test]
async fn the_body_is_read_only_after_the_bearer_check() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let oversized = crystalline_service::rest::MAX_BODY_BYTES + 1;
    let status = declared_post(&server, oversized).await;
    assert!(status.starts_with("HTTP/1.1 401"), "{status}");
}

fn verb(cmd: &str, rest: serde_json::Value) -> serde_json::Value {
    let mut body = rest;
    body["v"] = json!(1);
    body["cmd"] = json!(cmd);
    body
}

/// A runner's whole cycle over the route with a `cmt_` token: status, take,
/// renew, record, release, and the run in the history. The take names the
/// token's account as the lease holder, which is what an MCP write is noted
/// against, and a second take is refused with the bare sentence.
#[tokio::test]
async fn a_token_runs_a_full_cycle_over_the_ctl_route() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = server.token_for("keeper").await;
    assert!(token.starts_with("cmt_"), "{token}");
    let ask = |body: serde_json::Value| {
        let server = &server;
        let token = token.clone();
        async move { server.ctl(Some(&token), body).await.1 }
    };

    let status = ask(verb(
        "automation_status",
        json!({ "loop": "evolve", "domains": ["open"] }),
    ))
    .await;
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["data"]["domains"][0]["domain"], "open", "{status}");
    assert!(status["data"]["domains"][0]["due"].is_boolean(), "{status}");

    let take = ask(verb(
        "automation_lease_take",
        json!({ "loop": "evolve", "domain": "open", "runner": "k8s" }),
    ))
    .await;
    assert_eq!(take["ok"], true, "{take}");
    assert_eq!(take["data"]["renew_secs"], 60, "{take}");
    let lease = take["data"]["token"]
        .as_str()
        .expect("the take carries the token");

    let store = server.engine.store();
    let account = {
        let store = store.lock().await;
        let id = store.domain_id("open").await.unwrap().unwrap();
        store
            .automation_lease(id, "evolve")
            .await
            .unwrap()
            .unwrap()
            .holder
            .account
    };
    assert_eq!(account.as_deref(), Some("keeper"));

    let again = ask(verb(
        "automation_lease_take",
        json!({ "loop": "evolve", "domain": "open", "runner": "second" }),
    ))
    .await;
    assert_eq!(again["ok"], false, "{again}");
    assert_eq!(again["error"], "already running", "{again}");

    let renew = ask(verb(
        "automation_lease_renew",
        json!({ "loop": "evolve", "domain": "open", "token": lease }),
    ))
    .await;
    assert_eq!(renew["data"]["renewed"], true, "{renew}");

    let record = ask(verb(
        "automation_run_record",
        json!({ "loop": "evolve", "domain": "open", "token": lease,
                "report": { "started_at": "", "outcome": "done", "done": 2, "summary": "fixed two" } }),
    ))
    .await;
    assert_eq!(record["ok"], true, "{record}");

    // The record released the lease; a release now finds nothing to free.
    let release = ask(verb(
        "automation_lease_release",
        json!({ "loop": "evolve", "domain": "open", "token": lease }),
    ))
    .await;
    assert_eq!(release["data"]["released"], false, "{release}");

    let runs = ask(verb(
        "automation_runs",
        json!({ "loop": "evolve", "domain": "open", "limit": 5 }),
    ))
    .await;
    assert_eq!(runs["data"]["runs"][0]["summary"], "fixed two", "{runs}");
    assert_eq!(runs["data"]["runs"][0]["done"], 2, "{runs}");

    let bad = ask(verb(
        "automation_runs",
        json!({ "loop": "sweep", "domain": "open" }),
    ))
    .await;
    assert_eq!(bad["ok"], false, "{bad}");
    assert_eq!(bad["kind"], "invalid", "{bad}");
}

/// A viewer may read `open` but not write it, so it may not run its evolve
/// loop: the unnamed status lists nothing and a take is refused.
#[tokio::test]
async fn a_token_without_curation_rights_sees_nothing() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = server.token_for("looker").await;
    let (_, status) = server
        .ctl(
            Some(&token),
            verb("automation_status", json!({ "loop": "evolve" })),
        )
        .await;
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["data"]["domains"], json!([]), "{status}");
    let (_, take) = server
        .ctl(
            Some(&token),
            verb(
                "automation_lease_take",
                json!({ "loop": "evolve", "domain": "open", "runner": "k8s" }),
            ),
        )
        .await;
    assert_eq!(take["ok"], false, "{take}");
    assert!(take.get("data").is_none(), "{take}");
}

/// A private domain the caller may not see is refused with the very text an
/// unregistered name gets, so the refusal does not tell them apart.
#[tokio::test]
async fn a_hidden_domain_is_refused_like_an_unknown_one() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = server.token_for("out").await;
    let mut texts = Vec::new();
    for domain in ["lab", "nosuch"] {
        let (_, reply) = server
            .ctl(
                Some(&token),
                verb(
                    "automation_lease_take",
                    json!({ "loop": "evolve", "domain": domain, "runner": "k8s" }),
                ),
            )
            .await;
        assert_eq!(reply["ok"], false, "{reply}");
        texts.push(reply["error"].as_str().unwrap().replace(domain, "X"));
    }
    assert_eq!(texts[0], texts[1], "{texts:?}");
}
