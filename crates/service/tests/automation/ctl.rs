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

/// `automation_prompt` answers what a runner starts its own agent with, the
/// same for both loops: the filled-in prompt, the forbidden tools in order,
/// this version and the checksum of the prompt and the tools.
#[tokio::test]
async fn automation_prompt_answers_the_prompt_the_tools_and_a_checksum() {
    use crystalline_core::automation::{HEADLESS_DISALLOWED_TOOLS, Loop, runner_setup};
    let fx = fixture(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    for lp in [Loop::Reflect, Loop::Evolve] {
        let answer = ask(
            &fx,
            "automation_prompt",
            json!({ "loop": lp, "domain": "eng", "max_items": 7 }),
        )
        .await;
        assert_eq!(answer["ok"], true, "{answer}");
        let data = &answer["data"];
        assert_eq!(
            data,
            &serde_json::to_value(runner_setup(lp, "eng", 7, false)).unwrap(),
            "{lp}"
        );
        assert!(
            data["prompt"]
                .as_str()
                .unwrap()
                .contains("Stop after 7 items")
        );
        assert_eq!(data["disallowed_tools"], json!(HEADLESS_DISALLOWED_TOOLS));
        assert_eq!(data["version"], env!("CARGO_PKG_VERSION"));
        assert!(
            data["checksum"].as_str().unwrap().starts_with("sha256:"),
            "{data}"
        );
        // The same request answers the same sum.
        let again = ask(
            &fx,
            "automation_prompt",
            json!({ "loop": lp, "domain": "eng", "max_items": 7 }),
        )
        .await;
        assert_eq!(again["data"]["checksum"], data["checksum"], "{lp}");
    }
    // Without a cap the default of a run applies.
    let plain = ask(
        &fx,
        "automation_prompt",
        json!({ "loop": "evolve", "domain": "eng" }),
    )
    .await;
    assert!(
        plain["data"]["prompt"]
            .as_str()
            .unwrap()
            .contains("Stop after 20 items"),
        "{plain}"
    );
}

/// A cap that is no whole number in range, a `skills_loaded` that is no
/// boolean, a missing domain and an unknown one are refused.
#[tokio::test]
async fn automation_prompt_refuses_a_bad_cap_and_an_unknown_domain() {
    let fx = fixture(&[("MANIFEST.md", &manifest(""))]).await;
    for cap in [json!(-1), json!(1.5), json!("7"), json!(4_294_967_296u64)] {
        let answer = ask(
            &fx,
            "automation_prompt",
            json!({ "loop": "evolve", "domain": "eng", "max_items": cap }),
        )
        .await;
        assert_eq!(answer["kind"], "invalid", "{cap}: {answer}");
        assert_eq!(
            answer["error"], "max_items must be a whole number from 0 to 4294967295",
            "{answer}"
        );
    }
    let top = ask(
        &fx,
        "automation_prompt",
        json!({ "loop": "evolve", "domain": "eng", "max_items": 4_294_967_295u64 }),
    )
    .await;
    assert_eq!(top["ok"], true, "{top}");
    let loaded = ask(
        &fx,
        "automation_prompt",
        json!({ "loop": "evolve", "domain": "eng", "skills_loaded": "yes" }),
    )
    .await;
    assert_eq!(loaded["kind"], "invalid", "{loaded}");
    assert_eq!(loaded["error"], "skills_loaded must be true or false");
    let missing = ask(&fx, "automation_prompt", json!({ "loop": "evolve" })).await;
    assert_eq!(missing["error"], "domain is required", "{missing}");
    let unknown = ask(
        &fx,
        "automation_prompt",
        json!({ "loop": "evolve", "domain": "nosuch" }),
    )
    .await;
    let take = ask(
        &fx,
        "automation_lease_take",
        json!({ "loop": "evolve", "domain": "nosuch", "runner": "cron" }),
    )
    .await;
    assert_eq!(unknown["ok"], false, "{unknown}");
    assert_eq!(unknown["error"], take["error"], "{unknown} {take}");
    let wrong_loop = ask(
        &fx,
        "automation_prompt",
        json!({ "loop": "sweep", "domain": "eng" }),
    )
    .await;
    assert_eq!(wrong_loop["kind"], "invalid", "{wrong_loop}");
}

/// The prompt's start follows this instance's `skills.serve`: it calls the
/// `skills` tool where the tool is served, says the skills are loaded where
/// `false` hides it, and `skills_loaded` overrides the setting both ways.
#[tokio::test]
async fn automation_prompt_picks_the_skills_line_from_the_setting_and_the_request() {
    use crystalline_core::automation::{Loop, headless_prompt};
    use crystalline_core::config::{SkillsConfig, SkillsServe};
    for (serve, served) in [
        (None, true),
        (Some(SkillsServe::Auto), true),
        (Some(SkillsServe::Always), true),
        (Some(SkillsServe::Never), false),
    ] {
        let fx = fixture_with(&[("MANIFEST.md", &manifest(""))], |cfg, _| {
            cfg.skills = serve.map(|serve| SkillsConfig { serve: Some(serve) });
        })
        .await;
        for (requested, loaded) in [
            (Value::Null, !served),
            (json!(true), true),
            (json!(false), false),
        ] {
            let answer = ask(
                &fx,
                "automation_prompt",
                json!({ "loop": "evolve", "domain": "eng", "max_items": 7,
                        "skills_loaded": requested }),
            )
            .await;
            assert_eq!(
                answer["data"]["prompt"],
                headless_prompt(Loop::Evolve, "eng", 7, loaded),
                "serve {serve:?}, skills_loaded {requested}"
            );
        }
    }
}

/// `reflect_mark_stale` on the control socket runs as the machine owner:
/// one page, every page, the owned pages unless `only_page`, and the
/// engine's sentences for nothing named and for a bad request.
#[tokio::test]
async fn reflect_mark_stale_marks_as_the_machine_owner() {
    let fx = fixture(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    let owner = crystalline_service::ReflectionCaller::owner();
    for path in ["a.md", "b.md"] {
        fx.engine
            .write_reflection(
                &crystalline_service::params::WriteReflectionParams {
                    domain: "eng".into(),
                    path: path.into(),
                    markdown: Some("# A\n\n## One\n\nText.\n".into()),
                    ..Default::default()
                },
                &owner,
            )
            .await
            .unwrap();
    }
    let one = ask(
        &fx,
        "reflect_mark_stale",
        json!({ "domain": "eng", "pages": ["a.md"], "only_page": true }),
    )
    .await;
    assert_eq!(one["ok"], true, "{one}");
    assert_eq!(one["data"]["pages"], json!(["a.md"]), "{one}");
    assert_eq!(one["data"]["by"], owner.author.name.as_str());
    assert_eq!(one["data"]["only_page"], true);
    let all = ask(
        &fx,
        "reflect_mark_stale",
        json!({ "domain": "eng", "all": true }),
    )
    .await;
    assert_eq!(all["data"]["pages"], json!(["a.md", "b.md"]), "{all}");
    let none = ask(&fx, "reflect_mark_stale", json!({ "domain": "eng" })).await;
    assert_eq!(none["ok"], false, "{none}");
    assert!(
        none["error"]
            .as_str()
            .unwrap()
            .contains("name the pages to mark"),
        "{none}"
    );
    let both = ask(
        &fx,
        "reflect_mark_stale",
        json!({ "domain": "eng", "all": true, "pages": ["a.md"] }),
    )
    .await;
    assert_eq!(both["ok"], false, "{both}");
    let gone = ask(
        &fx,
        "reflect_mark_stale",
        json!({ "domain": "eng", "pages": ["gone.md"] }),
    )
    .await;
    assert_eq!(gone["ok"], false, "{gone}");
}
