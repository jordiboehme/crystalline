//! What a Reflection page says about itself beside its text, as a read
//! answers it (design 2.1 to 2.4 and 4): its settings, the pages and pacts
//! it follows, its owner and its retirement; the tree and search mark a
//! retired page; an owned page's clock is its owner's; and a curator
//! changes the settings without writing the page.

use crystalline_service::engine::ReflectionSearch;
use crystalline_service::params::{PageSettingsChange, WriteReflectionParams};
use crystalline_service::{EngineError, ReflectionCaller};
use serde_json::{Value, json};

use crate::docs::{agent, person, reflecting, write};
use crate::fixture::Fx;
use crate::page_rights::editor_and_curator;

const PAGE: &str = "# A\n\n## One\n\nFirst.\n";

async fn put(fx: &Fx, path: &str, text: &str) {
    fx.engine
        .write_reflection(&write(path, text), &person())
        .await
        .unwrap();
}

async fn owned(fx: &Fx, path: &str, owner: &str, text: &str) {
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                owner: Some(owner.into()),
                ..write(path, text)
            },
            &agent(),
        )
        .await
        .unwrap();
}

async fn act(fx: &Fx, action: &str, path: &str) {
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                action: Some(action.into()),
                path: path.into(),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
}

async fn page_of(fx: &Fx, path: &str) -> Value {
    fx.engine
        .read_reflection("eng", path, &person())
        .await
        .unwrap()["page"]
        .clone()
}

fn file(fx: &Fx, rel: &str) -> Option<String> {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).ok()
}

/// A page read names the page's settings, the pages it follows from the
/// top down with their titles, the pact files that apply in order and its
/// own pact file, and what did not read. PACT.md and a pact file are no
/// pages and carry none of it.
#[tokio::test]
async fn a_page_read_names_its_settings_its_chain_and_its_pact() {
    let fx = reflecting().await;
    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nShort.\n").await;
    put(&fx, "arch.md", "---\nfreshness: 1d\n---\n# Architecture\n").await;
    put(
        &fx,
        "arch.pact.md",
        "# Pact of arch\n\n## Tone\n\nShorter.\n",
    )
    .await;
    put(&fx, "arch/overview.md", "# Overview\n").await;
    put(
        &fx,
        "notes.md",
        "---\nfollows: arch/overview.md\nstatus: draft\nfreshness: 0h\n---\n# Notes\n",
    )
    .await;

    let arch = page_of(&fx, "arch.md").await;
    assert_eq!(arch["freshness"], "1d", "{arch}");
    assert_eq!(arch["follows"], Value::Null, "{arch}");
    assert_eq!(arch["chain"], json!([]), "{arch}");
    assert_eq!(arch["pacts"], json!(["PACT.md", "arch.pact.md"]), "{arch}");
    assert_eq!(
        arch["pact"],
        json!({ "path": "arch.pact.md", "exists": true }),
        "{arch}"
    );
    assert_eq!(arch["owner"], Value::Null, "{arch}");
    assert_eq!(arch["retired"], Value::Null, "{arch}");
    assert_eq!(arch["findings"], json!([]), "{arch}");

    // The folder parent is followed.
    let overview = page_of(&fx, "arch/overview.md").await;
    assert_eq!(
        overview["chain"],
        json!([{ "path": "arch.md", "title": "Architecture" }]),
        "{overview}"
    );
    assert_eq!(overview["pacts"], json!(["PACT.md", "arch.pact.md"]));
    assert_eq!(
        overview["pact"],
        json!({ "path": "arch/overview.pact.md", "exists": false })
    );

    // An explicit follows, a status as written, and a freshness that does
    // not read is a finding and no freshness.
    let notes = page_of(&fx, "notes.md").await;
    assert_eq!(notes["follows"], "arch/overview.md", "{notes}");
    assert_eq!(notes["status"], "draft", "{notes}");
    assert_eq!(notes["freshness"], Value::Null, "{notes}");
    assert_eq!(
        notes["chain"],
        json!([
            { "path": "arch.md", "title": "Architecture" },
            { "path": "arch/overview.md", "title": "Overview" },
        ]),
        "{notes}"
    );
    let findings = notes["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{notes}");
    assert_eq!(findings[0]["key"], "freshness");
    assert_eq!(findings[0]["value"], "0h");
    assert!(
        findings[0]["reason"]
            .as_str()
            .unwrap()
            .contains("shorter than the minimum"),
        "{notes}"
    );

    for pact in ["PACT.md", "arch.pact.md"] {
        let read = fx
            .engine
            .read_reflection("eng", pact, &person())
            .await
            .unwrap();
        assert!(read.get("page").is_none(), "{pact}: {read}");
    }
}

