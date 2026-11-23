//! The reflect queue of pages that keep themselves current: which engrams a
//! page with a freshness keeps covered, the `stale` item of a page whose
//! freshness ran out, a changed page pact, a gone owner and the links to a
//! retired or removed page.

use std::collections::{BTreeMap, BTreeSet};

use crystalline_service::params::{ReflectParams, WriteReflectionParams};

use crate::docs::{agent, person, write};
use crate::fixture::{Fx, engram, fixture, manifest};

/// `eng` with Reflection on and the engrams `weekly/a` to `weekly/d`.
async fn weekly() -> Fx {
    let files: Vec<(String, String)> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| {
            (
                format!("weekly/{n}.md"),
                engram(
                    &format!("Change {n}"),
                    &format!("weekly/{n}"),
                    &format!("- [fact] change {n} landed"),
                ),
            )
        })
        .chain([("MANIFEST.md".to_string(), manifest("reflection: on\n"))])
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    fixture(&refs).await
}

async fn put(fx: &Fx, path: &str, text: &str) {
    fx.engine
        .write_reflection(&write(path, text), &person())
        .await
        .unwrap();
}

/// An agent's whole-page write of `path` whose sections (heading, sources)
/// are `sections`, each with one line of text.
async fn sourced(fx: &Fx, path: &str, title: &str, sections: &[(&str, &[&str])]) {
    let mut text = format!("# {title}\n");
    let mut sources = BTreeMap::new();
    for (heading, cites) in sections {
        text.push_str(&format!("\n## {heading}\n\nAbout {heading}.\n"));
        sources.insert(
            heading.to_string(),
            cites.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
        );
    }
    let mut p = write(path, &text);
    p.sources = sources;
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
}

fn act(action: &str, path: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        domain: "eng".into(),
        action: Some(action.into()),
        path: path.into(),
        ..Default::default()
    }
}

fn eng() -> ReflectParams {
    ReflectParams {
        domains: vec!["eng".into()],
        limit: Some(100),
        ..Default::default()
    }
}

/// Every engram an `unreflected` item of the queue names.
async fn unreflected(fx: &Fx) -> BTreeSet<String> {
    let queue = fx.engine.reflect(&eng(), &agent()).await.unwrap();
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == "unreflected")
        .flat_map(|i| i["engrams"].as_array().unwrap().clone())
        .map(|e| e.as_str().unwrap().to_string())
        .collect()
}

/// Rule A (Jordi, 2026-11-23): an engram once cited by a page with a
/// freshness stays covered after the page drops it (last week's change
/// leaves a weekly page), until the engram changes; then it is
/// `unreflected` again unless a page cites it now. A page without a
/// freshness covers only what it cites now, and a retired page covers
/// nothing.
#[tokio::test]
async fn an_engram_a_rolling_page_dropped_stays_covered_until_it_changes() {
    let fx = weekly().await;
    // A rolling page: its freshness is a curator's, its sections the agent's.
    put(
        &fx,
        "guides/week.md",
        "---\nfreshness: 7d\n---\n# This week\n",
    )
    .await;
    sourced(&fx, "guides/week.md", "This week", &[("A", &["weekly/a"])]).await;
    // A page without a freshness, and one that will be retired.
    sourced(&fx, "guides/plain.md", "Plain", &[("B", &["weekly/b"])]).await;
    sourced(&fx, "guides/old.md", "Old", &[("C", &["weekly/c"])]).await;
    sourced(&fx, "guides/keep.md", "Keep", &[("D", &["weekly/d"])]).await;
    assert_eq!(unreflected(&fx).await, BTreeSet::new(), "all cited");

    // The next week: both pages drop their change, the old page retires.
    sourced(&fx, "guides/week.md", "This week", &[("Next", &["weekly/d"])]).await;
    sourced(&fx, "guides/plain.md", "Plain", &[("Other", &["weekly/d"])]).await;
    fx.engine
        .write_reflection(&act("retire", "guides/old.md"), &agent())
        .await
        .unwrap();
    assert_eq!(
        unreflected(&fx).await,
        BTreeSet::from(["weekly/b".to_string(), "weekly/c".to_string()]),
        "the rolling page keeps weekly/a covered; the plain page and the retired one do not"
    );

    // weekly/a changes: its checksum moves, and nothing cites it now.
    std::fs::write(
        fx.root.join("weekly/a.md"),
        engram("Change a", "weekly/a", "- [fact] change a landed, then moved"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    assert!(
        unreflected(&fx).await.contains("weekly/a"),
        "a changed engram is unreflected again"
    );

    // A page that cites it now covers it again.
    sourced(
        &fx,
        "guides/plain.md",
        "Plain",
        &[("Other", &["weekly/d"]), ("A again", &["weekly/a"])],
    )
    .await;
    assert!(!unreflected(&fx).await.contains("weekly/a"));
}
