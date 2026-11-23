//! The reflect queue of pages that keep themselves current: which engrams a
//! page with a freshness keeps covered, the `stale` item of a page whose
//! freshness ran out, a changed page pact, a gone owner and the links to a
//! retired or removed page.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, SystemTime};

use crystalline_core::reflection as r;
use crystalline_index::ReflectionReconcile;
use crystalline_service::EngineError;
use crystalline_service::params::{ReflectParams, WriteReflectionParams};
use crystalline_service::read_limit::{Limit, within};
use serde_json::Value;

use crate::docs::{agent, person, write};
use crate::fixture::{Fx, beside, engram, fixture, manifest};

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
    // `_top` with no sources names the top as curated.
    let mut sources = BTreeMap::from([("_top".to_string(), Vec::new())]);
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
    sourced(
        &fx,
        "guides/week.md",
        "This week",
        &[("Next", &["weekly/d"])],
    )
    .await;
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
        engram(
            "Change a",
            "weekly/a",
            "- [fact] change a landed, then moved",
        ),
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

/// The queue of `eng` with `stale` items asked for.
fn with_stale() -> ReflectParams {
    ReflectParams {
        stale: Some(true),
        ..eng()
    }
}

async fn queue(fx: &Fx, p: &ReflectParams) -> Value {
    fx.engine.reflect(p, &agent()).await.unwrap()
}

fn items<'a>(queue: &'a Value, what: &str) -> Vec<&'a Value> {
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == what)
        .collect()
}

fn paths(queue: &Value, what: &str) -> Vec<String> {
    items(queue, what)
        .iter()
        .map(|i| i["path"].as_str().unwrap().to_string())
        .collect()
}

/// Record a reconcile of `path` now (with the pact hash it has now), then
/// move its time `ago` seconds into the past.
async fn reconciled_ago(fx: &Fx, path: &str, ago: i64) {
    fx.engine
        .write_reflection(&reconciled(path, "updated", ""), &agent())
        .await
        .unwrap();
    backdate(fx, path, ago).await;
}

/// Move the recorded reconcile time of `path` `ago` seconds into the past.
async fn backdate(fx: &Fx, path: &str, ago: i64) {
    let store = fx.engine.store();
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    let mut state: ReflectionReconcile = store
        .reflection_reconcile(id, path)
        .await
        .unwrap()
        .expect("a recorded reconcile");
    let at = crystalline_core::time::now()
        .saturating_sub(crystalline_core::time::saturating_seconds(ago));
    state.last_reconciled = at.rfc3339();
    store.set_reflection_reconcile(id, &state).await.unwrap();
}

fn reconciled(path: &str, outcome: &str, version: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        outcome: Some(outcome.into()),
        version: Some(version.into()),
        ..act("reconciled", path)
    }
}

/// An agent's create of `path` owned by `owner`.
async fn owned(fx: &Fx, path: &str, owner: &str) {
    let mut p = write(path, "# Child\n\n## Part\n\nText.\n");
    p.owner = Some(owner.into());
    fx.engine.write_reflection(&p, &agent()).await.unwrap();
}

const DAY: i64 = 86_400;

