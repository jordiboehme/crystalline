//! The automation verbs on the control socket: the machine owner runs the
//! whole cycle with no account, and a refusal keeps its bare sentence.

use crystalline_service::control::answer_for_test;
use serde_json::{Value, json};

use crate::fixture::{Fx, fixture, fixture_with, manifest};

fn verb(cmd: &str, rest: Value) -> Value {
    let mut body = rest;
    body["v"] = json!(1);
    body["cmd"] = json!(cmd);
    body
}

async fn ask(fx: &Fx, cmd: &str, rest: Value) -> Value {
    answer_for_test(fx.engine.clone(), &verb(cmd, rest)).await
}

#[tokio::test]
async fn the_local_socket_runs_a_full_cycle_without_an_account() {
    let fx = fixture(&[("MANIFEST.md", &manifest(""))]).await;

    let status = ask(
        &fx,
        "automation_status",
        json!({ "loop": "evolve", "domains": ["eng"] }),
    )
    .await;
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["data"]["domains"][0]["domain"], "eng", "{status}");

    let take = ask(
        &fx,
        "automation_lease_take",
        json!({ "loop": "evolve", "domain": "eng", "runner": "cron" }),
    )
    .await;
    assert_eq!(take["ok"], true, "{take}");
    assert_eq!(take["data"]["renew_secs"], 60, "{take}");
    let token = take["data"]["token"]
        .as_str()
        .expect("the take carries the token");

    let again = ask(
        &fx,
        "automation_lease_take",
        json!({ "loop": "evolve", "domain": "eng", "runner": "second" }),
    )
    .await;
    assert_eq!(again["error"], "already running", "{again}");

    let renew = ask(
        &fx,
        "automation_lease_renew",
        json!({ "loop": "evolve", "domain": "eng", "token": token }),
    )
    .await;
    assert_eq!(renew["data"]["renewed"], true, "{renew}");

    let share = ask(
        &fx,
        "automation_share",
        json!({ "loop": "evolve", "domain": "eng", "token": token, "summary": "s" }),
    )
    .await;
    assert_eq!(share["ok"], true, "{share}");
    assert_eq!(share["data"]["attributed"], false, "{share}");

    let record = ask(
        &fx,
        "automation_run_record",
        json!({ "loop": "evolve", "domain": "eng", "token": token,
                "report": { "started_at": "", "outcome": "done", "done": 1, "summary": "ok" } }),
    )
    .await;
    assert_eq!(record["ok"], true, "{record}");

    let release = ask(
        &fx,
        "automation_lease_release",
        json!({ "loop": "evolve", "domain": "eng", "token": token }),
    )
    .await;
    assert_eq!(release["data"]["released"], false, "{release}");

    let runs = ask(
        &fx,
        "automation_runs",
        json!({ "loop": "evolve", "domain": "eng" }),
    )
    .await;
    assert_eq!(runs["data"]["runs"][0]["summary"], "ok", "{runs}");
}

/// A domain named by one of its aliases reaches the same domain.
#[tokio::test]
async fn a_domain_named_by_alias_is_localized_on_the_socket() {
    let fx = fixture_with(&[("MANIFEST.md", &manifest(""))], |cfg, _| {
        cfg.domains.get_mut("eng").unwrap().aliases = vec!["old-eng".to_string()];
    })
    .await;
    let take = ask(
        &fx,
        "automation_lease_take",
        json!({ "loop": "evolve", "domain": "old-eng", "runner": "cron" }),
    )
    .await;
    assert_eq!(take["ok"], true, "{take}");
    let status = ask(
        &fx,
        "automation_status",
        json!({ "loop": "evolve", "domains": ["old-eng"] }),
    )
    .await;
    assert_eq!(status["data"]["domains"][0]["domain"], "eng", "{status}");
    assert!(
        status["data"]["domains"][0]["running"].is_object(),
        "{status}"
    );
}

#[tokio::test]
async fn a_status_names_at_most_a_hundred_domains_and_a_summary_is_a_string() {
    let fx = fixture(&[("MANIFEST.md", &manifest(""))]).await;
    let many: Vec<String> = (0..101).map(|i| format!("d{i}")).collect();
    let status = ask(
        &fx,
        "automation_status",
        json!({ "loop": "evolve", "domains": many }),
    )
    .await;
    assert_eq!(status["ok"], false, "{status}");
    assert_eq!(status["kind"], "invalid", "{status}");
    let share = ask(
        &fx,
        "automation_share",
        json!({ "loop": "evolve", "domain": "eng", "token": "t", "summary": 5 }),
    )
    .await;
    assert_eq!(share["kind"], "invalid", "{share}");
    assert_eq!(share["error"], "summary must be a string", "{share}");
}

/// `force` on a status asks past the wait after failed runs, for a run a
/// person starts by hand; anything but a boolean is refused.
#[tokio::test]
async fn a_status_takes_force_as_a_boolean() {
    let fx = fixture(&[("MANIFEST.md", &manifest(""))]).await;
    let forced = ask(
        &fx,
        "automation_status",
        json!({ "loop": "evolve", "domains": ["eng"], "force": true }),
    )
    .await;
    assert_eq!(forced["ok"], true, "{forced}");
    let wrong = ask(
        &fx,
        "automation_status",
        json!({ "loop": "evolve", "domains": ["eng"], "force": "yes" }),
    )
    .await;
    assert_eq!(wrong["kind"], "invalid", "{wrong}");
    assert_eq!(wrong["error"], "force must be true or false", "{wrong}");
}

/// `domain_register` takes an absolute folder only: the daemon works in a
/// folder of its own, so a relative one would name another place there.
#[tokio::test]
async fn domain_register_refuses_a_relative_folder() {
    let fx = fixture(&[("MANIFEST.md", &manifest(""))]).await;
    let answer = ask(
        &fx,
        "domain_register",
        json!({ "kind": "file", "name": "kb", "path": "kb" }),
    )
    .await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"].as_str().unwrap().contains("absolute"),
        "{answer}"
    );
}

/// An unknown maintenance stamp is refused by name.
#[tokio::test]
async fn an_unknown_maintenance_stamp_is_refused() {
    let fx = fixture(&[("MANIFEST.md", &manifest(""))]).await;
    let answer = ask(&fx, "maintenance_stamp", json!({ "stamp": "bogus" })).await;
    assert_eq!(answer["ok"], false, "{answer}");
}