/// An owned page names its owner with its title; a retired page names
/// when and by whom, and the page that owned it; an owner that is gone
/// is named as missing.
#[tokio::test]
async fn an_owned_and_a_retired_page_name_their_owner() {
    let fx = reflecting().await;
    put(&fx, "arch.md", "---\nfreshness: 1d\n---\n# Architecture\n").await;
    owned(&fx, "arch/a.md", "arch.md", PAGE).await;
    owned(&fx, "arch/b.md", "arch.md", PAGE).await;

    let a = page_of(&fx, "arch/a.md").await;
    assert_eq!(
        a["owner"],
        json!({ "path": "arch.md", "title": "Architecture", "exists": true }),
        "{a}"
    );
    // An owned page follows its owner.
    assert_eq!(
        a["chain"],
        json!([{ "path": "arch.md", "title": "Architecture" }])
    );

    act(&fx, "retire", "arch/a.md").await;
    let a = page_of(&fx, "arch/a.md").await;
    assert_eq!(a["owner"], Value::Null, "{a}");
    assert_eq!(a["status"], "archived", "{a}");
    assert_eq!(a["retired"]["by"], "jordi", "{a}");
    assert!(a["retired"]["at"].as_str().unwrap().ends_with('Z'), "{a}");
    assert_eq!(
        a["retired"]["was_owned_by"],
        json!({ "path": "arch.md", "title": "Architecture", "exists": true }),
        "{a}"
    );

    // The owner goes: the record still names it, as missing.
    std::fs::remove_file(fx.root.join("reflections/arch.md")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let b = page_of(&fx, "arch/b.md").await;
    assert_eq!(
        b["owner"],
        json!({ "path": "arch.md", "title": "arch.md", "exists": false }),
        "{b}"
    );
}

/// The tree and search mark a retired page, so the navigation can hide it
/// and search can show it faded; the tree names a page's owner.
#[tokio::test]
async fn the_tree_and_search_mark_a_retired_page() {
    let fx = reflecting().await;
    put(
        &fx,
        "arch.md",
        "# Architecture\n\n## Night\n\nNight deploys.\n",
    )
    .await;
    owned(
        &fx,
        "arch/a.md",
        "arch.md",
        "# A\n\n## Night\n\nNight shifts.\n",
    )
    .await;
    put(&fx, "old.md", "# Old\n\n## Night\n\nNight trains.\n").await;
    act(&fx, "retire", "old.md").await;

    let tree = fx
        .engine
        .read_reflection_tree("eng", &person())
        .await
        .unwrap();
    let leaf = |path: &str| -> Value {
        fn find(nodes: &Value, path: &str) -> Option<Value> {
            for node in nodes.as_array()? {
                if node["path"] == path && node["kind"] == "reflection" {
                    return Some(node.clone());
                }
                if let Some(found) = find(&node["children"], path) {
                    return Some(found);
                }
            }
            None
        }
        find(&tree["nodes"], path).unwrap_or_else(|| panic!("{path} in {tree}"))
    };
    assert_eq!(leaf("old.md")["retired"], true);
    assert_eq!(leaf("arch.md")["retired"], false);
    assert_eq!(leaf("arch/a.md")["owner"], "arch.md");
    assert_eq!(leaf("arch.md")["owner"], Value::Null);

    let hits = fx
        .engine
        .search_reflection(
            "eng",
            &ReflectionSearch {
                query: "night",
                search_type: Some("text"),
                limit: 10,
                page: 1,
            },
            &person(),
        )
        .await
        .unwrap();
    let retired: Vec<(String, bool)> = hits["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| {
            (
                hit["path"].as_str().unwrap().to_string(),
                hit["retired"].as_bool().expect("every hit says retired"),
            )
        })
        .collect();
    assert!(retired.contains(&("old.md".to_string(), true)), "{hits}");
    assert!(retired.contains(&("arch.md".to_string(), false)), "{hits}");
}