/// A page whose freshness ran out gets one `stale` item: its freshness, when
/// it was last reconciled, how many engrams changed since then, the pacts
/// it follows (paths and one hash, each text once per answer), the pages it
/// owns, its sections a person changed, and the version a `reconciled`
/// echoes. A page still fresh, an owned page (its freshness is ignored) and
/// a retired page get none.
#[tokio::test]
async fn a_page_whose_freshness_ran_out_is_stale() {
    let fx = weekly().await;
    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nPlain.\n").await;
    put(
        &fx,
        "guides/week.md",
        "---\nfreshness: 1d\n---\n# This week\n",
    )
    .await;
    sourced(&fx, "guides/week.md", "This week", &[("A", &["weekly/a"])]).await;
    put(
        &fx,
        "guides/week.pact.md",
        "# Week pact\n\n## Scope\n\nLast seven days.\n",
    )
    .await;
    put(&fx, "guides/fresh.md", "---\nfreshness: 7d\n---\n# Fresh\n").await;
    put(&fx, "guides/gone.md", "---\nfreshness: 1d\n---\n# Gone\n").await;
    owned(&fx, "guides/week/mon.md", "guides/week.md").await;
    reconciled_ago(&fx, "guides/week.md", 2 * DAY).await;
    reconciled_ago(&fx, "guides/fresh.md", DAY).await;
    reconciled_ago(&fx, "guides/gone.md", 2 * DAY).await;
    fx.engine
        .write_reflection(&act("retire", "guides/gone.md"), &agent())
        .await
        .unwrap();
    // A person's change to a section of the stale page: unsettled.
    let text = std::fs::read_to_string(fx.root.join("reflections/guides/week.md"))
        .unwrap()
        .replace("About A.", "About A, edited.");
    put(&fx, "guides/week.md", &text).await;

    let q = queue(&fx, &with_stale()).await;
    assert_eq!(paths(&q, "stale"), vec!["guides/week.md"], "{q}");
    let item = items(&q, "stale")[0];
    assert_eq!(item["kind"], "update");
    assert_eq!(item["freshness"], "1d");
    let state = fx
        .engine
        .reflection_reconcile("eng", "guides/week.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item["last_reconciled"], state.last_reconciled.as_str());
    assert_eq!(item["hint"], 4, "the four engrams changed since: {item}");
    assert_eq!(
        item["pacts"],
        serde_json::json!(["PACT.md", "guides/week.pact.md"])
    );
    assert_eq!(item["pact_hash"], state.pact_hash.clone().unwrap().as_str());
    assert_eq!(item["owned"], serde_json::json!(["guides/week/mon.md"]));
    let a_id = r::split_sections(
        &std::fs::read_to_string(fx.root.join("reflections/guides/week.md")).unwrap(),
    )
    .into_iter()
    .find(|section| section.heading == "A")
    .and_then(|section| section.id)
    .unwrap();
    assert_eq!(item["unsettled"], serde_json::json!([a_id]), "{item}");
    assert!(item["version"].as_str().is_some_and(|v| !v.is_empty()));
    // Each pact text once per answer.
    assert!(
        q["pacts"]["eng"]["guides/week.pact.md"]
            .as_str()
            .unwrap()
            .contains("Last seven days."),
        "{q}"
    );
    assert!(
        q["pacts"]["eng"]["PACT.md"]
            .as_str()
            .unwrap()
            .contains("Plain.")
    );

    // The same answer twice has the same version: it moves with the
    // period, never with the clock.
    let again = queue(&fx, &with_stale()).await;
    assert_eq!(items(&again, "stale")[0]["version"], item["version"]);
}

/// `stale` items are handed out to a run (an unattended session) and on
/// request, never by default to an interactive session, and never count in
/// the open-item nudges.
#[tokio::test]
async fn stale_goes_to_a_run_or_on_request_and_is_no_nudge() {
    let fx = weekly().await;
    put(
        &fx,
        "guides/week.md",
        "---\nfreshness: 1d\n---\n# This week\n",
    )
    .await;
    sourced(
        &fx,
        "guides/week.md",
        "This week",
        &[
            ("A", &["weekly/a"]),
            ("B", &["weekly/b"]),
            ("C", &["weekly/c"]),
            ("D", &["weekly/d"]),
        ],
    )
    .await;
    reconciled_ago(&fx, "guides/week.md", 2 * DAY).await;

    assert!(paths(&queue(&fx, &eng()).await, "stale").is_empty());
    assert_eq!(
        fx.engine
            .reflect_open_counts(&["eng".to_string()], &agent())
            .await
            .unwrap(),
        Vec::<(String, usize)>::new(),
        "a stale page is no open item for a nudge"
    );
    let run = Limit {
        unattended: true,
        ..Limit::default()
    };
    let q = within(Some(run), fx.engine.reflect(&eng(), &agent()))
        .await
        .unwrap();
    assert_eq!(paths(&q, "stale"), vec!["guides/week.md"]);
    assert_eq!(
        paths(&queue(&fx, &with_stale()).await, "stale"),
        vec!["guides/week.md"]
    );
    let refused = ReflectParams {
        stale: Some(false),
        ..eng()
    };
    let run = Limit {
        unattended: true,
        ..Limit::default()
    };
    let q = within(Some(run), fx.engine.reflect(&refused, &agent()))
        .await
        .unwrap();
    assert!(paths(&q, "stale").is_empty(), "asked not to: {q}");
}

/// The hint counts the engrams changed since the last reconcile in the
/// domains the run may read: under a read limit, a hidden domain counts
/// nothing. With nothing changed it is 0.
#[tokio::test]
async fn the_hint_counts_only_what_the_run_may_read() {
    let fx = beside(
        &[(
            "runbooks/deploy.md",
            &engram("Deploy", "runbooks/deploy", "- [fact] at night"),
        )],
        "harbor",
        &[
            ("team/a.md", &engram("A", "team/a", "- [fact] a")),
            ("team/b.md", &engram("B", "team/b", "- [fact] b")),
        ],
    )
    .await;
    put(
        &fx,
        "guides/week.md",
        "---\nfreshness: 1d\n---\n# This week\n",
    )
    .await;
    reconciled_ago(&fx, "guides/week.md", 2 * DAY).await;

    let hint = |q: &Value| items(q, "stale")[0]["hint"].clone();
    assert_eq!(hint(&queue(&fx, &with_stale()).await), 3, "eng 1, harbor 2");
    let run = Limit {
        hidden: HashSet::from(["harbor".to_string()]),
        unattended: true,
        ..Limit::default()
    };
    let q = within(Some(run), fx.engine.reflect(&eng(), &agent()))
        .await
        .unwrap();
    assert_eq!(hint(&q), 1, "harbor is hidden from the run: {q}");

    // Nothing changed since the last reconcile: every engram's last change
    // is older than it.
    let harbor = crate::fixture::root_of(&fx, "harbor");
    for (abs, title, permalink) in [
        (
            fx.root.join("runbooks/deploy.md"),
            "Deploy",
            "runbooks/deploy",
        ),
        (harbor.join("team/a.md"), "A", "team/a"),
        (harbor.join("team/b.md"), "B", "team/b"),
    ] {
        std::fs::write(&abs, engram(title, permalink, "- [fact] older")).unwrap();
        let file = std::fs::File::options().write(true).open(&abs).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(3 * DAY as u64))
            .unwrap();
    }
    fx.engine.sync(None).await.unwrap();
    assert_eq!(hint(&queue(&fx, &with_stale()).await), 0);
}

