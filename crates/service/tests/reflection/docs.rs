//! Reflections through the engine: write, curate, read, move, delete, settle.

use std::collections::BTreeMap;

use crystalline_core::reflection::feedback_changes;
use crystalline_core::threads::{
    Anchor, Author, Entry, EntryKind, SectionRef, Thread, ThreadStatus, open_thread,
};
use crystalline_service::ReflectionCaller;
use crystalline_service::params::WriteReflectionParams;

use crate::fixture::{Fx, engram, fixture, manifest};

pub async fn reflecting() -> Fx {
    fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
        ),
    ])
    .await
}

pub fn write(path: &str, markdown: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        domain: "eng".into(),
        path: path.into(),
        markdown: Some(markdown.into()),
        ..Default::default()
    }
}

pub fn person() -> ReflectionCaller {
    ReflectionCaller::person(crystalline_service::Scope::Unrestricted, "jordi")
}

pub fn agent() -> ReflectionCaller {
    ReflectionCaller::agent(
        crystalline_service::Scope::Unrestricted,
        "claude-code",
        false,
    )
}

const PAGE: &str = "# Deploying\n\nFor new people.\n\n## Night deploys\n\nDeploys run at night.\n";

fn file(fx: &Fx, rel: &str) -> String {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).unwrap()
}

#[tokio::test]
async fn a_write_assigns_ids_and_records_who_changed_what() {
    let fx = reflecting().await;
    let written = fx
        .engine
        .write_reflection(&write("guides/deploying.md", PAGE), &person())
        .await
        .unwrap();
    assert_eq!(written["created"], true);
    let ids = written["ids_assigned"].as_array().unwrap().clone();
    assert_eq!(ids.len(), 1);
    let text = file(&fx, "guides/deploying.md");
    assert!(text.contains(&format!(
        "## Night deploys\n<!-- r:{} -->\n",
        ids[0].as_str().unwrap()
    )));
    let prov = crystalline_core::reflection::read_provenance(&text);
    assert_eq!(
        prov[ids[0].as_str().unwrap()].changed_by,
        vec!["jordi".to_string()]
    );
    let doc = fx
        .engine
        .read_reflection("eng", "guides/deploying.md", &person())
        .await
        .unwrap();
    assert_eq!(doc["title"], "Deploying");
    assert_eq!(doc["sections"][0]["changed"], true, "never reflected");
}

#[tokio::test]
async fn the_bookkeeping_block_cannot_be_written_from_outside() {
    let fx = reflecting().await;
    let forged = "---\nreflection:\n  aaaa1111:\n    reflected: forged\n---\n\n## A\n<!-- r:aaaa1111 -->\n\nText.\n";
    fx.engine
        .write_reflection(&write("a.md", forged), &person())
        .await
        .unwrap();
    let prov = crystalline_core::reflection::read_provenance(&file(&fx, "a.md"));
    assert_eq!(
        prov["aaaa1111"].reflected, None,
        "the forged hash was dropped"
    );
}

#[tokio::test]
async fn writing_with_sources_curates_and_advances_the_baseline() {
    let fx = reflecting().await;
    let mut p = write("guides/deploying.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let text = file(&fx, "guides/deploying.md");
    let baseline = file(&fx, ".reflected/guides/deploying.md");
    let changes = feedback_changes(&text, &baseline);
    assert!(
        changes.is_empty(),
        "the agent's own top is reflected with the section: {changes:?}"
    );
    let doc = fx
        .engine
        .read_reflection("eng", "guides/deploying.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        doc["sections"][0]["sources"][0]["permalink"],
        "runbooks/deploy"
    );
    assert_eq!(doc["sections"][0]["sources"][0]["state"], "current");

    let mut bad = write("b.md", PAGE);
    bad.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/nothere".to_string()],
    )]);
    let err = fx
        .engine
        .write_reflection(&bad, &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("runbooks/nothere"), "{err}");
    assert!(
        !fx.root.join("reflections/b.md").exists(),
        "nothing written"
    );
}

#[tokio::test]
async fn a_write_that_drops_the_id_lines_keeps_its_ids_and_sources() {
    let fx = reflecting().await;
    let mut p = write("a.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let id = crystalline_core::reflection::split_sections(&file(&fx, "a.md"))[1]
        .id
        .clone()
        .unwrap();
    let thread = Thread {
        id: "t-aaaa0002".into(),
        anchor: Anchor {
            section: SectionRef::Id(id.clone()),
            quote: Some("run at night".into()),
            occurrence: 1,
        },
        status: ThreadStatus::Open,
        entries: vec![Entry {
            at: "2026-09-30T10:00:00Z".into(),
            kind: EntryKind::Comment,
            author: Author {
                name: "jordi".into(),
                agent: false,
            },
            text: "?".into(),
        }],
    };
    std::fs::write(
        fx.root.join("reflections/a.threads.md"),
        open_thread("", &thread),
    )
    .unwrap();
    // An editor that drops HTML comments sends the page back without them.
    let written = fx
        .engine
        .write_reflection(&write("a.md", PAGE), &person())
        .await
        .unwrap();
    assert_eq!(
        written["ids_assigned"].as_array().unwrap().len(),
        0,
        "{written}"
    );
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(doc["sections"][0]["id"], id.as_str());
    assert_eq!(
        doc["sections"][0]["sources"][0]["permalink"], "runbooks/deploy",
        "provenance kept"
    );
    assert_eq!(doc["threads"][0]["anchor"]["state"], "found");
}

#[tokio::test]
async fn a_stale_checksum_is_refused() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", PAGE), &person())
        .await
        .unwrap();
    let mut p = write("a.md", "# Other\n");
    p.expected_checksum = Some("0000".into());
    let err = fx
        .engine
        .write_reflection(&p, &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("stale edit"), "{err}");
}

#[tokio::test]
async fn a_create_only_write_refuses_an_existing_page() {
    let fx = reflecting().await;
    let mut create = write("a.md", PAGE);
    create.create_only = true;
    let made = fx
        .engine
        .write_reflection(&create, &person())
        .await
        .unwrap();
    assert_eq!(made["created"], true, "a free path is created");
    let before = file(&fx, "a.md");
    let mut again = write("a.md", "# Somebody else's page\n");
    again.create_only = true;
    let err = fx
        .engine
        .write_reflection(&again, &person())
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err, "a reflection already lives at 'a.md'");
    assert_eq!(file(&fx, "a.md"), before, "the page is untouched");
}

#[tokio::test]
async fn every_section_line_points_at_its_heading_in_the_markdown_answered() {
    let fx = reflecting().await;
    let mut p = write(
        "guides/deploying.md",
        "# Deploying\n\nFor new people.\n\n## Night deploys\n\nDeploys run at night.\n\n## Rollback\n\nRun the script.\n",
    );
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    let written = fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert!(
        file(&fx, "guides/deploying.md").starts_with("---\nreflection:"),
        "the page carries its block"
    );
    let doc = fx
        .engine
        .read_reflection("eng", "guides/deploying.md", &agent())
        .await
        .unwrap();
    let markdown = doc["markdown"].as_str().unwrap();
    assert!(
        markdown.contains("engram: runbooks/deploy"),
        "a reader of engrams gets the whole file"
    );
    for section in doc["sections"].as_array().unwrap() {
        let line = section["line"].as_u64().unwrap() as usize;
        let heading = format!("## {}", section["heading"].as_str().unwrap());
        assert_eq!(
            markdown.lines().nth(line - 1),
            Some(heading.as_str()),
            "{doc}"
        );
    }
    assert_eq!(
        doc["checksum"], written["checksum"],
        "the checksum of the file as stored"
    );
}

