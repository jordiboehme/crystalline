//! The reflect queue.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crystalline_service::ReflectionCaller;
use crystalline_service::params::{DiscussParams, ReflectParams};

use crate::docs::{agent, person, write};
use crate::fixture::{Fx, engram, fixture, manifest};

async fn curated(extra: &[(&str, &str)]) -> Fx {
    let mut files = vec![
        ("MANIFEST.md", manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
        (
            "runbooks/rollback.md",
            engram(
                "Rollback",
                "runbooks/rollback",
                "- [fact] roll back with the script",
            ),
        ),
    ];
    files.extend(extra.iter().map(|(p, c)| (*p, c.to_string())));
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
    let fx = fixture(&refs).await;
    let mut p = write(
        "guides/deploying.md",
        "# Deploying\n\n## Night deploys\n\nDeploys run at night.\n\n## Rollback\n\nRoll back with the script.\n",
    );
    // `_top` with no sources names the top as curated. An agent's curated
    // write reflects the top it wrote without it too, so the queue starts
    // empty either way; the key keeps naming the top explicitly covered.
    p.sources = BTreeMap::from([
        ("_top".to_string(), Vec::new()),
        (
            "Night deploys".to_string(),
            vec!["runbooks/deploy".to_string()],
        ),
        (
            "Rollback".to_string(),
            vec!["runbooks/rollback".to_string()],
        ),
    ]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    if extra.is_empty() {
        let empty = fx
            .engine
            .reflect(&ReflectParams::default(), &agent())
            .await
            .unwrap();
        assert_eq!(
            empty["total"], 0,
            "a freshly curated reflection owes nothing: {empty}"
        );
    }
    fx
}

/// A curated reflection with one person's change waiting as a feedback item.
pub async fn curated_with_a_feedback_item() -> Fx {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    fx
}

/// Every queue tells the agent how to present what it writes: the guidance
/// names the presentation skill and where an agent without an install reads
/// it.
#[tokio::test]
async fn the_queue_guidance_points_to_the_presentation_skill() {
    let fx = curated_with_a_feedback_item().await;
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    let guidance = queue["guidance"].as_str().unwrap();
    assert!(
        guidance.contains("crystalline-presentation") && guidance.contains("skills tool"),
        "{guidance}"
    );
}

fn age_file(fx: &Fx, rel: &str, days: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(fx.root.join(rel))
        .unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
        .unwrap();
}

fn rows(queue: &serde_json::Value) -> Vec<(String, String, String)> {
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().into(),
                r["what"].as_str().unwrap().into(),
                r["heading"].as_str().unwrap_or("").into(),
            )
        })
        .collect()
}

fn edit_page(fx: &Fx, from: &str, to: &str) -> String {
    let path = fx.root.join("reflections/guides/deploying.md");
    std::fs::read_to_string(&path).unwrap().replace(from, to)
}

#[tokio::test]
async fn feedback_comes_before_update_within_a_section_and_the_oldest_group_first() {
    let fx = curated(&[]).await;
    // The engram behind Night deploys changed ten days ago.
    std::fs::write(
        fx.root.join("runbooks/deploy.md"),
        engram(
            "Deploy",
            "runbooks/deploy",
            "- [fact] deploys run in the evening",
        ),
    )
    .unwrap();
    age_file(&fx, "runbooks/deploy.md", 10);
    fx.engine.sync(None).await.unwrap();
    // A person changed both sections just now.
    let text = edit_page(
        &fx,
        "Deploys run at night.",
        "Deploys run at night, after the batch.",
    );
    let text = text.replace(
        "Roll back with the script.",
        "Roll back with the new script.",
    );
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();

    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(
        rows(&queue),
        [
            ("feedback".into(), "changed".into(), "Night deploys".into()),
            (
                "update".into(),
                "source_changed".into(),
                "Night deploys".into()
            ),
            ("feedback".into(), "changed".into(), "Rollback".into()),
        ],
        "{queue}"
    );
    assert!(
        queue["queue"][0]["diff"]
            .as_str()
            .unwrap()
            .contains("+Deploys run at night, after the batch.")
    );
    assert_eq!(queue["queue"][0]["changed_by"][0], "jordi");
    assert!(queue["queue"][1]["age_secs"].as_i64().unwrap() >= 9 * 86_400);
}

#[tokio::test]
async fn a_change_that_only_ticks_task_boxes_is_not_feedback() {
    let fx = curated(&[]).await;
    // The curator turns the rollback into a task list.
    let steps = "- [ ] stop the job\n- [ ] restore";
    let page = edit_page(&fx, "Roll back with the script.", steps);
    let mut curated_page = write("guides/deploying.md", &page);
    curated_page.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine
        .write_reflection(&curated_page, &agent())
        .await
        .unwrap();
    let settled = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        !rows(&settled).iter().any(|r| r.0 == "feedback"),
        "{settled}"
    );

    // A person ticks one box.
    let ticked = edit_page(&fx, "- [ ] stop the job", "- [x] stop the job");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &ticked), &person())
        .await
        .unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        !rows(&queue).iter().any(|r| r.0 == "feedback"),
        "a tick is not feedback: {queue}"
    );

    // A second write that also changes a word is.
    let worded = edit_page(&fx, "- [ ] restore", "- [x] restore the data");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &worded), &person())
        .await
        .unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        rows(&queue).contains(&("feedback".into(), "changed".into(), "Rollback".into())),
        "{queue}"
    );
}

/// The runbook shapes the skill asks for: a checklist inside a callout and
/// numbered steps with boxes. Ticking either is no more feedback than a tick
/// in a plain list.
#[tokio::test]
async fn ticking_a_box_in_a_callout_or_a_numbered_list_is_not_feedback() {
    let fx = curated(&[]).await;
    let steps = "> [!IMPORTANT]\n> - [ ] export the tenant\n\n1. [ ] stop the job\n2. [ ] restore";
    let page = edit_page(&fx, "Roll back with the script.", steps);
    let mut curated_page = write("guides/deploying.md", &page);
    curated_page.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine
        .write_reflection(&curated_page, &agent())
        .await
        .unwrap();

    let ticked = edit_page(
        &fx,
        "> - [ ] export the tenant",
        "> - [x] export the tenant",
    )
    .replace("2. [ ] restore", "2. [x] restore");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &ticked), &person())
        .await
        .unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        !rows(&queue).iter().any(|r| r.0 == "feedback"),
        "a tick in a callout or a numbered list is not feedback: {queue}"
    );
}