/// A stale item comes after every feedback item, older or not, and the
/// page's own `source_changed` items fold into it.
#[tokio::test]
async fn stale_ranks_after_feedback_and_takes_the_pages_source_changes() {
    let fx = weekly().await;
    put(
        &fx,
        "guides/week.md",
        "---\nfreshness: 1d\n---\n# This week\n",
    )
    .await;
    sourced(&fx, "guides/week.md", "This week", &[("A", &["weekly/a"])]).await;
    sourced(&fx, "guides/plain.md", "Plain", &[("B", &["weekly/b"])]).await;
    reconciled_ago(&fx, "guides/week.md", 30 * DAY).await;
    // A person's change on the plain page, younger than the stale page.
    let text = std::fs::read_to_string(fx.root.join("reflections/guides/plain.md"))
        .unwrap()
        .replace("About B.", "About B, edited.");
    put(&fx, "guides/plain.md", &text).await;
    // weekly/a changes: a source_changed on the stale page.
    std::fs::write(
        fx.root.join("weekly/a.md"),
        engram("Change a", "weekly/a", "- [fact] change a moved"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();

    let q = queue(&fx, &with_stale()).await;
    let order: Vec<(String, String)> = q["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["what"].as_str().unwrap().to_string(),
                i["path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let feedback = order
        .iter()
        .position(|(w, _)| w == "changed")
        .expect("feedback");
    let stale = order.iter().position(|(w, _)| w == "stale").expect("stale");
    assert!(feedback < stale, "{order:?}");
    assert!(
        !order
            .iter()
            .any(|(w, p)| w == "source_changed" && p == "guides/week.md"),
        "folded: {order:?}"
    );
    let folded = &items(&q, "stale")[0]["source_changed"];
    assert_eq!(folded[0]["heading"], "A", "{folded}");
    assert_eq!(folded[0]["sources"][0]["permalink"], "weekly/a", "{folded}");
}

/// A `reconciled` for an older period is refused: another reconcile was
/// recorded after the stale item was handed out. The current item's
/// version, a page with no recorded reconcile and a version that is no
/// stale version go through.
#[tokio::test]
async fn a_late_reconcile_for_an_older_period_is_refused() {
    let fx = weekly().await;
    put(
        &fx,
        "guides/week.md",
        "---\nfreshness: 1d\n---\n# This week\n",
    )
    .await;
    put(&fx, "guides/new.md", "---\nfreshness: 1d\n---\n# New\n").await;
    reconciled_ago(&fx, "guides/week.md", 2 * DAY).await;
    let version = |q: &Value| {
        items(q, "stale")[0]["version"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let first = version(&queue(&fx, &with_stale()).await);

    // Two runs got the same item; the first records its reconcile.
    fx.engine
        .write_reflection(&reconciled("guides/week.md", "unchanged", &first), &agent())
        .await
        .unwrap();
    let late = fx
        .engine
        .write_reflection(&reconciled("guides/week.md", "updated", &first), &agent())
        .await
        .unwrap_err();
    assert!(matches!(late, EngineError::Conflict(_)), "{late}");
    assert!(late.to_string().contains("older period"), "{late}");

    // The next period's item goes through, also when a period boundary
    // passed while the run worked (same reconcile, an earlier period index):
    // only a reconcile recorded after the item was handed out makes it older.
    backdate(&fx, "guides/week.md", 2 * DAY).await;
    let next = version(&queue(&fx, &with_stale()).await);
    assert_ne!(next, first);
    let mut parts = next.splitn(3, ':');
    let (base, period) = (parts.next().unwrap(), parts.next().unwrap());
    let ticked = format!(
        "{base}:{}:{}",
        period.parse::<u64>().unwrap() - 1,
        "0".repeat(64)
    );
    fx.engine
        .write_reflection(&reconciled("guides/week.md", "updated", &ticked), &agent())
        .await
        .unwrap();
    // And the item handed out with it is late now.
    let late = fx
        .engine
        .write_reflection(&reconciled("guides/week.md", "updated", &next), &agent())
        .await
        .unwrap_err();
    assert!(matches!(late, EngineError::Conflict(_)), "{late}");

    // A page reconciled for the first time: its seeded item's version is
    // never older than a recorded reconcile.
    fx.engine
        .write_reflection(&reconciled("guides/new.md", "updated", "0:1:abc"), &agent())
        .await
        .unwrap();
    // A version that is no stale version (another item's) is taken as sent.
    fx.engine
        .write_reflection(&reconciled("guides/new.md", "unchanged", "v1"), &agent())
        .await
        .unwrap();
}

/// A changed page pact raises `page_pact_changed` for its page and every
/// page that follows it, until the pact file is settled. A stored pact hash
/// that no longer matches (PACT.md changed, on disk or by a person) raises
/// it for that page until it is reconciled; a page with no stored hash and
/// a retired page raise nothing from it.
#[tokio::test]
async fn a_changed_pact_raises_page_pact_changed_on_the_pages_that_follow_it() {
    let fx = weekly().await;
    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nPlain.\n").await;
    for page in [
        "guides/week.md",
        "guides/week/mon.md",
        "guides/other.md",
        "guides/old.md",
    ] {
        sourced(&fx, page, "Page", &[("A", &["weekly/a"])]).await;
    }
    // The agent writes the page pact: its baseline is what it wrote.
    let pact = "# Week pact\n\n## Scope\n\nLast seven days.\n";
    fx.engine
        .write_reflection(&write("guides/week.pact.md", pact), &agent())
        .await
        .unwrap();
    for page in ["guides/week.md", "guides/other.md", "guides/old.md"] {
        reconciled_ago(&fx, page, 60).await;
    }
    fx.engine
        .write_reflection(&act("retire", "guides/old.md"), &agent())
        .await
        .unwrap();
    assert!(paths(&queue(&fx, &eng()).await, "page_pact_changed").is_empty());

    // A person changes the page pact: its page and the page that follows it.
    let text = std::fs::read_to_string(fx.root.join("reflections/guides/week.pact.md"))
        .unwrap()
        .replace("seven", "fourteen");
    put(&fx, "guides/week.pact.md", &text).await;
    let q = queue(&fx, &eng()).await;
    assert_eq!(
        paths(&q, "page_pact_changed"),
        vec!["guides/week.md", "guides/week/mon.md"],
        "{q}"
    );
    let item = items(&q, "page_pact_changed")[0];
    assert_eq!(item["pacts"], serde_json::json!(["guides/week.pact.md"]));
    assert!(
        item["diff"].as_str().unwrap().contains("fourteen"),
        "{item}"
    );

    // Settling the pact file ends the item of the page with no stored hash;
    // the reconciled page waits for its own reconcile.
    fx.engine
        .write_reflection(&act("settle", "guides/week.pact.md"), &agent())
        .await
        .unwrap();
    assert_eq!(
        paths(&queue(&fx, &eng()).await, "page_pact_changed"),
        vec!["guides/week.md"]
    );
    fx.engine
        .write_reflection(&reconciled("guides/week.md", "updated", ""), &agent())
        .await
        .unwrap();
    assert!(paths(&queue(&fx, &eng()).await, "page_pact_changed").is_empty());

    // PACT.md changes on disk: every page with a stored hash, none other.
    std::fs::write(
        fx.root.join("reflections/PACT.md"),
        "# Pact\n\n## Tone\n\nPlain and short.\n",
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let q = queue(&fx, &eng()).await;
    assert_eq!(
        paths(&q, "page_pact_changed"),
        vec!["guides/other.md", "guides/week.md"],
        "{q}"
    );
    assert_eq!(paths(&q, "pact_changed"), vec!["PACT.md"]);
}

/// A page whose owner is gone or retired is reconciled by nobody: it gets
/// `owner_gone`, which says how to keep it or let it go.
#[tokio::test]
async fn a_page_whose_owner_is_gone_or_retired_says_so() {
    let fx = weekly().await;
    put(&fx, "guides/week.md", "# This week\n").await;
    put(&fx, "guides/month.md", "# This month\n").await;
    owned(&fx, "guides/week/mon.md", "guides/week.md").await;
    owned(&fx, "guides/month/june.md", "guides/month.md").await;
    assert!(paths(&queue(&fx, &eng()).await, "owner_gone").is_empty());

    fx.engine
        .write_reflection(&act("delete", "guides/week.md"), &person())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&act("retire", "guides/month.md"), &agent())
        .await
        .unwrap();
    let q = queue(&fx, &eng()).await;
    let gone = items(&q, "owner_gone");
    let by_path: BTreeMap<&str, &Value> = gone
        .iter()
        .map(|i| (i["path"].as_str().unwrap(), *i))
        .collect();
    assert_eq!(by_path.len(), 2, "{q}");
    assert_eq!(by_path["guides/week/mon.md"]["owner"], "guides/week.md");
    assert_eq!(by_path["guides/week/mon.md"]["owner_is"], "missing");
    assert_eq!(by_path["guides/month/june.md"]["owner_is"], "retired");
    let hint = by_path["guides/month/june.md"]["hint"].as_str().unwrap();
    assert!(
        hint.contains("retire") && hint.contains("restore"),
        "{hint}"
    );
}

/// A page that links a retired page gets `link_retired`, one that links a
/// page that is not there `link_broken`; a retired page raises neither.
#[tokio::test]
async fn links_to_a_retired_or_removed_page_are_named() {
    let fx = weekly().await;
    put(&fx, "guides/old.md", "# Old\n").await;
    put(&fx, "guides/gone.md", "# Gone\n").await;
    put(
        &fx,
        "guides/links.md",
        "# Index\n\n## Links\n\n[old](old.md) and [gone](gone.md) and [here](links.md)\n",
    )
    .await;
    put(
        &fx,
        "guides/also.md",
        "# Also\n\n## See\n\n[gone](gone.md)\n",
    )
    .await;
    fx.engine
        .write_reflection(&act("retire", "guides/old.md"), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&act("retire", "guides/also.md"), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&act("delete", "guides/gone.md"), &person())
        .await
        .unwrap();

    let q = queue(&fx, &eng()).await;
    let retired = items(&q, "link_retired");
    assert_eq!(retired.len(), 1, "{q}");
    assert_eq!(retired[0]["path"], "guides/links.md");
    assert_eq!(retired[0]["heading"], "Links");
    assert_eq!(retired[0]["links"], serde_json::json!(["guides/old.md"]));
    let broken = items(&q, "link_broken");
    assert_eq!(broken.len(), 1, "the retired page raises none: {q}");
    assert_eq!(broken[0]["links"], serde_json::json!(["guides/gone.md"]));
}

/// Owned and retired pages are never off the pact's structure; a retired
/// page is never brought up to date from its sources.
#[tokio::test]
async fn owned_and_retired_pages_are_exempt() {
    let fx = weekly().await;
    put(&fx, "PACT.md", "# Pact\n\n## Structure\n\n- guides/*.md\n").await;
    put(&fx, "guides/week.md", "# This week\n").await;
    owned(&fx, "guides/week/mon.md", "guides/week.md").await;
    sourced(&fx, "other/old.md", "Old", &[("A", &["weekly/a"])]).await;
    put(&fx, "other/loose.md", "# Loose\n").await;
    fx.engine
        .write_reflection(&act("retire", "other/old.md"), &agent())
        .await
        .unwrap();
    std::fs::write(
        fx.root.join("weekly/a.md"),
        engram("Change a", "weekly/a", "- [fact] change a moved"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();

    let q = queue(&fx, &eng()).await;
    assert_eq!(paths(&q, "off_structure"), vec!["other/loose.md"], "{q}");
    assert!(paths(&q, "source_changed").is_empty(), "{q}");
}

/// A `reflect` call that does not ask about stale items sends no `stale`
/// key, so a call forwarded to an older server, which refuses a key it does
/// not know, goes through as before.
#[test]
fn an_unset_stale_is_left_out_of_a_forwarded_call() {
    let sent = serde_json::to_value(ReflectParams::default()).unwrap();
    assert!(sent.get("stale").is_none(), "{sent}");
    let asked = serde_json::to_value(ReflectParams {
        stale: Some(true),
        ..ReflectParams::default()
    })
    .unwrap();
    assert_eq!(asked["stale"], true);
}
