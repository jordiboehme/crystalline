//! An agent's own writes are reflected: a section or a pact part it writes,
//! with or without sources, never comes back to it as feedback or as
//! `pact_changed`. A person's edit still does. The pact says how it is read:
//! a write of it names the Structure bullets that are not read as globs,
//! and the queue says so when no glob fits a page.

use crystalline_service::params::ReflectParams;

use crate::docs::{agent, person, reflecting, write};
use crate::fixture::Fx;

const TWO_SECTIONS: &str = "# Deploying\n\nFor new people.\n\n## Night deploys\n\nAt night.\n\n## Rollback\n\nRoll back.\n";

fn page(fx: &Fx, rel: &str) -> String {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).unwrap()
}

async fn queue(fx: &Fx) -> serde_json::Value {
    fx.engine
        .reflect(
            &ReflectParams {
                domains: vec!["eng".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap()
}

fn items<'a>(queue: &'a serde_json::Value, what: &str) -> Vec<&'a serde_json::Value> {
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == what)
        .collect()
}

fn feedback(queue: &serde_json::Value) -> Vec<&serde_json::Value> {
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "feedback")
        .collect()
}

#[tokio::test]
async fn an_agents_write_without_sources_raises_no_feedback() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert!(feedback(&q).is_empty(), "{q}");
    // A later change of one section, still without sources, is its own text
    // too.
    let changed = page(&fx, "a.md").replace("Roll back.", "Roll back fast.");
    fx.engine
        .write_reflection(&write("a.md", &changed), &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert!(feedback(&q).is_empty(), "{q}");
}

#[tokio::test]
async fn a_persons_edit_still_raises_feedback() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let edited = page(&fx, "a.md").replace("At night.", "At dawn.");
    fx.engine
        .write_reflection(&write("a.md", &edited), &person())
        .await
        .unwrap();
    let q = queue(&fx).await;
    let owed = feedback(&q);
    assert_eq!(owed.len(), 1, "{q}");
    assert_eq!(owed[0]["heading"], "Night deploys", "{q}");
    assert_eq!(owed[0]["changed_by"][0], "jordi", "{q}");
}

/// A section an agent removes is its own change too; a section a person
/// changed and the agent then sends back changed again still owes the
/// person's change.
#[tokio::test]
async fn an_agents_removal_is_reflected_and_a_persons_change_stays_through_its_rewrite() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let stored = page(&fx, "a.md");
    let rollback = stored.find("## Rollback").unwrap();
    fx.engine
        .write_reflection(&write("a.md", &stored[..rollback]), &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert!(feedback(&q).is_empty(), "{q}");

    let edited = page(&fx, "a.md").replace("At night.", "At dawn.");
    fx.engine
        .write_reflection(&write("a.md", &edited), &person())
        .await
        .unwrap();
    let rewritten = page(&fx, "a.md").replace("At dawn.", "At dawn, after the batch.");
    fx.engine
        .write_reflection(&write("a.md", &rewritten), &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    let owed = feedback(&q);
    assert_eq!(owed.len(), 1, "{q}");
    assert_eq!(owed[0]["heading"], "Night deploys", "{q}");
    assert_eq!(owed[0]["changed_by"][0], "jordi", "{q}");
}

/// A heading a person typed in a plain editor gets its id on the agent's
/// next write of another section, and stays the person's feedback.
#[tokio::test]
async fn a_hand_typed_heading_stays_feedback_through_an_agents_section_write() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let path = fx.root.join("reflections/a.md");
    let typed = page(&fx, "a.md") + "\n## Typed by hand\n\nA person wrote this.\n";
    std::fs::write(&path, typed).unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut p = write("a.md", "");
    p.markdown = None;
    p.operation = Some("replace_section".into());
    p.section = Some("## Rollback".into());
    p.content = Some("Roll back with the script.".into());
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let q = queue(&fx).await;
    let owed = feedback(&q);
    assert_eq!(owed.len(), 1, "{q}");
    assert_eq!(owed[0]["heading"], "Typed by hand", "{q}");
    assert_eq!(owed[0]["what"], "new", "{q}");
}

/// An agent's new section that shares its heading with a person's changed
/// section elsewhere on the page is the agent's own text: only the person's
/// change is owed.
#[tokio::test]
async fn an_agents_new_section_named_like_a_persons_changed_one_is_its_own() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let edited = page(&fx, "a.md").replace("At night.", "At dawn.");
    fx.engine
        .write_reflection(&write("a.md", &edited), &person())
        .await
        .unwrap();
    let grown = page(&fx, "a.md") + "\n## Night deploys\n\nA second part.\n";
    fx.engine
        .write_reflection(&write("a.md", &grown), &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    let owed = feedback(&q);
    assert_eq!(owed.len(), 1, "{q}");
    assert_eq!(owed[0]["what"], "changed", "{q}");
    assert_eq!(owed[0]["changed_by"][0], "jordi", "{q}");
}

const PACT: &str = "# How we write\n\n## Audience\n\n- New people.\n\n## Structure\n\nOne glob per bullet.\n\n- *.md\n";

#[tokio::test]
async fn an_agents_pact_write_raises_no_pact_changed() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("PACT.md", PACT), &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert!(items(&q, "pact_changed").is_empty(), "{q}");
}

