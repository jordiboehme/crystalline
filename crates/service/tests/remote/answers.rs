//! The remote answers the hooks read: the routing model, `hook_status`, the
//! two origin reads, the etags, and the allow-list pin.

use serde_json::{Value, json};

use crate::fixture::{Options, RemoteServer};

fn cmd(name: &str) -> Value {
    json!({ "v": 1, "cmd": name })
}

/// Every tool the allow-list names has an arm behind it: a call with no
/// arguments is refused for its arguments or its target, never as a tool a
/// connected machine may not run.
#[tokio::test]
async fn every_listed_remote_tool_has_an_arm() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    for tool in crystalline_service::remote_ctl::REMOTE_TOOLS {
        let (_, reply) = server
            .ctl(
                Some(&keeper),
                json!({ "v": 1, "cmd": "tool", "tool": tool, "args": {} }),
            )
            .await;
        assert!(
            !reply
                .to_string()
                .contains("is not available over a remote connection"),
            "{tool}: {reply}"
        );
    }
}

#[tokio::test]
async fn routing_bullets_answers_the_routing_model_for_the_account() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server.ctl(Some(&keeper), cmd("routing_bullets")).await;
    assert_eq!(reply["ok"], true, "{reply}");
    let domains = reply["data"]["domains"].as_array().unwrap();
    let lab = domains
        .iter()
        .find(|d| d["name"] == "lab")
        .expect("the member sees lab");
    assert!(
        lab["bullets"].to_string().contains("confidential lab"),
        "{reply}"
    );
    assert_eq!(
        lab["origin"],
        serde_json::Value::Null,
        "a domain with no repository has no identity"
    );
    let platform = domains.iter().find(|d| d["name"] == "platform").unwrap();
    assert_eq!(
        platform["origin"],
        json!({ "forge": "github.com", "repository": "acme/platform", "path": "", "branch": "main" }),
        "a client recognizes the same domain by this (spec A2)"
    );
    assert_eq!(reply["data"]["read_only"], false);
    assert!(
        reply["etag"].as_str().is_some_and(|e| e.len() == 64),
        "{reply}"
    );

    let out = server.token_for("out").await;
    let (_, outsider) = server.ctl(Some(&out), cmd("routing_bullets")).await;
    assert!(!outsider.to_string().contains("lab"), "{outsider}");
    assert_ne!(
        reply["etag"], outsider["etag"],
        "callers with different rights get different answers and etags"
    );
}

/// The etag round trip: an unchanged answer is `not_modified`, a changed
/// MANIFEST is a new answer with a new etag.
#[tokio::test]
async fn the_routing_etag_round_trips() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, first) = server.ctl(Some(&keeper), cmd("routing_bullets")).await;
    let etag = first["etag"].as_str().unwrap().to_string();

    let (_, again) = server
        .ctl(
            Some(&keeper),
            json!({ "v": 1, "cmd": "routing_bullets", "if_none_match": etag }),
        )
        .await;
    assert_eq!(again["ok"], true);
    assert_eq!(again["not_modified"], true, "{again}");
    assert!(again.get("data").is_none(), "nothing is resent: {again}");

    let manifest = server.file("open", "MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replace("shared questions", "shared and new questions"),
    )
    .unwrap();
    let (_, changed) = server
        .ctl(
            Some(&keeper),
            json!({ "v": 1, "cmd": "routing_bullets", "if_none_match": etag }),
        )
        .await;
    assert!(changed.get("not_modified").is_none(), "{changed}");
    assert_ne!(changed["etag"], json!(etag));
    assert!(
        changed["data"]
            .to_string()
            .contains("shared and new questions")
    );
}