#[tokio::test]
async fn ticking_a_task_box_does_not_mark_a_section_changed() {
    let fx = reflecting().await;
    let page =
        "# Steps\n\nFor new people.\n\n## Night deploys\n\n- [ ] stop the job\n- [ ] restore\n";
    let mut p = write("a.md", page);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let stored = file(&fx, "a.md");
    let ticked = stored.replace("- [ ] stop the job", "- [x] stop the job");
    fx.engine
        .write_reflection(&write("a.md", &ticked), &person())
        .await
        .unwrap();
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(doc["sections"][0]["changed"], false, "{doc}");
    let worded = ticked.replace("- [ ] restore", "- [ ] restore the data");
    fx.engine
        .write_reflection(&write("a.md", &worded), &person())
        .await
        .unwrap();
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(doc["sections"][0]["changed"], true, "{doc}");
}

#[tokio::test]
async fn the_baseline_survives_an_index_rebuild() {
    let fx = reflecting().await;
    let mut p = write("a.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    fx.engine.reindex(true).await.unwrap();
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        doc["sections"][0]["changed"], false,
        "reflected text still known"
    );
    let tree = fx
        .engine
        .read_reflection_tree("eng", &agent())
        .await
        .unwrap();
    assert_eq!(tree["nodes"][0]["path"], "a.md");
}