#[tokio::test]
async fn a_persons_pact_edit_raises_pact_changed_with_its_sections() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("PACT.md", PACT), &agent())
        .await
        .unwrap();
    let edited = page(&fx, "PACT.md").replace("- New people.", "- New people and visitors.");
    fx.engine
        .write_reflection(&write("PACT.md", &edited), &person())
        .await
        .unwrap();
    let audience = crystalline_core::reflection::split_sections(&page(&fx, "PACT.md"))
        .into_iter()
        .find(|s| s.heading == "Audience")
        .and_then(|s| s.id)
        .unwrap();
    let q = queue(&fx).await;
    let changed = items(&q, "pact_changed");
    assert_eq!(changed.len(), 1, "{q}");
    assert_eq!(changed[0]["sections"], serde_json::json!([audience]), "{q}");
}

#[tokio::test]
async fn settle_without_sections_settles_the_whole_page() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let stored = page(&fx, "a.md");
    let rollback = stored.find("## Rollback").unwrap();
    // A person changes the top and a section and removes another.
    let edited = stored[..rollback]
        .replace("For new people.", "For everyone.")
        .replace("At night.", "At dawn.");
    fx.engine
        .write_reflection(&write("a.md", &edited), &person())
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert_eq!(feedback(&q).len(), 3, "{q}");
    let settled = fx
        .engine
        .settle_reflection("eng", "a.md", &[], "kept", &agent())
        .await
        .unwrap();
    assert!(
        settled["settled"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "_top"),
        "{settled}"
    );
    let q = queue(&fx).await;
    assert!(feedback(&q).is_empty(), "{q}");
}

#[tokio::test]
async fn settle_without_sections_settles_a_changed_pact() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("PACT.md", PACT), &person())
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert_eq!(items(&q, "pact_changed").len(), 1, "{q}");
    fx.engine
        .settle_reflection("eng", "PACT.md", &[], "kept", &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert!(items(&q, "pact_changed").is_empty(), "{q}");
}

#[tokio::test]
async fn a_prose_structure_bullet_is_named_in_the_receipt() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(
            &write("eng/README.md", "# Eng\n\n## About\n\nText.\n"),
            &agent(),
        )
        .await
        .unwrap();
    let pact = "# How we write\n\n## Structure\n\n- a folder per domain\n- */README.md\n";
    let written = fx
        .engine
        .write_reflection(&write("PACT.md", pact), &agent())
        .await
        .unwrap();
    assert_eq!(
        written["structure_findings"],
        serde_json::json!([
            {
                "bullet": "a folder per domain",
                "reason": crystalline_core::reflection::READS_AS_A_SENTENCE
            }
        ]),
        "{written}"
    );
    // A pact whose every bullet is read as a glob that fits a page names
    // nothing.
    let fitting = page(&fx, "PACT.md").replace("- a folder per domain\n", "");
    let written = fx
        .engine
        .write_reflection(&write("PACT.md", &fitting), &agent())
        .await
        .unwrap();
    assert!(written.get("structure_findings").is_none(), "{written}");
}

#[tokio::test]
async fn off_structure_carries_a_hint_when_no_glob_fits() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &write(
                "PACT.md",
                "# How we write\n\n## Structure\n\n- a page per topic\n",
            ),
            &agent(),
        )
        .await
        .unwrap();
    let q = queue(&fx).await;
    let off = items(&q, "off_structure");
    assert_eq!(off.len(), 1, "{q}");
    assert_eq!(
        off[0]["hint"],
        crystalline_service::engine::STRUCTURE_HINT,
        "{q}"
    );

    // One glob that fits a page: a page off it carries no hint.
    fx.engine
        .write_reflection(
            &write(
                "PACT.md",
                &page(&fx, "PACT.md").replace("- a page per topic", "- guides/*.md"),
            ),
            &agent(),
        )
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("guides/b.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let q = queue(&fx).await;
    let off = items(&q, "off_structure");
    assert_eq!(off.len(), 1, "{q}");
    assert!(off[0].get("hint").is_none(), "{q}");
}

/// A page pact (`<page>.pact.md`) is no page: it is not a page in the
/// navigation tree, it is never off the pact's structure, and it does not
/// count as a page a structure glob fits. Its write names no unsourced
/// section, as PACT.md's does not.
#[tokio::test]
async fn a_page_pact_is_no_page_in_the_tree_the_queue_or_the_structure_findings() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("guides/a.md", TWO_SECTIONS), &agent())
        .await
        .unwrap();
    let written = fx
        .engine
        .write_reflection(
            &write("guides/a.pact.md", "# Pact of a\n\n## Tone\n\nShort.\n"),
            &agent(),
        )
        .await
        .unwrap();
    assert!(written.get("unsourced_sections").is_none(), "{written}");

    // A glob that fits only the page pact fits no page.
    let pact = "# How we write\n\n## Structure\n\n- guides/?.md\n- guides/*.pact.md\n";
    let written = fx
        .engine
        .write_reflection(&write("PACT.md", pact), &agent())
        .await
        .unwrap();
    assert_eq!(
        written["structure_findings"],
        serde_json::json!([
            {
                "bullet": "guides/*.pact.md",
                "reason": crystalline_core::reflection::MATCHES_NO_PAGE
            }
        ]),
        "{written}"
    );

    // With a structure the page pact does not fit, it is still not off it.
    fx.engine
        .write_reflection(
            &write(
                "PACT.md",
                &page(&fx, "PACT.md").replace("- guides/*.pact.md\n", ""),
            ),
            &agent(),
        )
        .await
        .unwrap();
    let q = queue(&fx).await;
    assert!(items(&q, "off_structure").is_empty(), "{q}");
    assert!(
        !q.to_string().contains("guides/a.pact.md"),
        "no item names the page pact as a page: {q}"
    );

    let tree = fx
        .engine
        .read_reflection_tree("eng", &agent())
        .await
        .unwrap();
    let text = tree.to_string();
    assert!(text.contains("guides/a.md"), "{tree}");
    assert!(!text.contains("a.pact.md"), "{tree}");
}