#[tokio::test]
async fn hook_status_answers_the_backlog_the_account_may_see() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let _guard = crate::support::maintenance_guard().await;
    crystalline_service::maintenance::save(&crystalline_service::maintenance::MaintenanceState {
        pending_domains: vec!["open".to_string(), "lab".to_string()],
        last_run_at: chrono::DateTime::from_timestamp(1_800_000_000, 0),
        ..Default::default()
    })
    .unwrap();

    let keeper = server.token_for("keeper").await;
    let (_, reply) = server.ctl(Some(&keeper), cmd("hook_status")).await;
    assert_eq!(
        reply["data"]["evolve"]["pending_domains"],
        json!(["open", "lab"]),
        "{reply}"
    );
    let etag = reply["etag"].as_str().unwrap().to_string();
    let (_, again) = server
        .ctl(
            Some(&keeper),
            json!({ "v": 1, "cmd": "hook_status", "if_none_match": etag }),
        )
        .await;
    assert_eq!(again["not_modified"], true, "{again}");

    let out = server.token_for("out").await;
    let (_, reply) = server.ctl(Some(&out), cmd("hook_status")).await;
    assert_eq!(
        reply["data"]["evolve"]["pending_domains"],
        json!(["open"]),
        "{reply}"
    );
}

/// A write to a domain the caller cannot see does not move the caller's
/// `hook_status`: no pending time, the same etag.
#[tokio::test]
async fn a_write_to_a_hidden_domain_does_not_move_the_hook_status() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let _guard = crate::support::maintenance_guard().await;
    crystalline_service::maintenance::save(&Default::default()).unwrap();
    let out = server.token_for("out").await;
    let (_, before) = server.ctl(Some(&out), cmd("hook_status")).await;

    crystalline_service::maintenance::save(&crystalline_service::maintenance::MaintenanceState {
        pending_domains: vec!["lab".to_string()],
        pending_since: chrono::DateTime::from_timestamp(1_800_000_000, 0),
        ..Default::default()
    })
    .unwrap();
    let (_, after) = server.ctl(Some(&out), cmd("hook_status")).await;
    assert_eq!(
        after["data"]["evolve"]["pending_since"],
        Value::Null,
        "{after}"
    );
    assert_eq!(before["etag"], after["etag"], "{before} {after}");
}

/// Review focus 5: an account outside a private domain learns nothing of it
/// from any allow-listed answer.
#[tokio::test]
async fn a_private_domain_stays_out_of_every_remote_answer() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let _guard = crate::support::maintenance_guard().await;
    crystalline_service::maintenance::save(&crystalline_service::maintenance::MaintenanceState {
        pending_domains: vec!["lab".to_string()],
        ..Default::default()
    })
    .unwrap();
    let out = server.token_for("out").await;
    for request in [
        cmd("status"),
        cmd("routing_bullets"),
        cmd("hook_status"),
        json!({ "v": 1, "cmd": "tool", "tool": "list_domains", "args": {} }),
        json!({ "v": 1, "cmd": "tool", "tool": "search_engrams", "args": { "query": "vent" } }),
    ] {
        let (_, reply) = server.ctl(Some(&out), request.clone()).await;
        let text = reply.to_string();
        assert!(
            !text.contains("\"lab\"")
                && !text.contains("lab-note")
                && !text.contains("confidential"),
            "{request}: {reply}"
        );
    }
    let (_, hidden) = server
        .ctl(
            Some(&out),
            json!({ "v": 1, "cmd": "origin_changes", "domain": "lab" }),
        )
        .await;
    let (_, unknown) = server
        .ctl(
            Some(&out),
            json!({ "v": 1, "cmd": "origin_changes", "domain": "nosuch" }),
        )
        .await;
    assert_eq!(
        hidden["error"].as_str().unwrap().replace("lab", "X"),
        unknown["error"].as_str().unwrap().replace("nosuch", "X"),
        "a private domain answers like a domain nobody registered"
    );
}

/// The origin reads run as the account: this instance has collaboration off,
/// so the answer is the engine's own refusal, not the allow-list's.
#[tokio::test]
async fn origin_status_reaches_the_engine_under_the_account() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server.ctl(Some(&keeper), cmd("origin_status")).await;
    assert_eq!(reply["ok"], false);
    let error = reply["error"].as_str().unwrap();
    assert!(
        !error.contains("not available over a remote connection"),
        "{error}"
    );
    assert!(error.contains("github.enabled"), "{error}");
}