/// An owned page's clock is its owner's: it is part of its owner, and the
/// first page up the owners that has a freshness keeps it current. With
/// no such page it is only part of its owner.
#[tokio::test]
async fn an_owned_pages_clock_is_its_owners() {
    let fx = reflecting().await;
    put(&fx, "arch.md", "---\nfreshness: 1d\n---\n# Architecture\n").await;
    owned(&fx, "arch/a.md", "arch.md", "# Part A\n").await;
    owned(&fx, "arch/a/x.md", "arch/a.md", "# Part X\n").await;
    put(&fx, "plain.md", "# Plain\n").await;
    owned(&fx, "plain/p.md", "plain.md", "# Part P\n").await;

    let clock = |path: &'static str| {
        let fx = &fx;
        async move {
            fx.engine
                .reflection_page_clock("eng", path, &person())
                .await
                .unwrap()
        }
    };
    let owner = clock("arch.md").await;
    assert_eq!(owner["part_of"], Value::Null, "{owner}");
    assert_eq!(owner["clock_of"], "arch.md", "{owner}");

    for (path, part_of, title) in [
        ("arch/a.md", "arch.md", "Architecture"),
        ("arch/a/x.md", "arch/a.md", "Part A"),
    ] {
        let child = clock(path).await;
        assert_eq!(
            child["part_of"],
            json!({ "path": part_of, "title": title }),
            "{path}: {child}"
        );
        assert_eq!(child["clock_of"], "arch.md", "{path}: {child}");
        assert_eq!(child["freshness"], "1d", "{path}: {child}");
        assert_eq!(
            child["last_reconciled"], owner["last_reconciled"],
            "{path}: {child}"
        );
        // An owned page is never due on its own.
        assert_eq!(child["due_in_secs"], Value::Null, "{path}: {child}");
        assert_eq!(child["stale"], false, "{path}: {child}");
    }

    let p = clock("plain/p.md").await;
    assert_eq!(
        p["part_of"],
        json!({ "path": "plain.md", "title": "Plain" }),
        "{p}"
    );
    assert_eq!(p["clock_of"], Value::Null, "{p}");
    assert_eq!(p["freshness"], Value::Null, "{p}");
    assert_eq!(p["last_reconciled"], Value::Null, "{p}");

    // A page on its own without a freshness keeps no clock.
    assert_eq!(clock("plain.md").await, Value::Null);
}

fn change(path: &str) -> PageSettingsChange {
    PageSettingsChange {
        path: path.into(),
        ..Default::default()
    }
}