#[tokio::test]
async fn every_update_trigger() {
    let pact = "# Pact\n\n## Structure\n\n- guides/*.md\n";
    let fx = curated(&[(
        "notes/loose.md",
        &engram("Loose", "notes/loose", "- [fact] nobody reflected this"),
    )])
    .await;
    fx.engine
        .write_reflection(&write("PACT.md", pact), &agent())
        .await
        .unwrap();
    let pact_ids: Vec<String> = crystalline_core::reflection::split_sections(
        &std::fs::read_to_string(fx.root.join("reflections/PACT.md")).unwrap(),
    )
    .into_iter()
    .filter_map(|s| s.id)
    .collect();
    fx.engine
        .settle_reflection("eng", "PACT.md", &pact_ids, "kept", &agent())
        .await
        .unwrap();
    // Retire one source, change nothing about the other yet.
    std::fs::write(
        fx.root.join("runbooks/rollback.md"),
        engram(
            "Rollback",
            "runbooks/rollback",
            "- [fact] roll back with the script",
        )
        .replace("status: stable", "status: deprecated"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    // A page off the structure, and a change to the pact.
    fx.engine
        .write_reflection(&write("misc/stray.md", "# Stray\n"), &person())
        .await
        .unwrap();
    let edited_pact = std::fs::read_to_string(fx.root.join("reflections/PACT.md")).unwrap()
        + "\n## Audience\n\nNew people.\n";
    fx.engine
        .write_reflection(&write("PACT.md", &edited_pact), &person())
        .await
        .unwrap();

    let queue = fx
        .engine
        .reflect(
            &ReflectParams {
                kinds: vec!["update".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let whats: Vec<String> = rows(&queue).into_iter().map(|(_, w, _)| w).collect();
    for want in [
        "source_retired",
        "unreflected",
        "off_structure",
        "pact_changed",
    ] {
        assert!(whats.contains(&want.to_string()), "{want} missing: {queue}");
    }
    let unreflected = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["what"] == "unreflected")
        .unwrap();
    assert_eq!(unreflected["folder"], "notes");
    assert_eq!(unreflected["engrams"][0], "notes/loose");
    let pact_changed = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["what"] == "pact_changed")
        .unwrap();
    let diff = pact_changed["diff"].as_str().unwrap();
    assert!(diff.contains("+## Audience"), "{diff}");
    assert!(
        !diff.contains("-# Pact") && !diff.contains("-## Structure"),
        "only the added section is a hunk: {diff}"
    );
}

/// The home's summary counts what the review's queue counts, on a domain
/// that owes every kind of work at once: a person's edit (feedback), a
/// changed and a retired source, a page off the pact's structure, a pact
/// edit, an unreflected engram and an open thread. `updates` is the queue's
/// `kinds.update`, and `pages_to_update` the distinct pages of those rows.
#[tokio::test]
async fn the_summary_counts_the_updates_the_review_queues() {
    let pact = "# Pact\n\n## Structure\n\n- guides/*.md\n";
    let fx = curated(&[(
        "notes/loose.md",
        &engram("Loose", "notes/loose", "- [fact] nobody reflected this"),
    )])
    .await;
    fx.engine
        .write_reflection(&write("PACT.md", pact), &agent())
        .await
        .unwrap();
    let pact_ids: Vec<String> = crystalline_core::reflection::split_sections(
        &std::fs::read_to_string(fx.root.join("reflections/PACT.md")).unwrap(),
    )
    .into_iter()
    .filter_map(|s| s.id)
    .collect();
    fx.engine
        .settle_reflection("eng", "PACT.md", &pact_ids, "kept", &agent())
        .await
        .unwrap();
    // One source changes, the other retires.
    std::fs::write(
        fx.root.join("runbooks/deploy.md"),
        engram("Deploy", "runbooks/deploy", "- [fact] deploys run at dawn"),
    )
    .unwrap();
    std::fs::write(
        fx.root.join("runbooks/rollback.md"),
        engram(
            "Rollback",
            "runbooks/rollback",
            "- [fact] roll back with the script",
        )
        .replace("status: stable", "status: deprecated"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    // A person's edit of the page, a page off the structure, a pact edit.
    let page = std::fs::read_to_string(fx.root.join("reflections/guides/deploying.md"))
        .unwrap()
        .replace(
            "Deploys run at night.",
            "Deploys run at night, after the batch.",
        );
    fx.engine
        .write_reflection(&write("guides/deploying.md", &page), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("misc/stray.md", "# Stray\n"), &person())
        .await
        .unwrap();
    let edited_pact = std::fs::read_to_string(fx.root.join("reflections/PACT.md")).unwrap()
        + "\n## Audience\n\nNew people.\n";
    fx.engine
        .write_reflection(&write("PACT.md", &edited_pact), &person())
        .await
        .unwrap();
    fx.engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                path: Some("guides/deploying.md".into()),
                section: Some("Rollback".into()),
                text: Some("Which script?".into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();

    for caller in [agent(), person()] {
        let review = fx.engine.reflection_review("eng", &caller).await.unwrap();
        let summary = fx.engine.reflection_summary("eng", &caller).await.unwrap();
        let rows = review["queue"]["queue"].as_array().unwrap();
        let whats: Vec<&str> = rows.iter().filter_map(|r| r["what"].as_str()).collect();
        for want in [
            "changed",
            "source_changed",
            "source_retired",
            "off_structure",
            "pact_changed",
            "unreflected",
            "thread",
        ] {
            assert!(whats.contains(&want), "{want} missing: {review}");
        }
        assert_eq!(
            rows.len() as u64,
            review["queue"]["total"].as_u64().unwrap(),
            "one page holds the whole queue"
        );
        let mut pages: Vec<&str> = rows
            .iter()
            .filter(|r| r["kind"] == "update")
            .filter_map(|r| r["path"].as_str())
            .filter(|p| !p.is_empty())
            .collect();
        pages.sort_unstable();
        pages.dedup();
        assert_eq!(
            pages,
            ["PACT.md", "guides/deploying.md", "misc/stray.md"],
            "{review}"
        );
        assert_eq!(
            summary["updates"], review["queue"]["kinds"]["update"],
            "{summary}"
        );
        assert_eq!(summary["pages_to_update"], pages.len(), "{summary}");
        assert_eq!(summary["waiting_for_the_agent"], 1, "{summary}");
    }
}

#[tokio::test]
async fn a_deleted_source_is_an_update_item_not_an_error() {
    let fx = curated(&[]).await;
    std::fs::remove_file(fx.root.join("runbooks/deploy.md")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    let missing = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["what"] == "source_missing")
        .expect("reported");
    assert_eq!(missing["heading"], "Night deploys");
    assert_eq!(missing["sources"][0]["state"], "missing");
    let doc = fx
        .engine
        .read_reflection("eng", "guides/deploying.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        doc["sections"][0]["sources"][0]["state"], "missing",
        "the read does not fail either"
    );
}

#[tokio::test]
async fn a_section_added_in_a_plain_editor_is_a_feedback_item_addressed_by_heading() {
    let fx = curated(&[]).await;
    let path = fx.root.join("reflections/guides/deploying.md");
    let text = std::fs::read_to_string(&path).unwrap()
        + "\n## Typed by hand\n\nA person wrote this outside Crystalline.\n";
    std::fs::write(&path, text).unwrap();
    fx.engine.sync(None).await.unwrap();
    let queue = fx
        .engine
        .reflect(
            &ReflectParams {
                kinds: vec!["feedback".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let item = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["heading"] == "Typed by hand")
        .expect("reported");
    assert_eq!(item["what"], "new");
    assert!(item["section"].is_null(), "no id yet: addressed by heading");
    assert_eq!(
        item["changed_by"].as_array().unwrap().len(),
        0,
        "written outside Crystalline"
    );
}

/// A thread a person opens on a heading with no id anchors on the heading;
/// its review item joins the feedback item of that section, both addressed
/// by heading.
#[tokio::test]
async fn a_thread_on_a_heading_without_an_id_joins_that_sections_group() {
    let fx = curated(&[]).await;
    let path = fx.root.join("reflections/guides/deploying.md");
    let text = std::fs::read_to_string(&path).unwrap()
        + "\n## Typed by hand\n\nA person wrote this outside Crystalline.\n";
    std::fs::write(&path, text).unwrap();
    fx.engine.sync(None).await.unwrap();
    fx.engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                path: Some("guides/deploying.md".into()),
                section: Some("Typed by hand".into()),
                text: Some("Who wrote this?".into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(
        rows(&queue),
        [
            ("feedback".into(), "new".into(), "Typed by hand".into()),
            ("review".into(), "thread".into(), "Typed by hand".into()),
        ],
        "{queue}"
    );
    assert!(queue["queue"][1]["section"].is_null(), "{queue}");
    let counts = fx.engine.reflect_open_counts(&[], &agent()).await.unwrap();
    assert_eq!(counts, vec![("eng".to_string(), 2)]);
    let review = fx.engine.reflection_review("eng", &agent()).await.unwrap();
    let waiting = &review["waiting_for_the_agent"][0]["threads"][0];
    assert_eq!(waiting["anchor"]["section"], "Typed by hand", "{review}");
    assert_eq!(waiting["anchor"]["state"], "found", "{review}");
}

#[tokio::test]
async fn an_answered_feedback_item_waits_while_its_thread_is_open() {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let id = crystalline_core::reflection::split_sections(&text)[1]
        .id
        .clone()
        .unwrap();
    let before = fx
        .engine
        .reflect(
            &ReflectParams {
                kinds: vec!["feedback".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(before["total"], 1);
    let opened = fx
        .engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                path: Some("guides/deploying.md".into()),
                section: Some(id),
                text: Some("Dawn? The batch still runs at night.".into()),
                answers_feedback: true,
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let after = fx
        .engine
        .reflect(
            &ReflectParams {
                kinds: vec!["feedback".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(after["total"], 0, "answered, waiting on people");
    let review = fx.engine.reflection_review("eng", &agent()).await.unwrap();
    assert_eq!(
        review["waiting_for_people"][0]["threads"][0]["id"],
        opened["id"]
    );
    assert!(
        review["waiting_for_the_agent"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // The agent opened this thread, so the agent may close it, and the
    // change it answered comes back to the queue.
    fx.engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some("resolve".into()),
                path: Some("guides/deploying.md".into()),
                thread: Some(opened["id"].as_str().unwrap().into()),
                text: Some("Kept at night.".into()),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let resolved = fx
        .engine
        .reflect(
            &ReflectParams {
                kinds: vec!["feedback".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(resolved["total"], 1, "{resolved}");
}

#[tokio::test]
async fn the_queue_strips_engram_names_for_a_caller_who_cannot_read_engrams() {
    let (fx, ann) = crate::rights::contributor_engine().await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut p = write("a.md", "# A\n\n## Night\n\nDeploys run at night.\n");
    p.sources = BTreeMap::from([("Night".to_string(), vec!["runbooks/deploy".to_string()])]);
    fx.engine
        .write_reflection(&p, &ReflectionCaller::owner())
        .await
        .unwrap();
    std::fs::write(
        fx.root.join("runbooks/deploy.md"),
        engram("Deploy", "runbooks/deploy", "- [fact] evening"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let queue = fx
        .engine
        .reflect_queue("eng", None, None, &ReflectionCaller::person(ann, "ann"))
        .await
        .unwrap();
    let text = queue.to_string();
    assert!(queue["total"].as_u64().unwrap() >= 1);
    assert!(
        !text.contains("runbooks/deploy"),
        "no engram name reaches a contributor: {text}"
    );
}

#[tokio::test]
async fn open_counts_name_only_the_domains_the_caller_curates() {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let counts = fx.engine.reflect_open_counts(&[], &agent()).await.unwrap();
    assert_eq!(counts, vec![("eng".to_string(), 1)]);
    // A caller who may read the Reflection but not curate it gets no count.
    let (fx, ann) = crate::rights::contributor_engine().await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    fx.engine
        .write_reflection(
            &write("a.md", "# A\n\n## Night\n\nDeploys run at night.\n"),
            &ReflectionCaller::owner(),
        )
        .await
        .unwrap();
    let reader = ReflectionCaller::person(ann, "ann");
    assert!(
        fx.engine
            .reflect_queue("eng", None, None, &reader)
            .await
            .unwrap()["total"]
            .as_u64()
            .unwrap()
            >= 1,
        "the reader sees the queue"
    );
    assert!(
        fx.engine
            .reflect_open_counts(&[], &reader)
            .await
            .unwrap()
            .is_empty(),
        "and is not nudged"
    );
}

#[tokio::test]
async fn an_unnamed_queue_leaves_a_read_only_domain_out() {
    let fx = crate::docs::reflecting().await;
    std::fs::write(
        fx.root.join("MANIFEST.md"),
        crate::fixture::manifest("reflection: on\nread_only: on\n"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let unnamed = fx
        .engine
        .reflect(&ReflectParams::default(), &crate::docs::agent())
        .await
        .unwrap();
    assert_eq!(
        unnamed["scope"]["domains"],
        serde_json::json!([]),
        "{unnamed}"
    );
    let named = ReflectParams {
        domains: vec!["eng".into()],
        ..Default::default()
    };
    let named = fx
        .engine
        .reflect(&named, &crate::docs::agent())
        .await
        .unwrap();
    assert_eq!(
        named["scope"]["domains"],
        serde_json::json!(["eng"]),
        "a named domain still answers"
    );
}

/// While `reflection: on` waits for `reflections/` to empty, the write
/// receipt's close-folder hint never names that folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_close_folder_hint_leaves_the_reflections_folder_alone() {
    let fx = fixture(&[
        (
            "reflections/kept.md",
            &engram("Kept", "reflections/kept", "- [fact] still an engram"),
        ),
        (
            "reflections/also.md",
            &engram("Also", "reflections/also", "- [fact] another engram"),
        ),
    ])
    .await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert_eq!(
        fx.engine.reflection_state("eng").await.unwrap().word(),
        "pending"
    );
    let agent = crystalline_remote::ForwardedAgent::default();
    let receipt = crystalline_service::route::run_tool_routed(
        &fx.engine,
        "write_engram",
        serde_json::json!({
            "domain": "eng",
            "title": "Stray",
            "folder": "reflection",
            "content": "A fact about the topic.",
        }),
        &agent,
        None,
    )
    .await
    .unwrap();
    assert_eq!(receipt["action"], "created", "{receipt}");
    assert!(
        !receipt.to_string().contains("\"folder\":\"reflections\""),
        "{receipt}"
    );
    assert!(receipt.get("close_folders").is_none(), "{receipt}");
}

/// Evolve fills `reflection_pending` from the MANIFEST, so V112 stops pairing
/// `reflections/` with its close sibling once the switch is on and pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn evolve_leaves_the_pending_reflections_folder_out_of_v112() {
    let fx = fixture(&[
        (
            "reflections/kept.md",
            &engram("Kept", "reflections/kept", "- [fact] still an engram"),
        ),
        (
            "reflection/stray.md",
            &engram("Stray", "reflection/stray", "- [fact] close by"),
        ),
        (
            "reflection/other.md",
            &engram("Other", "reflection/other", "- [fact] close by too"),
        ),
    ])
    .await;
    let v112 = || crystalline_service::params::EvolveParams {
        domains: vec!["eng".to_string()],
        rules: vec!["V112".to_string()],
        ..Default::default()
    };
    let before = fx
        .engine
        .evolve_detect(&v112(), &crystalline_service::Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        before["queue"].as_array().unwrap().len(),
        1,
        "an ordinary pair while the switch is off: {before}"
    );
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let after = fx
        .engine
        .evolve_detect(&v112(), &crystalline_service::Scope::Unrestricted)
        .await
        .unwrap();
    assert!(after["queue"].as_array().unwrap().is_empty(), "{after}");
}

/// A domain whose queue cannot be read is left out of an unnamed queue and
/// of the counts, and named in the answer; named, it is the caller's error.
#[tokio::test]
async fn an_unreadable_domain_never_fails_the_unnamed_queue_or_the_counts() {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    // A threads "file" that is a folder: reading it fails.
    std::fs::create_dir_all(fx.root.join("reflections/guides/deploying.threads.md")).unwrap();
    let unnamed = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(unnamed["total"], 0, "{unnamed}");
    assert_eq!(
        unnamed["scope"]["domains"],
        serde_json::json!([]),
        "{unnamed}"
    );
    assert_eq!(
        unnamed["scope"]["unreadable"],
        serde_json::json!(["eng"]),
        "{unnamed}"
    );
    let named = ReflectParams {
        domains: vec!["eng".into()],
        ..Default::default()
    };
    assert!(fx.engine.reflect(&named, &agent()).await.is_err());
    assert!(
        fx.engine
            .reflect_open_counts(&[], &agent())
            .await
            .unwrap()
            .is_empty()
    );
}

/// `skip_unavailable` screens named domains as the unnamed queue screens
/// every domain: an unreadable one is named, an unregistered name is left
/// out, and neither fails the queue.
#[tokio::test]
async fn a_lenient_named_queue_names_an_unreadable_domain_and_skips_an_unknown_one() {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    std::fs::create_dir_all(fx.root.join("reflections/guides/deploying.threads.md")).unwrap();
    let lenient = ReflectParams {
        domains: vec!["eng".into(), "nowhere".into()],
        skip_unavailable: true,
        ..Default::default()
    };
    let queue = fx.engine.reflect(&lenient, &agent()).await.unwrap();
    assert_eq!(queue["total"], 0, "{queue}");
    assert_eq!(
        queue["scope"]["unreadable"],
        serde_json::json!(["eng"]),
        "{queue}"
    );
}

/// A whole queue carries no `unreadable` member.
#[tokio::test]
async fn a_readable_scope_names_nothing_unreadable() {
    let fx = curated(&[]).await;
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(queue["scope"].get("unreadable").is_none(), "{queue}");
}

#[tokio::test]
async fn a_domain_named_twice_is_swept_once() {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let twice = ReflectParams {
        domains: vec!["eng".into(), "eng".into()],
        ..Default::default()
    };
    let queue = fx.engine.reflect(&twice, &agent()).await.unwrap();
    assert_eq!(queue["total"], 1, "{queue}");
    assert_eq!(
        queue["scope"]["domains"],
        serde_json::json!(["eng"]),
        "{queue}"
    );
    let counts = fx
        .engine
        .reflect_open_counts(&["eng".to_string(), "eng".to_string()], &agent())
        .await
        .unwrap();
    assert_eq!(counts, vec![("eng".to_string(), 1)]);
}

/// The write list caps the Reflection at read (R18): an editor off the list
/// sees the queue and is not nudged; one with no list is.
#[tokio::test]
async fn an_off_list_curator_gets_no_count() {
    use std::sync::Arc;

    use crystalline_service::rest::{AuthStore, Role};
    use crystalline_service::{DomainAccess, Scope};

    let fx = curated(&[]).await;
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["ed", "lee"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_write_list("ed", &["agents-*".to_string()])
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let as_user = |name: &str| {
        ReflectionCaller::agent(
            Scope::User {
                account: name.into(),
                admin: false,
            },
            name,
            false,
        )
    };
    let ed = as_user("ed");
    assert_eq!(
        fx.engine
            .reflect_queue("eng", None, None, &ed)
            .await
            .unwrap()["total"],
        1,
        "the off-list editor still reads the queue"
    );
    assert!(
        fx.engine
            .reflect_open_counts(&[], &ed)
            .await
            .unwrap()
            .is_empty(),
        "and is not nudged"
    );
    assert_eq!(
        fx.engine
            .reflect_open_counts(&[], &as_user("lee"))
            .await
            .unwrap(),
        vec![("eng".to_string(), 1)],
        "an editor with no list curates and is nudged"
    );
}

/// A domain whose Reflection is pending is named in an unnamed queue's
/// `scope.pending` (and left out of the queue); named, it still refuses.
#[tokio::test]
async fn a_pending_reflection_is_named_in_the_unnamed_queue() {
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "reflections/old.md",
            &engram("Old", "reflections/old", "- [fact] still an engram"),
        ),
    ])
    .await;
    let unnamed = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(
        unnamed["scope"]["pending"],
        serde_json::json!(["eng"]),
        "{unnamed}"
    );
    assert_eq!(
        unnamed["scope"]["domains"],
        serde_json::json!([]),
        "{unnamed}"
    );
    assert!(unnamed["scope"].get("unreadable").is_none(), "{unnamed}");
    let named = ReflectParams {
        domains: vec!["eng".into()],
        ..Default::default()
    };
    let err = fx
        .engine
        .reflect(&named, &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("reflections/"), "{err}");
}

/// A queue with nothing pending carries no `pending` member.
#[tokio::test]
async fn a_queue_with_nothing_pending_names_nothing_pending() {
    let fx = curated(&[]).await;
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(queue["scope"].get("pending").is_none(), "{queue}");
}

/// The cost of the Reflection home's summary against the full review on a
/// large domain: 5,000 engrams, 200 pages of four sections each citing two
/// engrams, 400 cited engrams changed since, 100 pages edited by a person
/// and 400 open threads. Never a gate: the numbers depend on the machine and
/// go into the task report. Run with
/// `cargo nextest run -p crystalline-service --test reflection --run-ignored only reflection_summary_timing --no-capture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "timing evidence for the task report, not a gate"]
async fn reflection_summary_timing() {
    use std::time::Instant;

    const ENGRAMS: usize = 5_000;
    const PAGES: usize = 200;
    const SECTIONS: usize = 4;
    const RUNS: usize = 15;

    let note = |i: usize, body: &str| engram(&format!("Note {i}"), &format!("note-{i}"), body);
    let mut files: Vec<(String, String)> =
        vec![("MANIFEST.md".into(), manifest("reflection: on\n"))];
    for i in 0..ENGRAMS {
        files.push((
            format!("notes/d{}/n{i}.md", i % 50),
            note(i, "- [fact] a note"),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let built = Instant::now();
    let fx = fixture(&refs).await;

    let section =
        |p: usize, s: usize, text: &str| format!("## Part {s}\n\n{text} of page {p}.\n\n");
    for p in 0..PAGES {
        let mut markdown = format!("# Page {p}\n\nFor people.\n\n");
        let mut sources = BTreeMap::from([("_top".to_string(), Vec::new())]);
        for s in 0..SECTIONS {
            markdown.push_str(&section(p, s, "What the agent wrote"));
            let first = (p * SECTIONS + s) * 2 % ENGRAMS;
            sources.insert(
                format!("Part {s}"),
                vec![format!("note-{first}"), format!("note-{}", first + 1)],
            );
        }
        let mut params = write(&format!("area{}/page{p}.md", p % 10), &markdown);
        params.sources = sources;
        fx.engine.write_reflection(&params, &agent()).await.unwrap();
    }
    // 400 cited engrams change after they were reflected.
    for i in 0..400 {
        std::fs::write(
            fx.root.join(format!("notes/d{}/n{i}.md", i % 50)),
            note(i, "- [fact] a changed note"),
        )
        .unwrap();
    }
    fx.engine.sync(None).await.unwrap();
    // 100 pages edited by a person: every section differs from its baseline.
    for p in 0..100 {
        let path = format!("area{}/page{p}.md", p % 10);
        let doc = fx
            .engine
            .read_reflection("eng", &path, &person())
            .await
            .unwrap();
        let edited = doc["markdown"]
            .as_str()
            .unwrap()
            .replace("What the agent wrote", "What a person rewrote");
        fx.engine
            .write_reflection(&write(&path, &edited), &person())
            .await
            .unwrap();
    }
    // 400 open threads, two on each of the first 200 pages' first section.
    for t in 0..400 {
        let p = t % PAGES;
        fx.engine
            .discuss(
                &DiscussParams {
                    domain: "eng".into(),
                    path: Some(format!("area{}/page{p}.md", p % 10)),
                    section: Some("Part 0".into()),
                    text: Some(format!("Question {t}?")),
                    ..Default::default()
                },
                &person(),
            )
            .await
            .unwrap();
    }
    eprintln!("seeded in {:?}", built.elapsed());

    let caller = person();
    let review = fx.engine.reflection_review("eng", &caller).await.unwrap();
    let summary = fx.engine.reflection_summary("eng", &caller).await.unwrap();
    assert_eq!(summary["updates"], review["queue"]["kinds"]["update"]);
    assert_eq!(summary["waiting_for_the_agent"], 400);
    eprintln!(
        "queue: {} (update {} on {} pages, feedback {}); payload bytes: review {}, summary {}",
        review["queue"]["total"],
        review["queue"]["kinds"]["update"],
        summary["pages_to_update"],
        review["queue"]["kinds"]["feedback"],
        review.to_string().len(),
        summary.to_string().len(),
    );

    let median = |mut times: Vec<std::time::Duration>| {
        times.sort();
        times[times.len() / 2]
    };
    let mut review_times = Vec::new();
    let mut summary_times = Vec::new();
    for _ in 0..RUNS {
        let started = Instant::now();
        let v = fx.engine.reflection_review("eng", &caller).await.unwrap();
        let _ = v.to_string();
        review_times.push(started.elapsed());
        let started = Instant::now();
        let v = fx.engine.reflection_summary("eng", &caller).await.unwrap();
        let _ = v.to_string();
        summary_times.push(started.elapsed());
    }
    eprintln!(
        "median of {RUNS}: review {:?}, summary {:?}",
        median(review_times),
        median(summary_times)
    );
}

/// The cost of a queue pass over pages whose sources are in another
/// domain, beside the same pages with local sources: 2,000 engrams in each
/// domain, 100 pages of four sections each citing two engrams. Never a
/// gate: the numbers go into the task report. Run with
/// `cargo nextest run -p crystalline-service --test reflection --run-ignored only foreign_sources_timing --no-capture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "timing evidence for the task report, not a gate"]
async fn foreign_sources_timing() {
    use std::time::Instant;

    const ENGRAMS: usize = 2_000;
    const PAGES: usize = 100;
    const SECTIONS: usize = 4;
    const RUNS: usize = 15;

    let notes: Vec<(String, String)> = (0..ENGRAMS)
        .map(|i| {
            (
                format!("notes/d{}/n{i}.md", i % 20),
                engram(
                    &format!("Note {i}"),
                    &format!("note-{i}"),
                    "- [fact] a note",
                ),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = notes
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let fx = crate::fixture::beside(&refs, "harbor", &refs).await;
    let median = |mut times: Vec<std::time::Duration>| {
        times.sort();
        times[times.len() / 2]
    };
    let mut results = Vec::new();
    for (label, prefix) in [("local", ""), ("foreign", "crystalline://harbor/")] {
        for p in 0..PAGES {
            let mut markdown = format!("# Page {p}\n\nFor people.\n\n");
            let mut sources = BTreeMap::new();
            for s in 0..SECTIONS {
                markdown.push_str(&format!("## Part {s}\n\nText {s}.\n\n"));
                let first = (p * SECTIONS + s) * 2 % ENGRAMS;
                sources.insert(
                    format!("Part {s}"),
                    vec![
                        format!("{prefix}note-{first}"),
                        format!("{prefix}note-{}", first + 1),
                    ],
                );
            }
            let mut params = write(&format!("page{p}.md"), &markdown);
            params.sources = sources;
            fx.engine.write_reflection(&params, &agent()).await.unwrap();
        }
        let mut times = Vec::new();
        for _ in 0..RUNS {
            let started = Instant::now();
            let v = fx.engine.reflect(&eng_only(), &agent()).await.unwrap();
            let _ = v.to_string();
            times.push(started.elapsed());
        }
        results.push((label, median(times)));
    }
    eprintln!("median of {RUNS} reflect passes: {results:?}");
}

/// An agent's own curated write owes nobody feedback: the intro before the
/// first `##`, which no source names, and a section it wrote without
/// sources move to what it wrote. A person's later change of the intro is
/// feedback again.
#[tokio::test]
async fn an_agents_curated_write_owes_no_feedback_on_its_own_intro() {
    let fx = crate::docs::reflecting().await;
    let mut p = write(
        "guides/deploying.md",
        "# Deploying\n\nFor new people.\n\n## Night deploys\n\nDeploys run at night.\n\n## Questions\n\nAsk in the channel.\n",
    );
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(queue["total"], 0, "the agent's own text: {queue}");

    let text = edit_page(&fx, "For new people.", "For new people and visitors.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(
        rows(&queue),
        [("feedback".into(), "changed".into(), String::new())],
        "{queue}"
    );
    assert_eq!(queue["queue"][0]["section"], "_top", "{queue}");
    assert_eq!(queue["queue"][0]["changed_by"][0], "jordi", "{queue}");
}

/// A curated write moves only what it wrote: a person's change to a section
/// the agent sent back untouched stays a feedback item.
#[tokio::test]
async fn a_curated_write_keeps_a_persons_change_it_did_not_touch() {
    let fx = curated_with_a_feedback_item().await;
    let text = edit_page(
        &fx,
        "Roll back with the script.",
        "Roll back with the old script.",
    );
    let text = text.replace("# Deploying\n", "# Deploying\n\nFor new people.\n");
    let mut p = write("guides/deploying.md", &text);
    p.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(
        rows(&queue),
        [("feedback".into(), "changed".into(), "Night deploys".into())],
        "{queue}"
    );
}

/// A person's change an agent's curated write rewrites without naming its
/// sources is not settled by it: the feedback item stays until the agent
/// takes it in, keeps it or answers it.
#[tokio::test]
async fn a_curated_write_that_rewrites_a_persons_change_keeps_it_as_feedback() {
    let fx = curated_with_a_feedback_item().await;
    let text = edit_page(
        &fx,
        "Deploys run at dawn.",
        "Deploys run at dawn, after the batch.",
    )
    .replace(
        "Roll back with the script.",
        "Roll back with the old script.",
    );
    let mut p = write("guides/deploying.md", &text);
    p.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(
        rows(&queue),
        [("feedback".into(), "changed".into(), "Night deploys".into())],
        "{queue}"
    );
    assert_eq!(queue["queue"][0]["changed_by"][0], "jordi", "{queue}");
}

/// A stray id comment the write takes out of a person's changed section
/// changes its text, and still does not settle the person's change.
#[tokio::test]
async fn a_stray_id_taken_out_of_a_persons_change_keeps_it_as_feedback() {
    let fx = curated_with_a_feedback_item().await;
    let path = fx.root.join("reflections/guides/deploying.md");
    let page = std::fs::read_to_string(&path).unwrap().replace(
        "\nDeploys run at dawn.",
        "\n<!-- r:a1b2c3d4 -->\n\nDeploys run at dawn.",
    );
    std::fs::write(&path, &page).unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut p = write("guides/deploying.md", &page);
    p.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("a1b2c3d4"), "{text}");
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(
        rows(&queue),
        [("feedback".into(), "changed".into(), "Night deploys".into())],
        "{queue}"
    );
}

/// The agent curates Rollback and sends the rest of the page as `edit`
/// makes it; the queue's feedback rows after that write, by heading.
async fn curate_rollback_and_resend(fx: &Fx, edit: impl Fn(String) -> String) -> Vec<String> {
    let path = fx.root.join("reflections/guides/deploying.md");
    let text = edit(std::fs::read_to_string(&path).unwrap()).replace(
        "Roll back with the script.",
        "Roll back with the old script.",
    );
    let mut p = write("guides/deploying.md", &text);
    p.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let queue = fx
        .engine
        .reflect(
            &ReflectParams {
                kinds: vec!["feedback".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["heading"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A section a person typed in a plain editor (no id line yet) stays a
/// feedback item through an agent's curated write, sent back untouched or
/// reworded: the write gives it an id and nothing else.
#[tokio::test]
async fn a_hand_typed_section_stays_feedback_through_a_curated_write() {
    for reword in [false, true] {
        let fx = curated(&[]).await;
        let path = fx.root.join("reflections/guides/deploying.md");
        let text = std::fs::read_to_string(&path).unwrap()
            + "\n## Typed by hand\n\nA person wrote this outside Crystalline.\n";
        std::fs::write(&path, text).unwrap();
        fx.engine.sync(None).await.unwrap();
        let owed = curate_rollback_and_resend(&fx, |t| {
            if reword {
                t.replace("A person wrote this", "Someone wrote this")
            } else {
                t
            }
        })
        .await;
        assert_eq!(owed, ["Typed by hand"], "reword {reword}");
    }
}

/// A person's on-disk change of a section the agent wrote (and so
/// reflected) stays feedback through the agent's next curated write that
/// rewords it: the change was owed before that write, whoever the record
/// names, since sync does not touch the record.
#[tokio::test]
async fn a_persons_disk_edit_of_the_agents_unsettled_section_stays_feedback() {
    let fx = curated(&[]).await;
    let text = edit_page(
        &fx,
        "Roll back with the script.\n",
        "Roll back with the script.\n\n## Notes\n\nAsk first.\n",
    );
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &agent())
        .await
        .unwrap();
    let path = fx.root.join("reflections/guides/deploying.md");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Ask first.", "Ask the on-call first.");
    std::fs::write(&path, edited).unwrap();
    fx.engine.sync(None).await.unwrap();
    let owed = curate_rollback_and_resend(&fx, |t| {
        t.replace("the on-call first.", "the on-call engineer first.")
    })
    .await;
    assert_eq!(owed, ["Notes"]);
}

/// The agent's own unsourced section is its own text: it owes no feedback
/// after the write that adds it nor after the next write that rewords it.
#[tokio::test]
async fn the_agents_own_unsourced_section_never_comes_back_as_feedback() {
    let fx = curated(&[]).await;
    let text = edit_page(
        &fx,
        "Roll back with the script.\n",
        "Roll back with the script.\n\n## Notes\n\nAsk first.\n",
    );
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &agent())
        .await
        .unwrap();
    assert!(feedback_headings(&fx).await.is_empty());
    let owed =
        curate_rollback_and_resend(&fx, |t| t.replace("Ask first.", "Ask before you start.")).await;
    assert!(owed.is_empty(), "{owed:?}");
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert_eq!(queue["total"], 0, "{queue}");
}

/// A person's pending change of the top stays feedback when the agent's
/// curated write rewrites the top.
#[tokio::test]
async fn a_persons_top_change_stays_feedback_through_an_agents_rewrite() {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "# Deploying\n", "# Deploying\n\nFor new people.\n");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let owed = curate_rollback_and_resend(&fx, |t| {
        t.replace("For new people.", "For new people and visitors.")
    })
    .await;
    assert_eq!(owed, [""], "the top, addressed as _top");
}

/// A person removed a section; the agent then sends a stale copy that still
/// holds it (reflowed), without a checksum. The section was reflected
/// before, so it is not the agent's fresh text: it stays in the queue.
#[tokio::test]
async fn a_stale_copy_of_a_section_a_person_removed_is_not_settled() {
    let fx = curated(&[]).await;
    let path = fx.root.join("reflections/guides/deploying.md");
    let stale = std::fs::read_to_string(&path).unwrap();
    let rollback = stale.find("## Rollback").unwrap();
    fx.engine
        .write_reflection(&write("guides/deploying.md", &stale[..rollback]), &person())
        .await
        .unwrap();
    let mut p = write(
        "guides/deploying.md",
        &stale
            .replace("Deploys run at night.", "Deploys run at night, late.")
            .replace(
                "Roll back with the script.",
                "Roll back with the script, then check.",
            ),
    );
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        rows(&queue)
            .iter()
            .any(|(kind, _, heading)| kind == "feedback" && heading == "Rollback"),
        "{queue}"
    );
}

/// The page file is gone but its baseline survives; the agent re-creates the
/// page with the old ids. A section the baseline holds is not fresh text, so
/// a change of it against the baseline stays in the queue.
#[tokio::test]
async fn a_recreated_page_does_not_settle_what_its_baseline_holds() {
    let fx = curated(&[]).await;
    let path = fx.root.join("reflections/guides/deploying.md");
    let stale = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert!(
        fx.root
            .join("reflections/.reflected/guides/deploying.md")
            .exists()
    );
    let mut p = write(
        "guides/deploying.md",
        &stale
            .replace("Deploys run at night.", "Deploys run at night, late.")
            .replace(
                "Roll back with the script.",
                "Roll back with the script, then check.",
            ),
    );
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        rows(&queue)
            .iter()
            .any(|row| row == &("feedback".into(), "changed".into(), "Rollback".into())),
        "{queue}"
    );
}

/// The feedback rows of the queue, by heading.
async fn feedback_headings(fx: &Fx) -> Vec<String> {
    let queue = fx
        .engine
        .reflect(
            &ReflectParams {
                kinds: vec!["feedback".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["heading"].as_str().unwrap_or("").to_string())
        .collect()
}

/// On a page that already has a baseline, a brand-new section the agent's
/// curated write adds without sources is its own text: it owes no feedback.
#[tokio::test]
async fn a_curated_write_settles_a_new_unsourced_section_on_a_page_with_a_baseline() {
    let fx = curated(&[]).await;
    let text = edit_page(
        &fx,
        "Roll back with the script.\n",
        "Roll back with the old script.\n\n## Questions\n\nAsk in the channel.\n",
    );
    let mut p = write("guides/deploying.md", &text);
    p.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert!(feedback_headings(&fx).await.is_empty());
}

/// A person's unreflected section that the agent sends back with its
/// heading in another case and without its id is held the way every heading
/// is matched (Unicode case folding), so it is not settled silently.
#[tokio::test]
async fn a_persons_section_sent_back_in_another_case_stays_feedback() {
    let fx = curated(&[]).await;
    let text = edit_page(
        &fx,
        "Roll back with the script.\n",
        "Roll back with the script.\n\n## Über uns\n\nWe are the ops team.\n",
    );
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let owed = curate_rollback_and_resend(&fx, |t| {
        let at = t.find("## Über uns").unwrap();
        let end = t[at..].find("\n\n").unwrap() + at;
        format!("{}## über uns{}", &t[..at], &t[end..])
    })
    .await;
    assert_eq!(owed, ["über uns"]);
}

/// A section a person added and then removed again in a plain editor,
/// before any reflection, keeps its record in the page's bookkeeping (the
/// editor left the block alone). A stale copy that brings it back with its id
/// is not the agent's fresh text: it stays feedback.
#[tokio::test]
async fn a_stale_copy_of_an_unreflected_section_with_a_record_stays_feedback() {
    let fx = curated(&[]).await;
    let added = edit_page(
        &fx,
        "Roll back with the script.\n",
        "Roll back with the script.\n\n## Draft\n\nA person's draft.\n",
    );
    fx.engine
        .write_reflection(&write("guides/deploying.md", &added), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/guides/deploying.md");
    let stale = std::fs::read_to_string(&path).unwrap();
    let draft = stale.find("\n## Draft").unwrap();
    std::fs::write(&path, format!("{}\n", &stale[..draft])).unwrap();
    fx.engine.sync(None).await.unwrap();
    let owed = curate_rollback_and_resend(&fx, |_| stale.clone()).await;
    assert_eq!(owed, ["Draft"]);
}

/// Sources alone curate the sections as they stand: the page is not
/// rewritten, the named section's feedback is settled and the rest stays.
#[tokio::test]
async fn sources_alone_curate_the_page_as_it_stands() {
    let fx = curated_with_a_feedback_item().await;
    let path = fx.root.join("reflections/guides/deploying.md");
    let before = std::fs::read_to_string(&path).unwrap();
    let mut p = write("guides/deploying.md", "");
    p.markdown = None;
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    let receipt = fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert_eq!(receipt["curated"], true, "{receipt}");
    assert!(feedback_headings(&fx).await.is_empty());
    let after = std::fs::read_to_string(&path).unwrap();
    let body = |t: &str| t[t.find("# Deploying").unwrap()..].to_string();
    assert_eq!(
        body(&after),
        body(&before),
        "the page text is not rewritten"
    );

    let mut missing = write("guides/nowhere.md", "");
    missing.markdown = None;
    missing.sources = p.sources.clone();
    let err = fx
        .engine
        .write_reflection(&missing, &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("write it with markdown first"), "{err}");
}

/// A write that leaves out a section a person changed since the last
/// reflection says so on its receipt, naming the section.
#[tokio::test]
async fn a_write_that_drops_a_persons_unreflected_section_names_it() {
    let fx = curated(&[]).await;
    let added = edit_page(
        &fx,
        "Roll back with the script.\n",
        "Roll back with the script.\n\n## Notes\n\nAsk first.\n",
    );
    fx.engine
        .write_reflection(&write("guides/deploying.md", &added), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/guides/deploying.md");
    let page = std::fs::read_to_string(&path).unwrap();
    let notes = page.find("\n## Notes").unwrap();
    let receipt = fx
        .engine
        .write_reflection(
            &write("guides/deploying.md", &format!("{}\n", &page[..notes])),
            &agent(),
        )
        .await
        .unwrap();
    let guidance = receipt["guidance"].as_str().unwrap_or("");
    assert!(
        guidance.contains("removed sections a person changed since the last reflection: 'Notes'"),
        "{receipt}"
    );

    let clean = fx
        .engine
        .write_reflection(
            &write(
                "guides/deploying.md",
                &std::fs::read_to_string(&path).unwrap(),
            ),
            &agent(),
        )
        .await
        .unwrap();
    assert!(
        !clean["guidance"]
            .as_str()
            .unwrap_or("")
            .contains("removed sections"),
        "{clean}"
    );
}

/// A thread item carries the label of its opening conventional comment, and
/// a blocking thread comes before the other threads of its section. A plain
/// comment carries no label at all.
#[tokio::test]
async fn a_blocking_thread_comes_first_and_carries_its_label() {
    let fx = curated(&[]).await;
    let text = edit_page(&fx, "", "");
    let id = crystalline_core::reflection::split_sections(&text)[1]
        .id
        .clone()
        .unwrap();
    let mut opened = Vec::new();
    for words in [
        "Is this still true?",
        "question (non-blocking): is this still true?",
        "issue (blocking): the batch window moved\nSee the runbook.",
    ] {
        let thread = fx
            .engine
            .discuss(
                &DiscussParams {
                    domain: "eng".into(),
                    path: Some("guides/deploying.md".into()),
                    section: Some(id.clone()),
                    text: Some(words.into()),
                    ..Default::default()
                },
                &person(),
            )
            .await
            .unwrap();
        opened.push(thread["id"].as_str().unwrap().to_string());
    }
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    let items = queue["queue"].as_array().unwrap();
    let threads: Vec<&str> = items
        .iter()
        .map(|i| i["thread"].as_str().unwrap())
        .collect();
    assert_eq!(
        threads,
        [&opened[2], &opened[0], &opened[1]],
        "the blocking thread first, the rest as before: {queue}"
    );
    assert_eq!(items[0]["label"], "issue", "{queue}");
    assert_eq!(items[0]["blocking"], true, "{queue}");
    let plain = items[1].as_object().unwrap();
    assert!(
        !plain.contains_key("label") && !plain.contains_key("blocking"),
        "{queue}"
    );
    assert_eq!(items[2]["label"], "question", "{queue}");
    assert_eq!(items[2]["blocking"], false, "{queue}");
    let guidance = queue["guidance"].as_str().unwrap();
    assert!(guidance.contains("blocking threads first"), "{guidance}");
    assert!(
        guidance.contains(
            "Answer a person's thread and leave it open. The person who opened it resolves it."
        ),
        "{guidance}"
    );
}

/// `crossing` with `overview.md`: three sections, each built from one
/// engram of `harbor` (team, rota, pager).
async fn cross_curated() -> Fx {
    let fx = crate::sources::crossing().await;
    let mut p = write(
        "overview.md",
        "# Overview\n\nFor people.\n\n## Team\n\nFive.\n\n## Rota\n\nWeekly.\n\n## Pager\n\nOne.\n",
    );
    p.sources = BTreeMap::from([
        (
            "Team".to_string(),
            vec!["crystalline://harbor/team/harbor-team".to_string()],
        ),
        (
            "Rota".to_string(),
            vec!["crystalline://harbor/team/rota".to_string()],
        ),
        (
            "Pager".to_string(),
            vec!["crystalline://harbor/team/pager".to_string()],
        ),
    ]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    fx
}

fn items_of<'a>(queue: &'a serde_json::Value, what: &str) -> Vec<&'a serde_json::Value> {
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == what)
        .collect()
}

fn eng_only() -> ReflectParams {
    ReflectParams {
        domains: vec!["eng".into()],
        ..Default::default()
    }
}

/// An engram of another domain that changes, is retired or is deleted
/// gives the same items a local one gives, each naming the source in its
/// domain. Citing `harbor` engrams covers nothing of `eng`'s own.
#[tokio::test]
async fn a_source_in_another_domain_changes_retires_and_goes() {
    let fx = cross_curated().await;
    let harbor = crate::fixture::root_of(&fx, "harbor");
    std::fs::write(
        harbor.join("team/harbor-team.md"),
        engram("Harbor team", "team/harbor-team", "- [fact] six people"),
    )
    .unwrap();
    std::fs::write(
        harbor.join("team/rota.md"),
        engram("Rota", "team/rota", "- [fact] weekly")
            .replace("status: stable", "status: superseded"),
    )
    .unwrap();
    std::fs::remove_file(harbor.join("team/pager.md")).unwrap();
    fx.engine.sync(None).await.unwrap();

    let queue = fx.engine.reflect(&eng_only(), &agent()).await.unwrap();
    for (what, heading, permalink) in [
        ("source_changed", "Team", "team/harbor-team"),
        ("source_retired", "Rota", "team/rota"),
        ("source_missing", "Pager", "team/pager"),
    ] {
        let items = items_of(&queue, what);
        assert_eq!(items.len(), 1, "{what}: {queue}");
        assert_eq!(items[0]["heading"], heading, "{queue}");
        assert_eq!(items[0]["sources"][0]["permalink"], permalink, "{queue}");
        assert_eq!(items[0]["sources"][0]["domain"], "harbor", "{queue}");
    }
    // eng's runbook is cited by no page of eng: still unreflected.
    let unreflected = items_of(&queue, "unreflected");
    assert_eq!(unreflected.len(), 1, "{queue}");
    assert_eq!(
        unreflected[0]["engrams"],
        serde_json::json!(["runbooks/deploy"])
    );

    // Taking the change in refreshes the foreign checksum.
    let doc = fx
        .engine
        .read_reflection("eng", "overview.md", &agent())
        .await
        .unwrap();
    let team = doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["heading"] == "Team")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    fx.engine
        .settle_reflection("eng", "overview.md", &[team], "taken_in", &agent())
        .await
        .unwrap();
    let queue = fx.engine.reflect(&eng_only(), &agent()).await.unwrap();
    assert!(items_of(&queue, "source_changed").is_empty(), "{queue}");
}

/// Sources in a domain this instance does not have are `unavailable`,
/// never `missing`: one item per page and domain says they cannot be
/// checked here, a reader who may not open such a name gets the plain
/// words of any hidden source, a write by such a reader keeps them and a writer who sees the
/// address may leave one out.
#[tokio::test]
async fn sources_in_a_domain_not_here_are_unavailable_never_missing() {
    let fx = cross_curated().await;
    let abs = fx.root.join("reflections/overview.md");
    let text = std::fs::read_to_string(&abs).unwrap();
    std::fs::write(&abs, text.replace("domain: harbor", "domain: elsewhere")).unwrap();
    fx.engine.sync(None).await.unwrap();

    let doc = fx
        .engine
        .read_reflection("eng", "overview.md", &agent())
        .await
        .unwrap();
    let states: Vec<&str> = doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["sources"].as_array().unwrap())
        .map(|s| s["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["unavailable"; 3], "{doc}");

    let queue = fx.engine.reflect(&eng_only(), &agent()).await.unwrap();
    assert!(items_of(&queue, "source_missing").is_empty(), "{queue}");
    let items = items_of(&queue, "source_unavailable");
    assert_eq!(items.len(), 1, "one per page and domain: {queue}");
    assert_eq!(items[0]["source_domain"], "elsewhere");
    assert_eq!(items[0]["count"], 3);
    assert_eq!(
        items[0]["note"],
        "sources in elsewhere cannot be checked here; keep them while the domain may come back, or drop one by writing its section's sources without it"
    );
    assert_eq!(items[0]["path"], "overview.md");

    // A signed-in account: a name no registration covers is not shown.
    let auth = std::sync::Arc::new(
        crystalline_service::rest::AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user(
        "ed",
        "ed",
        None,
        crystalline_service::rest::Role::Editor,
        "pw12345678",
    )
    .await
    .unwrap();
    fx.engine
        .set_domain_access(std::sync::Arc::new(crystalline_service::DomainAccess::new(
            auth,
        )));
    let ed = ReflectionCaller::agent(
        crystalline_service::Scope::User {
            account: "ed".into(),
            admin: false,
        },
        "claude-code",
        false,
    );
    let queue = fx.engine.reflect(&eng_only(), &ed).await.unwrap();
    assert!(!queue.to_string().contains("elsewhere"), "{queue}");
    assert!(items_of(&queue, "source_unavailable").is_empty(), "{queue}");
    let items = items_of(&queue, "source_changed");
    assert_eq!(
        items.len(),
        3,
        "one per section, as any hidden source: {queue}"
    );
    for item in items {
        assert_eq!(item["hidden_sources"], 1);
        assert_eq!(
            item["note"],
            "a source in a domain you cannot open has changed"
        );
    }

    // A writer who is not shown the name keeps it: a sources write by the
    // account leaves the section's unavailable source as it was.
    let sources_of = |section: &str| {
        let mut p = crystalline_service::params::WriteReflectionParams {
            domain: "eng".into(),
            path: "overview.md".into(),
            ..Default::default()
        };
        p.sources = BTreeMap::from([(section.to_string(), vec!["runbooks/deploy".to_string()])]);
        p
    };
    fx.engine
        .write_reflection(&sources_of("Team"), &ed)
        .await
        .unwrap();
    let text = std::fs::read_to_string(&abs).unwrap();
    assert_eq!(text.matches("domain: elsewhere").count(), 3, "{text}");

    // A writer who sees the address drops it by leaving it out: how a
    // source of a domain that is gone for good goes.
    fx.engine
        .write_reflection(&sources_of("Team"), &agent())
        .await
        .unwrap();
    let text = std::fs::read_to_string(&abs).unwrap();
    assert_eq!(text.matches("domain: elsewhere").count(), 2, "{text}");
    let queue = fx.engine.reflect(&eng_only(), &agent()).await.unwrap();
    assert_eq!(
        items_of(&queue, "source_unavailable")[0]["count"],
        2,
        "{queue}"
    );
}

/// A Reflection restored from its backup branch has no `.reflected/`
/// baselines (the backup does not carry them): the restored pages are the
/// baseline. A section whose text is what was last reflected raises no
/// feedback; a person's change after the restore does, once.
#[tokio::test]
async fn a_restore_without_baselines_takes_the_pages_as_the_baseline() {
    let fx = curated(&[]).await;
    std::fs::remove_dir_all(fx.root.join("reflections/.reflected")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        !rows(&queue).iter().any(|(kind, _, _)| kind == "feedback"),
        "{queue}"
    );
    // A person's change after the restore is feedback, the rest is not.
    let text = edit_page(&fx, "Deploys run at night.", "Deploys run at dawn.");
    fx.engine
        .write_reflection(&write("guides/deploying.md", &text), &person())
        .await
        .unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    let feedback: Vec<String> = rows(&queue)
        .into_iter()
        .filter(|(kind, _, _)| kind == "feedback")
        .map(|(_, _, heading)| heading)
        .collect();
    assert_eq!(feedback, ["Night deploys"], "{queue}");
    // An agent's write of one section keeps the others reflected.
    let text = edit_page(
        &fx,
        "Roll back with the script.",
        "Roll back with the script now.",
    );
    let mut p = write("guides/deploying.md", &text);
    p.sources = BTreeMap::from([(
        "Rollback".to_string(),
        vec!["runbooks/rollback".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    let feedback: Vec<String> = rows(&queue)
        .into_iter()
        .filter(|(kind, _, _)| kind == "feedback")
        .map(|(_, _, heading)| heading)
        .collect();
    assert_eq!(feedback, ["Night deploys"], "{queue}");
}

/// A page a person wrote and no agent ever reflected has nothing to take
/// as its baseline: every section of it stays a person's change.
#[tokio::test]
async fn a_page_never_reflected_stays_feedback_without_a_baseline() {
    let fx = curated(&[]).await;
    fx.engine
        .write_reflection(
            &write("notes.md", "# Notes\n\n## Idea\n\nA person's idea.\n"),
            &person(),
        )
        .await
        .unwrap();
    let queue = fx
        .engine
        .reflect(&ReflectParams::default(), &agent())
        .await
        .unwrap();
    assert!(
        rows(&queue)
            .iter()
            .any(|(kind, _, heading)| kind == "feedback" && heading == "Idea"),
        "{queue}"
    );
}
