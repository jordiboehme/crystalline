//! Relative Markdown links between engrams, end to end: a converted collection
//! links its notes by path, and the read receipt, the graph rules and the
//! broken-link rule all see those links as edges.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{EvolveParams, ReadParams};
use serde_json::Value;
use tokio::sync::Mutex;

const TODAY: &str = "2026-08-02";

fn note(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - plant\nstatus: stable\nrecorded_at: 2026-07-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A domain `plant` whose notes link each other by relative path. Every
/// permalink differs from its path, so only the path can bind a link.
async fn fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plant");
    let files = [
        (
            "MANIFEST.md",
            "---\ntype: manifest\ntitle: plant\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\n# plant\n\n## Scope\n\n- The plant\n\n## When to Use\n\n- Route here for the plant\n".to_string(),
        ),
        (
            "overview.md",
            note("Overview", "start-here", "The plant at a glance, pumps and valves."),
        ),
        (
            "pumps/p-12.md",
            note(
                "P-12",
                "kit/pump-twelve",
                "The big pump.\n\n- part_of [Overview](../overview.md)\n\nRead [the overview](../overview.md#top) first.",
            ),
        ),
        (
            "valves/v-3.md",
            note(
                "V-3",
                "kit/valve-three",
                "The valve.\n\nSee [the overview](../overview.md) and [the old plant](../../old/plant.md).",
            ),
        ),
        (
            "lone.md",
            note("Lone", "lone", "Nobody links here and it links nowhere."),
        ),
    ];
    for (rel, body) in files {
        let abs = dir.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
    }
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("plant".to_string(), DomainEntry::file(dir));
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(tmp.path().join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

async fn read(engine: &Engine, identifier: &str) -> Value {
    engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some("plant".to_string()),
                share_link: None,
                include_images: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_engram_reports_a_relative_link_as_resolved() {
    let (_tmp, engine) = fixture().await;
    let v = read(&engine, "kit/pump-twelve").await;

    let relations = v["relations"].as_array().unwrap();
    assert_eq!(relations.len(), 1, "{v}");
    assert_eq!(relations[0]["rel_type"], "part_of");
    assert_eq!(
        relations[0]["target"],
        serde_json::json!({ "domain": null, "target": "../overview.md", "form": "relative" })
    );
    assert_eq!(relations[0]["resolved"], true, "{v}");
    assert_eq!(relations[0]["permalink"], "start-here", "{v}");

    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{v}");
    assert_eq!(links[0]["resolved"], true, "{v}");
    assert_eq!(links[0]["permalink"], "start-here", "{v}");

    // A link that leaves the domain is reported, unresolved and without an
    // address.
    let v = read(&engine, "kit/valve-three").await;
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 2, "{v}");
    assert_eq!(links[0]["resolved"], true);
    assert_eq!(links[1]["target"]["target"], "../../old/plant.md");
    assert_eq!(links[1]["resolved"], false, "{v}");
    assert!(links[1].get("permalink").is_none(), "{v}");

    // The target counts the links in.
    let v = read(&engine, "start-here").await;
    assert_eq!(v["inbound"]["count"], 3, "{}", v["inbound"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sweep_sees_relative_links_as_edges() {
    let (_tmp, engine) = fixture().await;
    let v = engine
        .evolve_detect(
            &EvolveParams {
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let queue = v["queue"].as_array().unwrap();
    let about =
        |rule: &str| -> Vec<&Value> { queue.iter().filter(|r| r["rule"] == rule).collect() };

    // V104: only the engram nothing links is an orphan. The others are joined
    // by relative links alone.
    let orphans: Vec<&str> = about("V104")
        .iter()
        .map(|r| r["permalink"].as_str().unwrap())
        .collect();
    assert_eq!(orphans, ["lone"], "{v}");

    // V102: the link out of the domain is broken, and its repair is a person's
    // call written as the link it is, never a wikilink swap.
    let broken = about("V102");
    assert_eq!(broken.len(), 1, "{v}");
    let row = broken[0];
    assert_eq!(row["permalink"], "kit/valve-three");
    assert_eq!(row["class"], "judgment", "{row}");
    let fix = row["fix"].as_str().unwrap();
    assert!(!fix.contains("[["), "{row}");
    assert!(fix.contains("../../old/plant.md"), "{row}");
}

/// A wikilink spelled as the target's file path binds by that path, as a
/// relative link does, so the read reports the permalink it lands on too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wikilink_spelled_as_a_path_reports_its_permalink() {
    let (tmp, engine) = fixture().await;
    std::fs::write(
        tmp.path().join("plant/wiki.md"),
        note(
            "Wiki",
            "wiki",
            "See [[pumps/p-12.md]] and [[Overview]] and [[Pumps/P-12.md]].",
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    let v = read(&engine, "wiki").await;
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 3, "{v}");
    assert_eq!(links[0]["target"]["target"], "pumps/p-12.md");
    assert_eq!(links[0]["resolved"], true, "{v}");
    assert_eq!(links[0]["permalink"], "kit/pump-twelve", "{v}");
    // A wikilink by title already names its target.
    assert_eq!(links[1]["resolved"], true, "{v}");
    assert!(links[1].get("permalink").is_none(), "{v}");
    // A path is compared exactly, case included, as the index compares it.
    assert_eq!(links[2]["target"]["target"], "Pumps/P-12.md");
    assert_eq!(links[2]["resolved"], false, "{v}");
    assert!(links[2].get("permalink").is_none(), "{v}");
}

/// A link back to a folder's `index.md` or `log.md` names one of the files
/// Crystalline keeps in every folder, never an engram: the read lists no link
/// and the sweep raises no V102 for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_to_a_folders_index_or_log_is_no_broken_link() {
    let (tmp, engine) = fixture().await;
    std::fs::write(
        tmp.path().join("plant/pumps/home.md"),
        note(
            "Home",
            "kit/home",
            "Back to [the folder](index.md), [the top](../index.md) and [the log](../log.md).\n\nAlso [[pumps/index.md]] and [[plant:log.md]].\n\nPart of [the overview](../overview.md).",
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    let v = read(&engine, "kit/home").await;
    let links = v["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{v}");
    assert_eq!(links[0]["target"]["target"], "../overview.md", "{v}");

    let v = engine
        .evolve_detect(
            &EvolveParams {
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let broken: Vec<&Value> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["rule"] == "V102" && r["permalink"] == "kit/home")
        .collect();
    assert!(broken.is_empty(), "{broken:?}");
}