/// A curator sets, keeps and clears a page's settings without writing its
/// text: an absent key stays, an empty value clears it, a value sets it.
/// Only the
/// frontmatter changes.
#[tokio::test]
async fn a_curator_changes_the_settings_and_nothing_else() {
    let fx = reflecting().await;
    put(&fx, "arch.md", "# Architecture\n").await;
    put(
        &fx,
        "notes.md",
        "---\ntags: [a]\n---\n# Notes\n\n## One\n\nText.\n",
    )
    .await;

    let receipt = fx
        .engine
        .set_reflection_page_settings(
            "eng",
            &PageSettingsChange {
                freshness: Some("6h".into()),
                follows: Some("arch.md".into()),
                status: Some("draft".into()),
                ..change("notes.md")
            },
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(receipt["page"]["freshness"], "6h", "{receipt}");
    assert_eq!(receipt["page"]["follows"], "arch.md", "{receipt}");
    assert_eq!(receipt["page"]["status"], "draft", "{receipt}");
    let text = file(&fx, "notes.md").unwrap();
    assert!(text.contains("tags: [a]"), "{text}");
    assert!(text.contains("## One\n<!-- r:"), "{text}");
    assert!(text.ends_with("Text.\n"), "{text}");

    // Absent keys stay; an empty value clears.
    fx.engine
        .set_reflection_page_settings(
            "eng",
            &PageSettingsChange {
                freshness: Some(String::new()),
                ..change("notes.md")
            },
            &person(),
        )
        .await
        .unwrap();
    let page = page_of(&fx, "notes.md").await;
    assert_eq!(page["freshness"], Value::Null, "{page}");
    assert_eq!(page["follows"], "arch.md", "{page}");
    assert_eq!(page["status"], "draft", "{page}");
    let text = file(&fx, "notes.md").unwrap();
    assert!(!text.contains("freshness"), "{text}");
}

/// What a settings change refuses: a freshness that does not read or is
/// under an hour, a freshness on an owned page, a follows that names no
/// page or closes a cycle, a retired word in status (retire instead), a
/// status change on a retired page (restore instead), a pact file, a
/// missing page, and anyone below curate.
#[tokio::test]
async fn a_settings_change_refuses_what_a_page_cannot_take() {
    let (fx, ann, curator) = editor_and_curator().await;
    for (path, text) in [
        ("arch.md", "# Architecture\n"),
        ("notes.md", "# Notes\n"),
        ("old.md", "# Old\n"),
        ("arch.pact.md", "# Pact\n"),
    ] {
        fx.engine
            .write_reflection(&write(path, text), &curator)
            .await
            .unwrap();
    }
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                owner: Some("arch.md".into()),
                ..write("arch/a.md", "# A\n")
            },
            &curator,
        )
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                action: Some("retire".into()),
                path: "old.md".into(),
                ..Default::default()
            },
            &curator,
        )
        .await
        .unwrap();
    let before = |path: &str| file(&fx, path);
    let snapshot: Vec<Option<String>> = ["arch.md", "notes.md", "old.md", "arch/a.md"]
        .iter()
        .map(|p| before(p))
        .collect();

    let refused = |c: PageSettingsChange, who: ReflectionCaller| {
        let fx = &fx;
        async move {
            fx.engine
                .set_reflection_page_settings("eng", &c, &who)
                .await
                .unwrap_err()
        }
    };
    let says = |e: &EngineError, needle: &str| {
        assert!(e.to_string().contains(needle), "{needle}: {e}");
    };

    let e = refused(
        PageSettingsChange {
            freshness: Some("0h".into()),
            ..change("notes.md")
        },
        curator.clone(),
    )
    .await;
    assert!(matches!(e, EngineError::Invalid(_)), "{e}");
    says(&e, "shorter than the minimum");
    let e = refused(
        PageSettingsChange {
            freshness: Some("soon".into()),
            ..change("notes.md")
        },
        curator.clone(),
    )
    .await;
    says(&e, "is not a duration");
    let e = refused(
        PageSettingsChange {
            freshness: Some("1d".into()),
            ..change("arch/a.md")
        },
        curator.clone(),
    )
    .await;
    says(&e, "owned");
    let e = refused(
        PageSettingsChange {
            follows: Some("gone.md".into()),
            ..change("notes.md")
        },
        curator.clone(),
    )
    .await;
    says(&e, "gone.md");
    let e = refused(
        PageSettingsChange {
            status: Some("Archived".into()),
            ..change("notes.md")
        },
        curator.clone(),
    )
    .await;
    says(&e, "retire");
    let e = refused(
        PageSettingsChange {
            status: Some(String::new()),
            ..change("old.md")
        },
        curator.clone(),
    )
    .await;
    says(&e, "restore");
    let e = refused(
        PageSettingsChange {
            freshness: Some("1d".into()),
            ..change("arch.pact.md")
        },
        curator.clone(),
    )
    .await;
    says(&e, "pact file");
    let e = refused(
        PageSettingsChange {
            freshness: Some("1d".into()),
            ..change("gone.md")
        },
        curator.clone(),
    )
    .await;
    assert!(matches!(e, EngineError::NotFound(_)), "{e}");
    let e = refused(
        PageSettingsChange {
            freshness: Some("1d".into()),
            ..change("notes.md")
        },
        ann.clone(),
    )
    .await;
    assert!(matches!(e, EngineError::Forbidden(_)), "{e}");
    says(&e, "curate");

    let after: Vec<Option<String>> = ["arch.md", "notes.md", "old.md", "arch/a.md"]
        .iter()
        .map(|p| before(p))
        .collect();
    assert_eq!(snapshot, after, "nothing was written");
}