#[tokio::test]
async fn moving_a_reflection_carries_its_open_thread_its_baseline_and_the_links_to_it() {
    let fx = reflecting().await;
    let mut p = write("guides/deploying.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    fx.engine
        .write_reflection(
            &write(
                "home.md",
                "# Home\n\nSee [deploying](guides/deploying.md#night-deploys).\n",
            ),
            &person(),
        )
        .await
        .unwrap();
    let id = crystalline_core::reflection::split_sections(&file(&fx, "guides/deploying.md"))[1]
        .id
        .clone()
        .unwrap();
    let thread = Thread {
        id: "t-aaaa0001".into(),
        anchor: Anchor {
            section: SectionRef::Id(id.clone()),
            quote: Some("run at night".into()),
            occurrence: 1,
        },
        status: ThreadStatus::Open,
        entries: vec![Entry {
            at: "2026-09-30T10:00:00Z".into(),
            kind: EntryKind::Comment,
            author: Author {
                name: "jordi".into(),
                agent: false,
            },
            text: "Still true?".into(),
        }],
    };
    std::fs::write(
        fx.root.join("reflections/guides/deploying.threads.md"),
        open_thread("", &thread),
    )
    .unwrap();

    let moved = fx
        .engine
        .move_reflection(
            "eng",
            "guides/deploying.md",
            "ops/night/deploying.md",
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(moved["links_rewritten"], 1);
    assert_eq!(moved["threads_moved"], true);
    assert!(!fx.root.join("reflections/guides/deploying.md").exists());
    assert!(
        fx.root
            .join("reflections/ops/night/deploying.threads.md")
            .exists()
    );
    assert!(
        fx.root
            .join("reflections/.reflected/ops/night/deploying.md")
            .exists(),
        "the baseline moved"
    );
    assert!(file(&fx, "home.md").contains("](ops/night/deploying.md#night-deploys)"));
    let doc = fx
        .engine
        .read_reflection("eng", "ops/night/deploying.md", &person())
        .await
        .unwrap();
    assert_eq!(doc["threads"][0]["id"], "t-aaaa0001");
    assert_eq!(
        doc["threads"][0]["anchor"]["state"], "found",
        "the thread still anchors on its section"
    );
    assert_eq!(doc["sections"][0]["changed"], false, "provenance travelled");
}

/// A move rewrites relative links in the moved page, in its
/// baseline the same way, and in the pages that link to it. None of that is
/// a person's change: no section owes feedback, and every `reflected` hash
/// matches the rewritten section, which is the rewritten baseline section.
#[tokio::test]
async fn moving_a_page_with_relative_links_owes_no_feedback() {
    use crystalline_core::reflection as r;
    let fx = reflecting().await;
    fx.engine
        .write_reflection(
            &write(
                "guides/rollback.md",
                "# Rollback\n\nRoll back with the script.\n",
            ),
            &person(),
        )
        .await
        .unwrap();
    let mut p = write(
        "guides/deploying.md",
        "# Deploying\n\nFor new people.\n\n## Night deploys\n\nDeploys run at night; see [rollback](rollback.md).\n",
    );
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let mut home = write(
        "home.md",
        "# Home\n\n## Start\n\nSee [deploying](guides/deploying.md).\n",
    );
    home.sources = BTreeMap::from([("Start".to_string(), vec!["runbooks/deploy".to_string()])]);
    fx.engine.write_reflection(&home, &agent()).await.unwrap();

    // The unreflected `_top` of a page is feedback before the move already;
    // the move must not add any.
    let owed_before: Vec<usize> = ["guides/deploying.md", "home.md"]
        .iter()
        .map(|p| r::feedback_changes(&file(&fx, p), &file(&fx, &r::baseline_path(p))).len())
        .collect();

    fx.engine
        .move_reflection("eng", "guides/deploying.md", "ops/deploying.md", &person())
        .await
        .unwrap();

    for ((page_path, link), before) in [
        ("ops/deploying.md", "](../guides/rollback.md)"),
        ("home.md", "](ops/deploying.md)"),
    ]
    .into_iter()
    .zip(owed_before)
    {
        let page = file(&fx, page_path);
        let baseline = file(&fx, &r::baseline_path(page_path));
        assert!(
            page.contains(link) && baseline.contains(link),
            "{page_path}: page and baseline follow the link"
        );
        assert_eq!(
            r::feedback_changes(&page, &baseline).len(),
            before,
            "{page_path} owes no more feedback than before the move"
        );
        let prov = r::read_provenance(&page);
        let mut checked = 0;
        for section in r::split_sections(&page) {
            let Some(id) = section.id else { continue };
            let Some(hash) = prov.get(&id).and_then(|record| record.reflected.clone()) else {
                continue;
            };
            assert_eq!(
                hash,
                r::section_hash(&section.text),
                "{page_path} {id}: the hash follows the rewrite"
            );
            checked += 1;
        }
        assert!(checked > 0, "{page_path} has a reflected section");
    }
    let doc = fx
        .engine
        .read_reflection("eng", "ops/deploying.md", &person())
        .await
        .unwrap();
    assert!(
        doc["sections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["changed"] == false),
        "{doc}"
    );
}

#[tokio::test]
async fn delete_takes_the_threads_and_the_baseline_and_the_pact_stays() {
    let fx = reflecting().await;
    // Written with sources, so a baseline exists to be taken.
    let mut p = write("a.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert!(
        fx.root.join("reflections/.reflected/a.md").exists(),
        "the curated write left a baseline"
    );
    std::fs::write(fx.root.join("reflections/a.threads.md"), "# Threads\n").unwrap();
    fx.engine
        .write_reflection(
            &write("PACT.md", "# How we write\n\n## Audience\n\nEveryone.\n"),
            &agent(),
        )
        .await
        .unwrap();
    fx.engine
        .delete_reflection("eng", "a.md", &person())
        .await
        .unwrap();
    assert!(!fx.root.join("reflections/a.md").exists(), "the page goes");
    assert!(
        !fx.root.join("reflections/a.threads.md").exists(),
        "its threads go"
    );
    assert!(
        !fx.root.join("reflections/.reflected/a.md").exists(),
        "its baseline goes"
    );
    let err = fx
        .engine
        .delete_reflection("eng", "PACT.md", &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("PACT.md"), "{err}");
    assert!(
        fx.root.join("reflections/PACT.md").exists(),
        "the pact stays"
    );
}

#[tokio::test]
async fn a_move_refuses_a_target_that_still_holds_threads_or_a_baseline() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", PAGE), &person())
        .await
        .unwrap();
    // A leftover of an earlier page at the target would be adopted by the
    // moved one.
    std::fs::write(fx.root.join("reflections/b.threads.md"), "# Threads\n").unwrap();
    let err = fx
        .engine
        .move_reflection("eng", "a.md", "b.md", &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("b.threads.md"), "{err}");
    assert!(
        fx.root.join("reflections/a.md").exists(),
        "a refused move changes nothing"
    );
}

/// Open point 1 (approved): one section through edit_engram's operations,
/// its id and its provenance kept.
#[tokio::test]
async fn a_section_operation_changes_one_section_and_keeps_its_id_and_sources() {
    let fx = reflecting().await;
    let mut p = write(
        "a.md",
        "# Deploying\n\n## Night deploys\n\nDeploys run at night.\n\n## Rollback\n\nRun the script.\n",
    );
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let before = crystalline_core::reflection::split_sections(&file(&fx, "a.md"));
    let night_id = before[1].id.clone().unwrap();
    let rollback_id = before[2].id.clone().unwrap();
    let op = WriteReflectionParams {
        domain: "eng".into(),
        path: "a.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## Rollback".into()),
        content: Some("Run the script, then check the queue.".into()),
        ..Default::default()
    };
    let written = fx.engine.write_reflection(&op, &person()).await.unwrap();
    assert_eq!(
        written["ids_assigned"].as_array().unwrap().len(),
        0,
        "{written}"
    );
    let text = file(&fx, "a.md");
    assert!(
        text.contains(&format!(
            "## Rollback\n<!-- r:{rollback_id} -->\n\nRun the script, then check the queue.\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "## Night deploys\n<!-- r:{night_id} -->\n\nDeploys run at night.\n"
        )),
        "the other section is untouched: {text}"
    );
    let prov = crystalline_core::reflection::read_provenance(&text);
    assert_eq!(prov[&night_id].sources.len(), 1, "provenance kept");
    // The agent's curated write reflected the section it wrote without
    // sources too, so only the person who changed it since is on the list.
    assert_eq!(
        prov[&rollback_id].changed_by,
        vec!["jordi".to_string()],
        "the edited section records who changed it"
    );
    let after = WriteReflectionParams {
        operation: Some("insert_after_section".into()),
        section: Some("## Rollback".into()),
        content: Some("Ask first.".into()),
        ..op.clone()
    };
    fx.engine.write_reflection(&after, &person()).await.unwrap();
    assert!(
        file(&fx, "a.md").contains(&format!(
            "## Rollback\n<!-- r:{rollback_id} -->\n\nAsk first.\n"
        )),
        "under the id line"
    );
    assert_eq!(
        file(&fx, "a.md")
            .matches(&format!("<!-- r:{rollback_id} -->"))
            .count(),
        1,
        "the id line stands once, under its heading"
    );
    let missing = WriteReflectionParams {
        section: Some("## Nowhere".into()),
        ..op.clone()
    };
    assert!(
        fx.engine
            .write_reflection(&missing, &person())
            .await
            .is_err()
    );
    let both = WriteReflectionParams {
        markdown: Some("# X\n".into()),
        ..op
    };
    let err = fx
        .engine
        .write_reflection(&both, &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("markdown") && err.contains("operation"),
        "one or the other: {err}"
    );
}

/// Open point 2 (approved): a page's cross-domain spellings are written with
/// the canonical name, as an engram's are.
#[tokio::test]
async fn a_write_spells_other_domains_by_their_canonical_name() {
    let fx = crate::fixture::fixture_with(
        &[("MANIFEST.md", &manifest("reflection: on\n"))],
        |cfg, _| {
            cfg.domains.get_mut("eng").unwrap().aliases = vec!["engineering".to_string()];
        },
    )
    .await;
    let written = fx
        .engine
        .write_reflection(&write("a.md", "# A\n\nSee [[engineering:runbooks/deploy]] and crystalline://engineering/runbooks/deploy.\n"), &person())
        .await
        .unwrap();
    assert_eq!(written["domain_names_normalized"], 2, "{written}");
    let text = file(&fx, "a.md");
    assert!(
        text.contains("[[eng:runbooks/deploy]]")
            && text.contains("crystalline://eng/runbooks/deploy"),
        "{text}"
    );
}

/// `eng` with Reflection on and a deploy runbook, and a second domain
/// `ops` whose MANIFEST takes `ops_manifest_extra` (each line ending in `\n`)
/// and whose files are `ops_files`.
async fn with_ops(
    eng_manifest_extra: &str,
    ops_manifest_extra: &str,
    ops_files: &[(&str, &str)],
) -> Fx {
    let ops_manifest = crystalline_core::manifest_template("ops", "2026-01-01").replacen(
        "\n---\n",
        &format!("\n{ops_manifest_extra}---\n"),
        1,
    );
    let ops_files: Vec<(String, String)> = ops_files
        .iter()
        .map(|(p, t)| (p.to_string(), t.to_string()))
        .collect();
    crate::fixture::fixture_with(
        &[
            (
                "MANIFEST.md",
                &manifest(&format!("reflection: on\n{eng_manifest_extra}")),
            ),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
        move |cfg, base| {
            let ops = base.join("ops");
            std::fs::create_dir_all(&ops).unwrap();
            std::fs::write(ops.join("MANIFEST.md"), ops_manifest).unwrap();
            for (rel, text) in ops_files {
                let abs = ops.join(rel);
                std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
                std::fs::write(abs, text).unwrap();
            }
            cfg.domains.insert(
                "ops".into(),
                crystalline_core::config::DomainEntry::file(ops),
            );
        },
    )
    .await
}

async fn rename_ops(fx: &Fx) -> serde_json::Value {
    fx.engine
        .rename_domain(
            "ops",
            "platform",
            false,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// Open point 2 (approved): a domain rename respells the reflection pages
/// that name the old domain, as it respells engrams, the renamed domain's
/// own pages included. The respell is a link rewrite, not a person's change:
/// the baseline gets the same rewrite and a curated section keeps its
/// `reflected` hash, so the rename owes no feedback.
#[tokio::test]
async fn a_domain_rename_respells_the_reflection_pages() {
    use crystalline_core::reflection as r;
    let own = "# B\n\n## Links\n<!-- r:bbbb2222 -->\n\nSee [[ops:runbook]].\n";
    let fx = with_ops("", "reflection: on\n", &[("reflections/b.md", own)]).await;
    let mut p = write(
        "a.md",
        "# A\n\nSee [[ops:runbook]] and crystalline://ops/runbook.\n\n## Night deploys\n\nAsk [[ops:pager]] first.\n",
    );
    p.sources = BTreeMap::from([
        ("_top".to_string(), vec!["runbooks/deploy".to_string()]),
        (
            "Night deploys".to_string(),
            vec!["runbooks/deploy".to_string()],
        ),
    ]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let owed_before = r::feedback_changes(&file(&fx, "a.md"), &file(&fx, ".reflected/a.md")).len();
    let mut rx = fx.engine.changes().subscribe();

    let report = rename_ops(&fx).await;

    let text = file(&fx, "a.md");
    let baseline = file(&fx, ".reflected/a.md");
    assert!(
        text.contains("[[platform:runbook]]") && text.contains("crystalline://platform/runbook"),
        "{text}"
    );
    assert!(
        baseline.contains("[[platform:pager]]") && !baseline.contains("[[ops:"),
        "the baseline follows: {baseline}"
    );
    assert_eq!(
        r::feedback_changes(&text, &baseline).len(),
        owed_before,
        "the rename owes no feedback"
    );
    let prov = r::read_provenance(&text);
    for section in r::split_sections(&text) {
        let id = section.id.clone().unwrap();
        assert_eq!(
            prov[&id].reflected,
            Some(r::section_hash(&section.text)),
            "{id}: the hash follows the respell"
        );
        assert!(prov[&id].changed_by.is_empty(), "{id}: nobody changed it");
    }
    let own = std::fs::read_to_string(fx.tmp.path().join("ops/reflections/b.md")).unwrap();
    assert!(
        own.contains("[[platform:runbook]]"),
        "the renamed domain's own page: {own}"
    );
    let rewritten = report["rewritten"].as_array().unwrap();
    assert!(
        rewritten
            .iter()
            .any(|row| row["domain"] == "eng" && row["references"] == 3),
        "{report}"
    );
    assert!(
        rewritten.iter().any(|row| row["domain"] == "ops"),
        "{report}"
    );
    // One `changed` per page the respell rewrote, by nobody: eng's a.md and
    // the renamed domain's own b.md.
    let mut changed = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let crystalline_service::changes::Change::Reflection(change) = envelope.change {
            assert_eq!(
                change.kind,
                crystalline_service::changes::ReflectionChangeKind::Changed
            );
            assert_eq!(change.actor, None);
            changed.push(change.path);
        }
    }
    changed.sort();
    assert_eq!(changed, ["a.md", "b.md"]);
}

/// A reflection page in a domain the rename may not write is listed under
/// `left_behind` and left as it is, like an engram there.
#[tokio::test]
async fn a_page_in_a_read_only_domain_is_left_behind_by_a_rename() {
    let page = "# A\n\n## Links\n<!-- r:aaaa1111 -->\n\nSee [[ops:runbook]] and crystalline://ops/runbook.\n";
    let fx = with_ops("read_only: on\n", "", &[]).await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/a.md"), page).unwrap();
    fx.engine.sync(None).await.unwrap();

    let report = rename_ops(&fx).await;

    assert_eq!(
        report["left_behind"],
        serde_json::json!([{ "domain": "eng", "path": "reflections/a.md", "references": 2 }]),
        "{report}"
    );
    assert_eq!(file(&fx, "a.md"), page, "the page is untouched");
}

/// A reflection row carries the file's own stamp, so the next sync does
/// not read the file Crystalline just wrote.
#[tokio::test]
async fn a_written_reflection_is_not_read_again_by_the_next_sync() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", PAGE), &person())
        .await
        .unwrap();
    let synced = fx.engine.sync(None).await.unwrap();
    let reports = synced["reports"].as_array().unwrap();
    assert!(
        reports.iter().all(|r| r["reflections_changed"] == 0),
        "{synced}"
    );
}

#[tokio::test]
async fn settle_validates_its_path() {
    let fx = reflecting().await;
    let err = fx
        .engine
        .settle_reflection(
            "eng",
            "../MANIFEST.md",
            &["aaaa1111".to_string()],
            "kept",
            &agent(),
        )
        .await
        .unwrap_err()
        .to_string();
    // The path rule's own words: a path echoed back in another refusal
    // would carry the `..` too.
    assert!(err.contains("segment"), "{err}");
    assert_eq!(
        std::fs::read_to_string(fx.root.join("MANIFEST.md")).unwrap(),
        manifest("reflection: on\n"),
        "nothing outside reflections/ is touched"
    );
}

/// The top of a page whose frontmatter holds only the bookkeeping block is
/// cut the same way on both sides of a write, so a write that leaves the top
/// alone does not record a change of it.
#[tokio::test]
async fn a_write_that_leaves_the_top_alone_does_not_mark_it_changed() {
    let fx = reflecting().await;
    let mut p = write("a.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let edited = file(&fx, "a.md").replace(
        "Deploys run at night.",
        "Deploys run at night, after the batch.",
    );
    fx.engine
        .write_reflection(&write("a.md", &edited), &person())
        .await
        .unwrap();
    let prov = crystalline_core::reflection::read_provenance(&file(&fx, "a.md"));
    assert!(
        prov["_top"].changed_by.is_empty() && prov["_top"].changed_at.is_none(),
        "the agent's curated write reflected the top, and the person did not change it: {:?}",
        prov["_top"]
    );
}

/// A curated top carries the hash of its text in the file as stored, after
/// the frontmatter the bookkeeping block brings, so it reads as reflected.
#[tokio::test]
async fn a_curated_top_is_reflected_as_stored() {
    use crystalline_core::reflection as r;
    let fx = reflecting().await;
    let mut p = write("a.md", PAGE);
    p.sources = BTreeMap::from([("_top".to_string(), vec!["runbooks/deploy".to_string()])]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let text = file(&fx, "a.md");
    let top = r::split_sections(&text).remove(0);
    assert_eq!(
        r::read_provenance(&text)["_top"].reflected,
        Some(r::section_hash(&top.text)),
        "{text}"
    );
    let baseline = file(&fx, ".reflected/a.md");
    assert!(baseline.starts_with("<!-- r:_top -->\n"), "{baseline}");
    assert!(
        r::feedback_changes(&text, &baseline)
            .iter()
            .all(|c| c.id.as_deref() != Some("_top")),
        "the curated top owes no feedback"
    );
}

#[tokio::test]
async fn settling_as_kept_advances_the_baseline_and_keeps_the_sources() {
    let fx = reflecting().await;
    let mut p = write("a.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let edited = file(&fx, "a.md").replace(
        "Deploys run at night.",
        "Deploys run at night, after the batch.",
    );
    fx.engine
        .write_reflection(&write("a.md", &edited), &person())
        .await
        .unwrap();
    let id = crystalline_core::reflection::split_sections(&edited)[1]
        .id
        .clone()
        .unwrap();
    assert_eq!(
        feedback_changes(&file(&fx, "a.md"), &file(&fx, ".reflected/a.md"))
            .iter()
            .filter(|c| c.id.as_deref() == Some(id.as_str()))
            .count(),
        1
    );
    fx.engine
        .settle_reflection("eng", "a.md", std::slice::from_ref(&id), "kept", &agent())
        .await
        .unwrap();
    assert!(
        feedback_changes(&file(&fx, "a.md"), &file(&fx, ".reflected/a.md"))
            .iter()
            .all(|c| c.id.as_deref() != Some(id.as_str()))
    );
    let prov = crystalline_core::reflection::read_provenance(&file(&fx, "a.md"));
    assert_eq!(prov[&id].sources.len(), 1, "sources stay");
    assert!(prov[&id].changed_by.is_empty());
}

#[tokio::test]
async fn a_verb_on_a_non_home_instance_names_the_home() {
    let fx = fixture(&[(
        "MANIFEST.md",
        &manifest("reflection: on\nreflection_home: https://team.example.com\n"),
    )])
    .await;
    let err = fx
        .engine
        .read_reflection_tree("eng", &person())
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "this domain's Reflection lives at https://team.example.com"
    );
}

/// A settled top on a page with no frontmatter carries the hash of its text
/// in the file as stored, after the frontmatter the bookkeeping block
/// brings: the write and the settle stamp it the same way.
#[tokio::test]
async fn a_settled_top_is_reflected_as_stored() {
    use crystalline_core::reflection as r;
    let fx = reflecting().await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(
        fx.root.join("reflections/a.md"),
        "# A\n\nIntro.\n\n## Part\n<!-- r:aaaa1111 -->\n\nText.\n",
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    fx.engine
        .settle_reflection(
            "eng",
            "a.md",
            &["_top".to_string(), "aaaa1111".to_string()],
            "kept",
            &agent(),
        )
        .await
        .unwrap();
    let text = file(&fx, "a.md");
    let prov = r::read_provenance(&text);
    for section in r::split_sections(&text) {
        let id = section.id.clone().unwrap();
        assert_eq!(
            prov[&id].reflected,
            Some(r::section_hash(&section.text)),
            "{id}: {text}"
        );
    }
}

/// Rule 8 respells a section the writer did not touch as a link rewrite, not
/// as the writer's change: a curated section whose stored text spells an
/// alias is written canonically, its baseline follows, it records nobody
/// and owes no feedback.
#[tokio::test]
async fn a_respelled_section_the_writer_left_alone_owes_no_feedback() {
    use crystalline_core::reflection as r;
    let fx = crate::fixture::fixture_with(
        &[
            ("MANIFEST.md", &manifest("reflection: on\n")),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
        |cfg, _| {
            cfg.domains.get_mut("eng").unwrap().aliases = vec!["engineering".to_string()];
        },
    )
    .await;
    let mut p = write(
        "a.md",
        "# A\n\n## Night deploys\n\nSee [[eng:runbooks/deploy]].\n\n## Rollback\n\nRun the script.\n",
    );
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    // The stored page and its baseline spell the alias, in sync: the state a
    // page curated before the alias was declared is in.
    let aliased = r::relink(&file(&fx, "a.md"), &file(&fx, ".reflected/a.md"), &|s| {
        (
            s.replace("[[eng:", "[[engineering:"),
            s.matches("[[eng:").count(),
        )
    })
    .unwrap();
    std::fs::write(fx.root.join("reflections/a.md"), &aliased.page).unwrap();
    std::fs::write(
        fx.root.join("reflections/.reflected/a.md"),
        &aliased.baseline,
    )
    .unwrap();
    let night = r::split_sections(&aliased.page)[1].id.clone().unwrap();

    let op = WriteReflectionParams {
        domain: "eng".into(),
        path: "a.md".into(),
        operation: Some("replace_section".into()),
        section: Some("## Rollback".into()),
        content: Some("Run the script, then check the queue.".into()),
        ..Default::default()
    };
    fx.engine.write_reflection(&op, &person()).await.unwrap();

    let text = file(&fx, "a.md");
    let baseline = file(&fx, ".reflected/a.md");
    assert!(
        text.contains("[[eng:runbooks/deploy]]") && baseline.contains("[[eng:runbooks/deploy]]"),
        "{text}\n{baseline}"
    );
    let changes = r::feedback_changes(&text, &baseline);
    assert!(
        changes
            .iter()
            .all(|c| c.id.as_deref() != Some(night.as_str())),
        "{changes:?}"
    );
    let owed: Vec<&str> = changes.iter().map(|c| c.heading.as_str()).collect();
    assert_eq!(
        owed,
        ["Rollback"],
        "only the person's change is owed, the respell owes nothing: {changes:?}"
    );
    let prov = r::read_provenance(&text);
    assert!(
        prov[&night].changed_by.is_empty(),
        "nobody changed the untouched section"
    );
}

/// After a move landed, a page whose links cannot be rewritten is named in
/// the receipt, and the move is not reported as failed.
#[tokio::test]
async fn a_page_that_cannot_be_relinked_is_named_in_the_move_receipt() {
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", PAGE), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("home.md", "# Home\n\nSee [a](a.md).\n"), &person())
        .await
        .unwrap();
    // Not UTF-8 any more: the relink cannot read it.
    std::fs::write(
        fx.root.join("reflections/home.md"),
        b"# Home\n\xff\xfe [a](a.md)\n",
    )
    .unwrap();
    let moved = fx
        .engine
        .move_reflection("eng", "a.md", "b.md", &person())
        .await
        .unwrap();
    assert!(fx.root.join("reflections/b.md").exists(), "the move landed");
    assert_eq!(moved["links_rewritten"], 0, "{moved}");
    assert_eq!(moved["links_failed"][0]["path"], "home.md", "{moved}");
    assert_eq!(moved["links_failed"][0]["domain"], "eng", "{moved}");
}

/// A reflection's row and its chunks commit together: a write whose index
/// step fails after the row is in leaves the row as it was, so the page's
/// new text is unstamped and the next walk reads and chunks it.
#[tokio::test]
async fn a_failed_reflection_index_leaves_the_page_for_the_next_walk() {
    a_failed_index_leaves_the_page(reflecting().await).await;
}

/// The same on Postgres, where the row and the chunks are separate
/// autocommitted statements unless the write's transaction holds them.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_failed_reflection_index_leaves_the_page_for_the_next_walk_on_postgres() {
    use std::sync::Arc;

    use crate::fixture::{fixture_on, scratch_schema};

    let Some(scratch) = scratch_schema() else {
        return;
    };
    let store = crystalline_index::PostgresStore::open_in_schema(scratch.url(), scratch.name())
        .await
        .expect("open the postgres test schema");
    let fx = fixture_on(
        Arc::new(tokio::sync::Mutex::new(store)),
        &[
            ("MANIFEST.md", &manifest("reflection: on\n")),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
        |_, _| {},
        None,
        None,
        None,
    )
    .await;
    a_failed_index_leaves_the_page(fx).await;
}

async fn a_failed_index_leaves_the_page(fx: Fx) {
    fx.engine
        .write_reflection(&write("a.md", "# A\n\nfirst text\n"), &person())
        .await
        .unwrap();
    let store = fx.engine.store();
    let did = store.lock().await.domain_id("eng").await.unwrap().unwrap();
    let before = store
        .lock()
        .await
        .get_reflection(did, "a.md")
        .await
        .unwrap()
        .unwrap();

    fx.engine.fail_next_reflection_index();
    let err = fx
        .engine
        .write_reflection(
            &write("a.md", "# A\n\nthe second, longer text\n"),
            &person(),
        )
        .await;
    assert!(err.is_err(), "{err:?}");
    let after = store
        .lock()
        .await
        .get_reflection(did, "a.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after, before,
        "the failed write left the row as it was, stamp included"
    );

    fx.engine.sync(None).await.unwrap();
    let healed = store
        .lock()
        .await
        .get_reflection(did, "a.md")
        .await
        .unwrap()
        .unwrap();
    assert!(healed.content.contains("second, longer"), "{healed:?}");
    let chunks = store
        .lock()
        .await
        .chunks_needing_embedding_in(
            crystalline_index::ChunkTable::Reflection,
            "m",
            None,
            10,
            None,
        )
        .await
        .unwrap();
    assert!(
        chunks.iter().any(|c| c.text.contains("second, longer")),
        "the walk chunked the new text: {chunks:?}"
    );
}

/// What the bus heard of the Reflection, in order: kind, path, from.
pub fn reflection_events(
    rx: &mut tokio::sync::broadcast::Receiver<crystalline_service::changes::Envelope>,
) -> Vec<(
    crystalline_service::changes::ReflectionChangeKind,
    String,
    Option<String>,
)> {
    let mut heard = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let crystalline_service::changes::Change::Reflection(change) = envelope.change {
            heard.push((change.kind, change.path, change.from));
        }
    }
    heard
}

#[tokio::test]
async fn every_reflection_verb_announces_a_reflection_event() {
    use crystalline_service::changes::ReflectionChangeKind as K;
    let fx = reflecting().await;
    let mut rx = fx.engine.changes().subscribe();
    fx.engine
        .write_reflection(&write("a.md", PAGE), &person())
        .await
        .unwrap();
    let opened = fx
        .engine
        .discuss(
            &crystalline_service::params::DiscussParams {
                domain: "eng".into(),
                action: Some("open".into()),
                path: Some("a.md".into()),
                section: Some("Night deploys".into()),
                text: Some("Still true?".into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
    let thread = opened["id"].as_str().unwrap().to_string();
    fx.engine
        .reflection_thread_entry(
            "eng",
            "a.md",
            &thread,
            EntryKind::Resolve,
            "Yes.",
            &person(),
        )
        .await
        .unwrap();
    let id = crystalline_core::reflection::split_sections(&file(&fx, "a.md"))[1]
        .id
        .clone()
        .unwrap();
    fx.engine
        .settle_reflection("eng", "a.md", &[id], "kept", &agent())
        .await
        .unwrap();
    fx.engine
        .move_reflection("eng", "a.md", "b.md", &person())
        .await
        .unwrap();
    fx.engine
        .delete_reflection("eng", "b.md", &person())
        .await
        .unwrap();
    let a = || "a.md".to_string();
    assert_eq!(
        reflection_events(&mut rx),
        vec![
            (K::Changed, a(), None),
            (K::Threads, a(), None),
            (K::Threads, a(), None),
            (K::Changed, a(), None),
            (K::Moved, "b.md".to_string(), Some(a())),
            (K::Deleted, "b.md".to_string(), None),
        ]
    );
}

#[tokio::test]
async fn a_move_announces_the_pages_whose_links_it_rewrote() {
    use crystalline_service::changes::ReflectionChangeKind as K;
    let fx = reflecting().await;
    fx.engine
        .write_reflection(&write("a.md", PAGE), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("links.md", "# Links\n\nSee [a](a.md).\n"), &person())
        .await
        .unwrap();
    let mut rx = fx.engine.changes().subscribe();
    fx.engine
        .move_reflection("eng", "a.md", "b.md", &person())
        .await
        .unwrap();
    let heard = reflection_events(&mut rx);
    assert!(
        heard.contains(&(K::Moved, "b.md".to_string(), Some("a.md".to_string()))),
        "{heard:?}"
    );
    assert!(
        heard.contains(&(K::Changed, "links.md".to_string(), None)),
        "{heard:?}"
    );
}

/// A reflection receipt names an unknown callout kind, and never raises Q008
/// (an observation inside a callout is only a finding for an engram).
#[tokio::test]
async fn a_reflection_receipt_names_an_unknown_callout_kind_but_never_q008() {
    let fx = reflecting().await;
    let page = "# Deploying\n\n> [!DANGER]\n> Careful.\n> - [fact] x\n";
    let written = fx
        .engine
        .write_reflection(&write("guides/danger.md", page), &person())
        .await
        .unwrap();
    let codes: Vec<&str> = written["markdown_findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["Q007"], "{written}");
    // The line is the one the saved page holds it on, ids and all.
    let saved = file(&fx, "guides/danger.md");
    let line = saved.lines().position(|l| l.contains("[!DANGER]")).unwrap() + 1;
    assert_eq!(written["markdown_findings"][0]["line"], line, "{written}");

    let clean = fx
        .engine
        .write_reflection(&write("guides/clean.md", PAGE), &person())
        .await
        .unwrap();
    assert!(clean.get("markdown_findings").is_none(), "{clean}");
}

/// Crystalline gives a heading its id: an id line no heading can carry (not
/// hex, a blank line away from its heading, or one another page of the
/// domain holds) is replaced and the receipt says so, and sources keyed by
/// the replaced id still find their section. An id the page had stays.
#[tokio::test]
async fn an_id_line_no_heading_can_carry_is_replaced_and_said() {
    let fx = reflecting().await;
    let other = fx
        .engine
        .write_reflection(
            &write("other.md", "# O\n\n## Elsewhere\n\nText.\n"),
            &agent(),
        )
        .await
        .unwrap();
    let foreign = other["ids_assigned"][0].as_str().unwrap().to_string();
    let markdown = format!(
        "# Deploying\n\n## Night deploys\n\n<!-- r:t2u3v4w5 -->\n\nDeploys run at night.\n\n## Rollback\n\n<!-- r:a1b2c3d4 -->\n\nRoll back.\n\n## Elsewhere\n<!-- r:{foreign} -->\n\nCopied.\n"
    );
    let mut p = write("guides/deploying.md", &markdown);
    p.sources = BTreeMap::from([("a1b2c3d4".to_string(), vec!["runbooks/deploy".to_string()])]);
    let written = fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert_eq!(written["curated"], true, "{written}");
    let text = file(&fx, "guides/deploying.md");
    assert!(
        !text.contains("t2u3v4w5") && !text.contains("a1b2c3d4") && !text.contains(&foreign),
        "{text}"
    );
    let sections = crystalline_core::reflection::split_sections(&text);
    let ids: Vec<String> = sections[1..]
        .iter()
        .map(|s| s.id.clone().unwrap())
        .collect();
    assert_eq!(written["ids_assigned"], serde_json::json!(ids), "{written}");
    assert_eq!(
        sections[1].text, "## Night deploys\n\nDeploys run at night.\n",
        "no stray line and no extra blank line"
    );
    let prov = crystalline_core::reflection::read_provenance(&text);
    assert_eq!(prov[&ids[1]].sources[0].engram, "runbooks/deploy");
    let guidance = written["guidance"].as_str().unwrap();
    for (made_up, id) in [
        ("t2u3v4w5", &ids[0]),
        ("a1b2c3d4", &ids[1]),
        (&foreign, &ids[2]),
    ] {
        assert!(
            guidance.contains(&format!("{made_up} (now {id})")),
            "{guidance}"
        );
    }
    assert!(guidance.contains("never write one yourself"), "{guidance}");

    // The page's own ids stay when it is sent back, the copied one now too.
    let again = fx
        .engine
        .write_reflection(
            &write(
                "guides/deploying.md",
                &text.replace("Copied.", "Copied, then kept."),
            ),
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(again["ids_assigned"], serde_json::json!([]), "{again}");
    // Nothing is said about ids; the one section it changed names no source.
    assert_eq!(
        again["guidance"],
        "These sections name no source engrams: pass sources for each section you built from engrams, so the queue can follow them.",
        "{again}"
    );
    assert_eq!(
        again["unsourced_sections"],
        serde_json::json!(["Elsewhere"]),
        "{again}"
    );
}

/// The page sent back as it was read, block and ids included, is told
/// nothing: the receipt carries no guidance.
#[tokio::test]
async fn a_page_sent_back_as_read_is_told_nothing() {
    let fx = reflecting().await;
    let mut p = write("guides/deploying.md", PAGE);
    p.sources = BTreeMap::from([(
        "Night deploys".to_string(),
        vec!["runbooks/deploy".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let read = file(&fx, "guides/deploying.md");
    assert!(read.contains("reflection:"), "{read}");
    let mut again = write(
        "guides/deploying.md",
        &read.replace("Deploys run at night.", "Deploys run at night, late."),
    );
    again.sources = p.sources.clone();
    let written = fx.engine.write_reflection(&again, &agent()).await.unwrap();
    assert_eq!(written["ids_assigned"], serde_json::json!([]), "{written}");
    assert!(written.get("guidance").is_none(), "{written}");
}

/// A `reflection:` key the writer made up is Crystalline's bookkeeping: it is
/// replaced and the receipt says so, beside the note on a made-up id line.
#[tokio::test]
async fn a_bookkeeping_block_the_writer_made_up_is_replaced_and_said() {
    let fx = reflecting().await;
    let markdown = "---\nreflection:\n  for: new operators\n---\n\n# Deploying\n\n## Night deploys\n<!-- r:s1t2u3v4 -->\n\nDeploys run at night.\n";
    let written = fx
        .engine
        .write_reflection(&write("guides/deploying.md", markdown), &agent())
        .await
        .unwrap();
    let text = file(&fx, "guides/deploying.md");
    assert!(!text.contains("new operators"), "{text}");
    let guidance = written["guidance"].as_str().unwrap();
    assert!(
        guidance.contains("reflection: frontmatter key is Crystalline's bookkeeping")
            && guidance.contains("pass sources")
            && guidance.contains("s1t2u3v4"),
        "both notes: {guidance}"
    );
}

/// The path is inside reflections/ already: a new page whose path repeats
/// the folder is refused with the corrected path, and one that sits there
/// from before can still be moved out.
#[tokio::test]
async fn a_path_that_repeats_the_reflections_folder_is_refused() {
    let fx = reflecting().await;
    let err = fx
        .engine
        .write_reflection(&write("reflections/guides/a.md", PAGE), &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("drop the reflections/ prefix") && err.contains("'guides/a.md'"),
        "{err}"
    );
    assert!(!fx.root.join("reflections/reflections").exists());

    std::fs::create_dir_all(fx.root.join("reflections/reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/reflections/old.md"), PAGE).unwrap();
    fx.engine.sync(None).await.unwrap();
    let err = fx
        .engine
        .move_reflection("eng", "guides/x.md", "reflections/x.md", &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("drop the reflections/ prefix"), "{err}");
    fx.engine
        .write_reflection(
            &write("reflections/old.md", &PAGE.replace("night", "dawn")),
            &agent(),
        )
        .await
        .expect("a page already there can be edited");
    fx.engine
        .move_reflection("eng", "reflections/old.md", "old.md", &person())
        .await
        .expect("and moved out");
    assert!(fx.root.join("reflections/old.md").exists());
}

/// A page's own id one blank line below its heading is seated back under
/// its heading: no id is minted, nothing is said and the section does not
/// read as changed.
#[tokio::test]
async fn a_pages_own_id_a_blank_line_down_is_seated_back() {
    let fx = reflecting().await;
    let first = fx
        .engine
        .write_reflection(&write("a.md", PAGE), &agent())
        .await
        .unwrap();
    let id = first["ids_assigned"][0].as_str().unwrap().to_string();
    let read = file(&fx, "a.md");
    let loose = read.replace(
        &format!("## Night deploys\n<!-- r:{id} -->\n"),
        &format!("## Night deploys\n\n<!-- r:{id} -->\n"),
    );
    assert_ne!(loose, read);
    let written = fx
        .engine
        .write_reflection(&write("a.md", &loose), &agent())
        .await
        .unwrap();
    assert_eq!(written["ids_assigned"], serde_json::json!([]), "{written}");
    assert!(written.get("guidance").is_none(), "{written}");
    assert_eq!(file(&fx, "a.md"), read, "seated back, nothing else moved");
}

/// A domain rename rewrites the stored domain of every source of the
/// renamed domain, also on a page whose text never names it, so a rename
/// never turns those sources into ones that cannot be checked. The
/// rewrite owes no feedback.
#[tokio::test]
async fn a_domain_rename_rewrites_the_domain_of_sources_it_names() {
    use crystalline_core::reflection as r;
    let fx = with_ops(
        "",
        "",
        &[(
            "runbook.md",
            &engram("Runbook", "runbook", "- [fact] page the on-call"),
        )],
    )
    .await;
    let mut p = write("a.md", "# A\n\nFor people.\n\n## On call\n\nPage them.\n");
    p.sources = BTreeMap::from([(
        "On call".to_string(),
        vec!["crystalline://ops/runbook".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert!(file(&fx, "a.md").contains("domain: ops"));

    rename_ops(&fx).await;

    let text = file(&fx, "a.md");
    assert!(text.contains("domain: platform"), "{text}");
    assert!(!text.contains("domain: ops"), "{text}");
    assert!(
        r::feedback_changes(&text, &file(&fx, ".reflected/a.md")).is_empty(),
        "the rename owes no feedback"
    );
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    let source = &doc["sections"][0]["sources"][0];
    assert_eq!(source["domain"], "platform", "{doc}");
    assert_eq!(source["state"], "current", "{doc}");
}

/// A rename of this machine's name only keeps every former name as an
/// alias, so a stored source domain still resolves and reads current.
#[tokio::test]
async fn a_local_rename_keeps_the_sources_of_the_renamed_domain_current() {
    let fx = with_ops(
        "",
        "",
        &[(
            "runbook.md",
            &engram("Runbook", "runbook", "- [fact] page the on-call"),
        )],
    )
    .await;
    let mut p = write("a.md", "# A\n\nFor people.\n\n## On call\n\nPage them.\n");
    p.sources = BTreeMap::from([(
        "On call".to_string(),
        vec!["crystalline://ops/runbook".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    fx.engine
        .rename_domain(
            "ops",
            "platform",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        doc["sections"][0]["sources"][0]["state"], "current",
        "{doc}"
    );
}

/// A canonical name changed by editing the MANIFEST on disk, outside a
/// rename and before any adoption lines it up, leaves the stored name
/// resolving nowhere: its sources read unavailable, never missing, and stay
/// on the page. (A MANIFEST write through Crystalline lines the name up in
/// that write, keeping the old canonical name as an alias.)
#[tokio::test]
async fn a_canonical_name_edited_in_the_manifest_leaves_sources_unavailable() {
    let fx = with_ops(
        "",
        "",
        &[(
            "runbook.md",
            &engram("Runbook", "runbook", "- [fact] page the on-call"),
        )],
    )
    .await;
    let rename_in_manifest = |from: &str, to: &str| crystalline_service::params::EditParams {
        identifier: "manifest".to_string(),
        domain: "ops".to_string(),
        operation: "find_replace".to_string(),
        find_text: Some(format!("domain_name: {from}\n")),
        content: Some(format!("domain_name: {to}\n")),
        expected_replacements: Some(1),
        ..Default::default()
    };
    fx.engine
        .edit_engram(&rename_in_manifest("ops", "ops-team"))
        .await
        .unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut p = write("a.md", "# A\n\nFor people.\n\n## On call\n\nPage them.\n");
    p.sources = BTreeMap::from([(
        "On call".to_string(),
        vec!["crystalline://ops/runbook".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert!(
        file(&fx, "a.md").contains("domain: ops-team"),
        "the canonical name: {}",
        file(&fx, "a.md")
    );
    let manifest_path = fx.tmp.path().join("ops/MANIFEST.md");
    let on_disk = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(
        &manifest_path,
        on_disk.replacen("domain_name: ops-team\n", "domain_name: ops-crew\n", 1),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        doc["sections"][0]["sources"][0]["state"], "unavailable",
        "{doc}"
    );
    let queue = fx
        .engine
        .reflect(
            &crystalline_service::params::ReflectParams {
                domains: vec!["eng".into()],
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let whats: Vec<&str> = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["what"].as_str().unwrap())
        .collect();
    assert!(whats.contains(&"source_unavailable"), "{queue}");
    assert!(!whats.contains(&"source_missing"), "{queue}");
    assert!(file(&fx, "a.md").contains("domain: ops-team"), "kept");
}

/// A canonical name changed through Crystalline (an edit of the MANIFEST)
/// is lined up in that write: the old canonical name stays an alias at once,
/// so a source stored under it still resolves and reads current, with no
/// sync or adoption in between.
#[tokio::test]
async fn a_canonical_name_edited_through_crystalline_keeps_sources_current() {
    let fx = with_ops(
        "",
        "",
        &[(
            "runbook.md",
            &engram("Runbook", "runbook", "- [fact] page the on-call"),
        )],
    )
    .await;
    let rename_in_manifest = |from: &str, to: &str| crystalline_service::params::EditParams {
        identifier: "manifest".to_string(),
        domain: "ops".to_string(),
        operation: "find_replace".to_string(),
        find_text: Some(format!("domain_name: {from}\n")),
        content: Some(format!("domain_name: {to}\n")),
        expected_replacements: Some(1),
        ..Default::default()
    };
    fx.engine
        .edit_engram(&rename_in_manifest("ops", "ops-team"))
        .await
        .unwrap();
    fx.engine.sync(None).await.unwrap();
    let mut p = write("a.md", "# A\n\nFor people.\n\n## On call\n\nPage them.\n");
    p.sources = BTreeMap::from([(
        "On call".to_string(),
        vec!["crystalline://ops/runbook".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    assert!(
        file(&fx, "a.md").contains("domain: ops-team"),
        "the canonical name: {}",
        file(&fx, "a.md")
    );

    fx.engine
        .edit_engram(&rename_in_manifest("ops-team", "ops-crew"))
        .await
        .unwrap();
    assert_eq!(
        fx.engine.local_domain_name("ops-team").await.as_deref(),
        Some("ops"),
        "the old canonical name is an alias at once"
    );
    let doc = fx
        .engine
        .read_reflection("eng", "a.md", &agent())
        .await
        .unwrap();
    assert_eq!(
        doc["sections"][0]["sources"][0]["state"], "current",
        "{doc}"
    );
    assert!(file(&fx, "a.md").contains("domain: ops-team"), "kept");
}

/// A page whose frontmatter cannot be rewritten key by key keeps its
/// stored source domain through a rename, and the report lists it with
/// why, instead of passing it over in silence.
#[tokio::test]
async fn a_rename_lists_a_page_whose_sources_it_cannot_rewrite() {
    use crystalline_core::reflection as r;
    let fx = with_ops(
        "",
        "",
        &[(
            "runbook.md",
            &engram("Runbook", "runbook", "- [fact] page the on-call"),
        )],
    )
    .await;
    let mut p = write("a.md", "# A\n\nFor people.\n\n## On call\n\nPage them.\n");
    p.sources = BTreeMap::from([(
        "On call".to_string(),
        vec!["crystalline://ops/runbook".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let text = file(&fx, "a.md");
    let (id, record) = r::read_provenance(&text)
        .into_iter()
        .find(|(_, rec)| !rec.sources.is_empty())
        .unwrap();
    let body = crystalline_core::frontmatter::split_frontmatter(&text)
        .unwrap()
        .body
        .to_string();
    let flow = format!(
        "---\n{{reflection: {{{id}: {{sources: [{{domain: ops, engram: runbook, checksum: {}}}]}}}}}}\n---\n{body}",
        record.sources[0].checksum
    );
    std::fs::write(fx.root.join("reflections/a.md"), &flow).unwrap();
    fx.engine.sync(None).await.unwrap();

    let report = rename_ops(&fx).await;

    assert!(file(&fx, "a.md").contains("domain: ops"), "left as it was");
    let left = report["left_behind"].as_array().unwrap();
    assert!(
        left.iter().any(|row| row["domain"] == "eng"
            && row["path"] == "reflections/a.md"
            && row["reason"]
                .as_str()
                .is_some_and(|why| why.contains("cannot be rewritten key by key"))),
        "{report}"
    );
}

/// A domain rename by an account that cannot see the citing page's domain
/// still rewrites the stored source domain there, and names nothing of
/// that domain in its report.
#[tokio::test]
async fn a_rename_rewrites_sources_in_a_domain_hidden_from_the_renamer() {
    use crystalline_service::rest::{AuthStore, Role};
    use std::sync::Arc;
    let fx = with_ops(
        "",
        "",
        &[(
            "runbook.md",
            &engram("Runbook", "runbook", "- [fact] page the on-call"),
        )],
    )
    .await;
    let mut p = write("a.md", "# A\n\nFor people.\n\n## On call\n\nPage them.\n");
    p.sources = BTreeMap::from([(
        "On call".to_string(),
        vec!["crystalline://ops/runbook".to_string()],
    )]);
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "olga"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("eng", true, "keeper")
        .await
        .unwrap();
    auth.set_domain_visibility("ops", true, "olga")
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(crystalline_service::DomainAccess::new(auth)));
    let olga = crystalline_service::Scope::User {
        account: "olga".into(),
        admin: false,
    };
    let report = fx
        .engine
        .rename_domain("ops", "platform", false, &olga)
        .await
        .unwrap();
    assert!(!report.to_string().contains("\"eng\""), "{report}");
    let text = file(&fx, "a.md");
    assert!(text.contains("domain: platform"), "{text}");
}
