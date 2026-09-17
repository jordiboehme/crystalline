//! Engine-level coverage for the consolidation sweep behind `evolve`.
//!
//! One fixture domain plants exactly one exemplar of every `V` rule, so the
//! whole catalog is exercised against real parsing, real indexing and the real
//! graph rather than hand-built facts (the detector library's own unit tests
//! cover the predicates in isolation). Every assertion pins `today`, which is
//! what makes a run reproducible: the detectors never read the clock, the
//! engine supplies the date.

mod support;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crystalline_core::config::{DomainEntry, GlobalConfig, OriginConfig};
use crystalline_index::TursoStore;
use crystalline_remote::state::OriginState;
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{EditParams, EvolveParams};
use serde_json::Value;
use tokio::sync::Mutex;

/// The date every assertion evaluates the fixture against.
const TODAY: &str = "2026-08-02";

/// A date early enough that no temporal rule can fire on the fixture: before
/// every planted window, staleness date and age floor.
const BEFORE_EVERYTHING: &str = "2025-06-01";

/// Build an engine over a temporary file domain carrying the fixture, synced.
async fn fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("eng");
    for (rel, body) in files() {
        let abs = dir.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
    }
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// A body long enough to blow past the default 2500 token budget
/// (`chars / 4`), for `V105`.
fn oversized_body() -> String {
    let mut out = String::new();
    for i in 0..120 {
        out.push_str(&format!(
            "Paragraph {i} of the migration log records which shard moved, who approved the move \
             and what the replica lag looked like once the switch completed.\n\n"
        ));
    }
    out
}

/// The fixture files, one exemplar per rule. Every engram not meant to trip a
/// rule is dated inside the orphan and staleness floors so it stays quiet.
fn files() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |rel: &str, body: String| out.push((rel.to_string(), body));

    add(
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\n# eng\n\n## Scope\n\n- Everything about engineering\n\n## When to Use\n\n- Route here for engineering questions\n".to_string(),
    );

    // V005: the replacement landed, the retirement never did. The finding
    // attaches to the still-stable old pipeline.
    add(
        "deploy/new-pipeline.md",
        "---\ntype: engram\ntitle: Deploy new pipeline\npermalink: deploy/new-pipeline\ntags:\n  - deploys\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nThe new pipeline builds the image once and promotes it through the canary check.\n\n- supersedes [[Old deploy pipeline]]\n- [decision] we cut over at the start of July\n".to_string(),
    );
    add(
        "deploy/old-pipeline.md",
        "---\ntype: engram\ntitle: Old deploy pipeline\npermalink: deploy/old-pipeline\ntags:\n  - deploy\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nThe old pipeline ran the rollout by hand from a checklist.\n\n- [context] it served us for two years\n- [lesson] manual steps drift out of date\n".to_string(),
    );

    // V001: the validity window closed while the status still reads current.
    add(
        "expired-policy.md",
        "---\ntype: engram\ntitle: Retention policy 2025\npermalink: expired-policy\ntags:\n  - retention\nstatus: stable\nrecorded_at: 2026-07-25\nvalid_to: 2026-01-01\n---\n\nLogs were kept for ninety days under the 2025 retention policy.\n\n- [context] the window closed at the end of the year\n- [decision] renew it or retire it before the audit\n".to_string(),
    );

    // V002: the staleness date elapsed with no verification since.
    add(
        "stale-runbook.md",
        "---\ntype: engram\ntitle: Index rebuild runbook\npermalink: stale-runbook\ntags:\n  - rebuild\nstatus: stable\nrecorded_at: 2026-07-25\nstale_after: 2026-06-01\n---\n\nRebuild the index from the daemon rather than the CLI so the lock is held once.\n\n- [context] the rebuild takes about an hour\n- [lesson] never rebuild while a sync is running\n".to_string(),
    );

    // V003: old, never verified, no staleness bound. Linked so the orphan rule
    // stays out of it.
    add(
        "ancient-note.md",
        "---\ntype: engram\ntitle: Ancient note\npermalink: ancient-note\ntags:\n  - history\nstatus: stable\nrecorded_at: 2025-01-01\n---\n\nThe first shape of the deploy story before the pipeline split in two.\n\n- relates_to [[Deploy new pipeline]]\n- [context] kept for the history it carries\n".to_string(),
    );

    // V004: retired as superseded, naming a successor that does not resolve.
    // Its dangling reference is not a V102, because V102 never speaks about a
    // retired engram.
    add(
        "retired-thing.md",
        "---\ntype: engram\ntitle: Retired thing\npermalink: retired-thing\ntags:\n  - legacy-notes\nstatus: superseded\nrecorded_at: 2026-07-25\n---\n\nThis approach was replaced during the migration.\n\n- superseded_by [[Nothing At All]]\n- [context] the successor was never captured\n".to_string(),
    );

    // V006: a person captured it in their own words and nobody has reviewed it
    // since. Linked and three body lines long so the orphan and stub rules stay
    // out of it, and recent enough that the aging rule does too.
    add(
        "human-capture.md",
        "---\ntype: engram\ntitle: Incident capture\npermalink: human-capture\ntags:\n  - reference\nstatus: stable\nrecorded_at: 2026-07-25\ngenerated:\n  by: \"human:jordi\"\n  at: 2026-07-25T09:12:00+02:00\n---\n\nWritten straight after the incident call, in the words the responder used.\n\n- relates_to [[Live doc]]\n- [context] nobody has read it back since the call\n".to_string(),
    );

    // V101: a current engram pointing at retired knowledge.
    add(
        "live-doc.md",
        "---\ntype: engram\ntitle: Live doc\npermalink: live-doc\ntags:\n  - reference\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nThe current reference for the migration, still cited by the runbooks.\n\n- relates_to [[Retired thing]]\n- [context] the citation was never repointed\n".to_string(),
    );

    // V102: a prose wikilink one letter off an existing title, so the repair is
    // mechanical. The resolved relation keeps it out of the orphan rule.
    add(
        "link-typo.md",
        "---\ntype: engram\ntitle: Link typo\npermalink: link-typo\ntags:\n  - linking\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nRollouts are described in [[Deploy new pipelines]] which nothing resolves to.\n\n- relates_to [[Live doc]]\n- [context] the bracket text was never checked\n".to_string(),
    );

    // V103: a summarizes edge with no summarized_by coming back. The finding
    // attaches to the counterpart, the full text.
    add(
        "summary-doc.md",
        "---\ntype: engram\ntitle: Summary doc\npermalink: summary-doc\ntags:\n  - summary\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nThe distilled version of the migration write-up, three paragraphs long.\n\n- summarizes [[Full text doc]]\n- [context] written after the migration closed\n".to_string(),
    );
    add(
        "full-text-doc.md",
        "---\ntype: engram\ntitle: Full text doc\npermalink: full-text-doc\ntags:\n  - fulltext\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nThe full migration write-up with the shard tables and the approval trail.\n\n- [context] the source the summary was cut from\n- [lesson] keep the tables out of the summary\n".to_string(),
    );

    // V104: no resolved link in or out, old enough that the capture session is
    // long over. The future staleness date keeps V002 and V003 quiet.
    add(
        "lonely-note.md",
        "---\ntype: engram\ntitle: Lonely note\npermalink: lonely-note\ntags:\n  - standalone\nstatus: stable\nrecorded_at: 2026-01-01\nstale_after: 2027-01-01\n---\n\nA note nobody wired into the neighbourhood it belongs to.\n\n- [context] captured in a hurry during an incident\n- [lesson] wire a capture in before the session ends\n".to_string(),
    );

    // V105: over the default token budget.
    add(
        "huge-doc.md",
        format!(
            "---\ntype: engram\ntitle: Huge doc\npermalink: huge-doc\ntags:\n  - oversized\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\n{}",
            oversized_body()
        ),
    );

    // V106: two non-blank body lines, under the three-line floor.
    add(
        "stub-note.md",
        "---\ntype: engram\ntitle: Stub note\npermalink: stub-note\ntags:\n  - stub\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nA title and almost nothing else.\n\n- [context] meant to be filled in later\n".to_string(),
    );

    // V201: two bodies one word apart. The salience on the first makes it the
    // cluster leader deterministically.
    let dup = |peak: &str| {
        format!(
            "Warming the cache before the {peak} traffic peak keeps the first requests off the cold path.\n\
             Run the warmer after the nightly index rebuild finishes and before the queue drains.\n\
             Watch the hit ratio for the first ten minutes and stop the warmer once it settles above ninety percent.\n\
             The warmer reads the same key list the scheduler uses so nothing is ever warmed twice.\n"
        )
    };
    add(
        "dup-a.md",
        format!(
            "---\ntype: engram\ntitle: Cache warming procedure\npermalink: dup-a\ntags:\n  - caching\nstatus: stable\nrecorded_at: 2026-07-25\nsalience: 5\n---\n\n{}",
            dup("morning")
        ),
    );
    add(
        "dup-b.md",
        format!(
            "---\ntype: engram\ntitle: Warming the cache with a script\npermalink: dup-b\ntags:\n  - caching\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\n{}",
            dup("evening")
        ),
    );

    // V202: a plural apart in the same domain, with bodies too short for the
    // duplicate clusterer to look at, so the two rules do not overlap.
    add(
        "title-a.md",
        "---\ntype: engram\ntitle: Deploy checklist\npermalink: deploy-checklist\ntags:\n  - checklist\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nCheck the migration plan.\n\nConfirm the rollback path.\n\nAnnounce the window.\n".to_string(),
    );
    add(
        "title-b.md",
        "---\ntype: engram\ntitle: Deploy checklists\npermalink: deploy-checklists\ntags:\n  - checklist\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nPage the on-call engineer.\n\nDrain the queue first.\n\nRecord who signed off.\n".to_string(),
    );

    // V203 needs no engram of its own: `deploy` and `deploys` above are one
    // concept spelled two ways.
    out
}

/// A sweep over the fixture as of `today`, with the given extra parameters.
///
/// The detection half rather than [`Engine::evolve_engrams`]: the response is
/// identical, and detection records no maintenance run, so the rule assertions
/// below neither depend on the state directory nor write to it. The recording
/// wrapper has its own tests at the end of this file.
async fn sweep(engine: &Engine, today: &str, p: EvolveParams) -> Value {
    engine
        .evolve_detect(
            &EvolveParams {
                today: Some(today.to_string()),
                ..p
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// The `rule` column of a response's queue, in queue order.
fn rules(v: &Value) -> Vec<String> {
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["rule"].as_str().unwrap().to_string())
        .collect()
}

/// The whole catalog fires on the fixture, exactly once each, ranked by
/// priority descending with the rule id breaking a tie. This is the assertion
/// that catches a detector silently going quiet because the engine handed it
/// the wrong facts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_rule_fires_once_and_the_queue_ranks_by_priority() {
    let (_tmp, engine) = fixture().await;
    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;

    assert_eq!(
        rules(&v),
        vec![
            "V005", // 90
            "V001", // 85
            "V201", // 85, base 80 plus the salience boost
            "V002", // 70
            "V004", // 65
            "V105", // 60
            "V006", // 58, base 50 plus the human-authored boost
            "V010", // 55
            "V101", // 55
            "V202", // 55
            "V102", // 50
            "V106", // 45
            "V103", // 35
            "V104", // 30
            "V203", // 30
            "V003", // 25
        ]
    );
    assert_eq!(v["total"], 16);
    assert_eq!(v["count"], 16);
    assert_eq!(v["engrams_scanned"], 19);
    assert_eq!(v["unparsed"], 0);
    assert_eq!(v["scope"]["today"], TODAY);
    assert_eq!(v["scope"]["domains"], serde_json::json!(["eng"]));
    assert!(v["truncations"].as_array().unwrap().is_empty());
    assert!(
        v["guidance"]
            .as_str()
            .unwrap()
            .starts_with("This queue changes nothing by itself.")
    );

    // The family summary counts the whole filtered result, in catalog order.
    assert_eq!(
        v["families"],
        serde_json::json!([
            { "family": "temporal", "findings": 7 },
            { "family": "structure", "findings": 6 },
            { "family": "redundancy", "findings": 3 },
        ])
    );

    // The prose instruction rides the legend once per rule, never a row.
    let actions = v["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 16);
    assert_eq!(actions[0]["rule"], "V001");
    assert!(
        actions
            .iter()
            .all(|a| !a["instruction"].as_str().unwrap().is_empty())
    );

    // The classes the server computed, which is what makes the propose-first
    // rule assertable rather than a matter of prose.
    let by_rule = |rule: &str| -> Value {
        v["queue"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["rule"] == rule)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_rule("V005")["class"], "mechanical");
    assert_eq!(by_rule("V102")["class"], "mechanical");
    assert_eq!(by_rule("V103")["class"], "mechanical");
    assert_eq!(by_rule("V001")["class"], "judgment");

    // The findings attach where the catalog says they do.
    assert_eq!(by_rule("V005")["permalink"], "deploy/old-pipeline");
    assert_eq!(by_rule("V103")["permalink"], "full-text-doc");
    assert_eq!(by_rule("V201")["permalink"], "dup-a");
    assert_eq!(by_rule("V202")["permalink"], "deploy-checklist");
    assert_eq!(by_rule("V105")["permalink"], "huge-doc");
    // The retired engram whose one observation turns up in no live engram of
    // the domain: `- [context] the successor was never captured`.
    assert_eq!(by_rule("V010")["permalink"], "retired-thing");
    assert_eq!(by_rule("V010")["class"], "judgment");

    // V006 reads the `generated.by` actor the engine put on the facts, so this
    // is what catches the fact assembly dropping write provenance: the rule
    // itself has unit coverage, the wiring only has this.
    assert_eq!(by_rule("V006")["permalink"], "human-capture");
    assert_eq!(
        by_rule("V006")["evidence"],
        "generated.by human:jordi; recorded 2026-07-25; no verified entry"
    );
    assert_eq!(by_rule("V006")["class"], "judgment");

    // V102 quotes the bracket text verbatim and points at the near match, and
    // it is the typo rather than the retired engram's dangling successor: a
    // retired engram is V004's alone.
    assert_eq!(by_rule("V102")["permalink"], "link-typo");
    assert_eq!(
        by_rule("V102")["fix"],
        "[[Deploy new pipelines]] -> [[Deploy new pipeline]]"
    );
    assert!(by_rule("V102")["line"].is_number());

    // V203 is about a domain's vocabulary, not one engram, so it carries no
    // permalink and no title and the shaping has to tolerate that.
    assert_eq!(by_rule("V203")["permalink"], "");
    assert_eq!(by_rule("V203")["title"], "");
    assert!(
        by_rule("V203")["fix"]
            .as_str()
            .unwrap()
            .contains("crystalline tags merge")
    );
}

/// Paging walks one ranked queue: `n` is the rank across the whole result, the
/// total never moves and no finding is seen twice or skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paging_walks_the_same_ranked_queue() {
    let (_tmp, engine) = fixture().await;
    let mut walked: Vec<String> = Vec::new();
    for page in 1..=4 {
        let v = sweep(
            &engine,
            TODAY,
            EvolveParams {
                limit: Some(5),
                page: Some(page),
                ..EvolveParams::default()
            },
        )
        .await;
        assert_eq!(v["total"], 16);
        assert_eq!(v["limit"], 5);
        assert_eq!(v["page"], page);
        assert_eq!(
            v["count"],
            if page == 4 { 1 } else { 5 },
            "sixteen findings fill three whole pages and one more row"
        );
        for (i, row) in v["queue"].as_array().unwrap().iter().enumerate() {
            assert_eq!(row["n"].as_u64().unwrap() as usize, (page - 1) * 5 + i + 1);
        }
        walked.extend(rules(&v));
    }

    let all = sweep(
        &engine,
        TODAY,
        EvolveParams {
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(walked, rules(&all));

    // A page past the end is empty rather than an error, so an agent that keeps
    // paging stops cleanly.
    let past = sweep(
        &engine,
        TODAY,
        EvolveParams {
            limit: Some(5),
            page: Some(9),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(past["total"], 16);
    assert_eq!(past["count"], 0);
    assert!(past["queue"].as_array().unwrap().is_empty());
}

/// The `today` override moves the temporal comparisons and nothing else, which
/// is what makes a run reproducible. Evaluated before every planted date, the
/// age and window rules go silent while the structural and redundancy rules are
/// unchanged - and so are the three temporal rules that compare no date at all
/// (`V004`, `V005` and `V010`, which read the graph and the text).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_today_override_moves_only_the_temporal_rules() {
    let (_tmp, engine) = fixture().await;
    let v = sweep(
        &engine,
        BEFORE_EVERYTHING,
        EvolveParams {
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;

    let mut fired = rules(&v);
    fired.sort();
    assert_eq!(
        fired,
        vec![
            "V004", "V005", "V010", "V101", "V102", "V103", "V105", "V106", "V201", "V202", "V203"
        ]
    );
    assert_eq!(v["scope"]["today"], BEFORE_EVERYTHING);
    assert_eq!(v["engrams_scanned"], 19);

    // Two runs over the same scope and the same date are identical, findings
    // and order alike.
    let again = sweep(
        &engine,
        BEFORE_EVERYTHING,
        EvolveParams {
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(v, again);
}

/// The family and rule filters narrow the same queue, and a rule id is accepted
/// in any case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn family_and_rule_filters_narrow_the_queue() {
    let (_tmp, engine) = fixture().await;

    let temporal = sweep(
        &engine,
        TODAY,
        EvolveParams {
            families: vec!["temporal".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(
        rules(&temporal),
        vec!["V005", "V001", "V002", "V004", "V006", "V010", "V003"]
    );
    assert_eq!(temporal["total"], 7);
    assert_eq!(
        temporal["scope"]["families"],
        serde_json::json!(["temporal"])
    );

    let redundancy = sweep(
        &engine,
        TODAY,
        EvolveParams {
            families: vec!["Redundancy".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(rules(&redundancy), vec!["V201", "V202", "V203"]);

    let one_rule = sweep(
        &engine,
        TODAY,
        EvolveParams {
            rules: vec!["v001".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(rules(&one_rule), vec!["V001"]);
    assert_eq!(one_rule["scope"]["rules"], serde_json::json!(["V001"]));
    // The legend follows the filter: one rule shown, one instruction.
    assert_eq!(one_rule["actions"].as_array().unwrap().len(), 1);
    // Scanning is unaffected by a filter; only the queue narrows.
    assert_eq!(one_rule["engrams_scanned"], 19);
}

/// `min_priority` drops the low-scoring tail without touching the ranking.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn min_priority_drops_the_low_scoring_tail() {
    let (_tmp, engine) = fixture().await;
    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            // Also the explicitly scoped path, which resolves the name the way
            // every other tool does before sweeping it.
            domains: vec!["eng".to_string()],
            min_priority: Some(70),
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(rules(&v), vec!["V005", "V001", "V201", "V002"]);
    assert_eq!(v["total"], 4);
    assert_eq!(v["scope"]["domains"], serde_json::json!(["eng"]));
    assert_eq!(v["scope"]["min_priority"], 70);
    assert!(
        v["queue"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["priority"].as_u64().unwrap() >= 70)
    );
}

/// An unknown domain, family or rule errors naming the valid set, so a caller
/// recovers in one step. An id outside the catalog errors rather than
/// returning silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_domain_family_and_rule_error_with_the_valid_set() {
    let (_tmp, engine) = fixture().await;

    let e = engine
        .evolve_engrams(
            &EvolveParams {
                domains: vec!["nope".to_string()],
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("domain 'nope' not registered"), "{e}");
    assert!(e.contains("eng"), "{e}");

    let e = engine
        .evolve_engrams(
            &EvolveParams {
                families: vec!["lifecycle".to_string()],
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "unknown family 'lifecycle'; valid families: temporal, structure, redundancy"
    );

    let e = engine
        .evolve_engrams(
            &EvolveParams {
                rules: vec!["V999".to_string()],
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("unknown rule 'V999'; valid rules: V001, V002"),
        "{e}"
    );
    // The catalog's last id, so the error names the whole of it.
    assert!(e.ends_with("V301"), "{e}");

    let e = engine
        .evolve_engrams(
            &EvolveParams {
                today: Some("last tuesday".to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(e, "today 'last tuesday' is not an ISO date (YYYY-MM-DD)");
}

/// An engram the sweep cannot read is counted and skipped, never fatal: one
/// unreadable file must not hide every finding behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_engram_is_counted_rather_than_aborting_the_sweep() {
    let (tmp, engine) = fixture().await;
    // Removed after the sync, so the index still lists it while the content is
    // gone - the same shape a file deleted between a sync and a sweep leaves.
    std::fs::remove_file(tmp.path().join("eng").join("stub-note.md")).unwrap();

    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(v["unparsed"], 1);
    assert_eq!(v["engrams_scanned"], 18);
    assert_eq!(v["total"], 15);
    assert!(!rules(&v).contains(&"V106".to_string()));
}

/// The queue renders as one TOON tabular block, which is the whole reason every
/// row is flat: uniform keys and scalar-only cells are exactly what
/// `toon::is_tabular` requires, and the prose instruction lives in the legend
/// rather than in a row. The encoder is crate-private, so the predicate is
/// mirrored here; M5's tool test asserts the rendered block end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queue_rows_stay_tabular_for_toon() {
    let (_tmp, engine) = fixture().await;
    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;

    let rows = v["queue"].as_array().unwrap();
    assert!(!rows.is_empty());
    let first: Vec<&String> = rows[0].as_object().unwrap().keys().collect();
    assert_eq!(
        first,
        vec![
            "class",
            "domain",
            "evidence",
            "finding",
            "fix",
            "line",
            "n",
            "permalink",
            "priority",
            "rule",
            "title",
        ]
    );
    for row in rows {
        let obj = row.as_object().unwrap();
        assert_eq!(
            obj.keys().collect::<Vec<_>>(),
            first,
            "every row needs the same keys in the same order"
        );
        for (key, cell) in obj {
            assert!(
                cell.is_null() || cell.is_boolean() || cell.is_number() || cell.is_string(),
                "cell {key} is not a scalar: {cell}"
            );
        }
    }

    // The legend is tabular too, and so is the family summary.
    for list in ["actions", "families"] {
        for row in v[list].as_array().unwrap() {
            assert!(row.as_object().unwrap().values().all(|c| !c.is_array()));
        }
    }
}

// --- the run recorder --------------------------------------------------------

/// `evolve_engrams` is `evolve_detect` plus exactly one side effect: it stamps
/// the maintenance state file so the Stop hook stops nudging about the domains
/// this sweep just looked at. Both halves are asserted in one test because
/// they share one state file, and a sweep that recorded nothing would pass a
/// detection-only assertion made anywhere else.
///
/// The state directory is redirected into a scratch home for the duration, so
/// the run never touches the developer's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_run_recorder_stamps_a_sweep_and_leaves_detection_pure() {
    // Both assertions here are about the whole file - its exact bytes, and the
    // exact backlog left after a scoped sweep - so this test needs the state
    // to itself while it runs. See `support::maintenance_guard`.
    let _serialized = support::maintenance_guard().await;
    let scratch = support::ScratchStateDir::acquire();
    let (_tmp, engine) = fixture().await;

    // Two domains owe a sweep; the run below is scoped to one of them.
    crystalline_service::maintenance::record_pending("eng");
    crystalline_service::maintenance::record_pending("ops");
    let started = crystalline_service::maintenance::load().pending_since;
    assert!(started.is_some(), "the backlog carries its start");
    let before = std::fs::read(scratch.maintenance_path()).unwrap();

    // Detection changes nothing at all, which is what lets a queue view show
    // this page without claiming anybody worked it.
    engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["eng".to_string()],
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(scratch.maintenance_path()).unwrap(),
        before,
        "detection must not write the maintenance state"
    );

    let v = engine
        .evolve_engrams(
            &EvolveParams {
                domains: vec!["eng".to_string()],
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(v["scope"]["domains"], serde_json::json!(["eng"]));

    let state = crystalline_service::maintenance::load();
    assert!(state.last_run_at.is_some(), "the run was stamped");
    assert_eq!(
        state.pending_domains,
        vec!["ops".to_string()],
        "only the swept domain leaves the backlog"
    );
    assert_eq!(
        state.pending_since, started,
        "what is left keeps the age it had"
    );
    assert!(
        scratch.maintenance_path().starts_with(scratch.home()),
        "the state file must land in the scratch home, never the developer's"
    );
}

/// An unscoped sweep empties the whole backlog, including a name no scope can
/// ever cover again.
///
/// That name is the ghost this heals: a domain a human wrote to through Fluid
/// and then unregistered stays on the pending list for ever if the recorder
/// only ever subtracts the domains it swept, and the Stop hook would keep
/// naming it. A sweep with no scope looked at every registered domain, so what
/// is left over is by definition unreachable and the run settles it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unscoped_run_settles_the_whole_backlog_including_a_ghost() {
    // "A full sweep leaves no ghost behind" is a claim about the whole file, so
    // it takes the same exclusivity as the test above.
    let _serialized = support::maintenance_guard().await;
    let _scratch = support::ScratchStateDir::acquire();
    let (_tmp, engine) = fixture().await;

    // `eng` is the only registered domain, so `ghost` can never be swept.
    crystalline_service::maintenance::record_pending("eng");
    crystalline_service::maintenance::record_pending("ghost");

    let v = engine
        .evolve_engrams(
            &EvolveParams {
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        v["scope"]["domains"],
        serde_json::json!(["eng"]),
        "the unscoped sweep covered every registered domain"
    );

    let state = crystalline_service::maintenance::load();
    assert!(state.last_run_at.is_some(), "the run was stamped");
    assert!(
        state.pending_domains.is_empty(),
        "a full sweep leaves no ghost behind: {:?}",
        state.pending_domains
    );
    assert_eq!(
        state.pending_since, None,
        "an empty backlog carries no age to nudge about"
    );
}

// --- attachments -------------------------------------------------------------

/// A domain whose attachments are the point: one file an engram shows but
/// nobody captured, one an engram claims with the hash it had when it was read
/// (now wrong), one reference to a file that is not there and one file nothing
/// mentions at all.
///
/// Its own fixture rather than four more files in the one above, because the
/// grace period on `V007` and `V108` is measured against the file's mtime and
/// a file written by a test is always modified today: the two dates below are
/// what moves the sweep to either side of that, and mixing them into the
/// catalog fixture would make its every-rule assertion depend on the clock.
async fn attachment_fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("att");
    std::fs::create_dir_all(dir.join("assets")).unwrap();

    let files: Vec<(&str, String)> = vec![
        (
            "MANIFEST.md",
            "---\ntype: manifest\ntitle: att\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\n# att\n\n## Scope\n\n- Everything with a file attached\n\n## When to Use\n\n- Route here for the attachment rules\n".to_string(),
        ),
        // V007: shown to a reader, claimed by nobody.
        (
            "shows-deck.md",
            "---\ntype: engram\ntitle: Shows the deck\npermalink: shows-deck\ntags:\n  - decks\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nThe quarter's numbers are in the deck below.\n\n![Deck](assets/deck.png)\n\n- [context] the deck came out of the review\n".to_string(),
        ),
        // V008: a claim in the frontmatter carrying the hash the file had when
        // it was read. The file's real hash is different, and reading the claim
        // at all is what proves the sweep sees a file domain's frontmatter.
        (
            "captured-shot.md",
            "---\ntype: engram\ntitle: What the shot shows\npermalink: captured-shot\ntags:\n  - decks\nstatus: stable\nrecorded_at: 2026-07-25\nanalyzes: assets/shot.png\nanalyzed_hash: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n---\n\nThe screenshot shows the queue draining after the restart.\n\n- [context] read out of the incident channel\n- [lesson] the drain is not instant\n".to_string(),
        ),
        // V107: a body reference to a file the domain does not hold.
        (
            "ghost-ref.md",
            "---\ntype: engram\ntitle: Points at a ghost\npermalink: ghost-ref\ntags:\n  - decks\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nThe diagram used to live beside this text.\n\n[Diagram](assets/gone.png)\n\n- [context] the file left with a folder move\n".to_string(),
        ),
    ];
    for (rel, body) in files {
        std::fs::write(dir.join(rel), body).unwrap();
    }
    for name in ["deck.png", "shot.png", "stray.png"] {
        std::fs::write(
            dir.join("assets").join(name),
            format!("PNG bytes of {name}"),
        )
        .unwrap();
    }

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("att".to_string(), DomainEntry::file(dir));
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// The four attachment rules over a real file domain: the rows come from the
/// walker, the claims come from the frontmatter on disk and the grace period
/// comes from the files' own mtimes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_attachment_rules_fire_over_a_real_domain() {
    let (_tmp, engine) = attachment_fixture().await;
    let attachment_rules = vec![
        "V007".to_string(),
        "V008".to_string(),
        "V107".to_string(),
        "V108".to_string(),
    ];

    // Long after the files were written, so both grace periods have passed.
    let v = sweep(
        &engine,
        "2099-01-01",
        EvolveParams {
            domains: vec!["att".to_string()],
            rules: attachment_rules.clone(),
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;

    let by_rule = |rule: &str| -> Value {
        v["queue"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["rule"] == rule)
            .cloned()
            .unwrap_or_else(|| panic!("no {rule} in {:?}", rules(&v)))
    };
    assert_eq!(v["total"], 4, "one of each: {:?}", rules(&v));

    assert_eq!(by_rule("V007")["permalink"], "shows-deck");
    assert!(
        by_rule("V007")["evidence"]
            .as_str()
            .unwrap()
            .starts_with("assets/deck.png; image/png, "),
        "{}",
        by_rule("V007")["evidence"]
    );
    assert!(
        by_rule("V007")["evidence"]
            .as_str()
            .unwrap()
            .ends_with("no engram claims it via analyzes")
    );

    // The claim and its hash were read off the file's frontmatter, which the
    // index never stores for a file domain.
    assert_eq!(by_rule("V008")["permalink"], "captured-shot");
    assert!(
        by_rule("V008")["evidence"].as_str().unwrap().starts_with(
            "analyzes assets/shot.png; analyzed_hash 01234567.. but the attachment is now "
        ),
        "{}",
        by_rule("V008")["evidence"]
    );

    assert_eq!(by_rule("V107")["permalink"], "ghost-ref");
    assert_eq!(by_rule("V107")["fix"], "assets/gone.png");

    // The orphan carries the path as its subject and no engram address, so
    // nothing renders it as a link to knowledge that does not exist.
    assert_eq!(by_rule("V108")["permalink"], "");
    assert_eq!(by_rule("V108")["title"], "assets/stray.png");
    assert_eq!(by_rule("V108")["class"], "judgment");

    // Evaluated before the files existed, the two rules with a grace period go
    // quiet and the two without it do not - the same reproducibility the other
    // temporal rules get from `today`.
    let early = sweep(
        &engine,
        "2020-01-01",
        EvolveParams {
            domains: vec!["att".to_string()],
            rules: attachment_rules,
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    let mut fired = rules(&early);
    fired.sort();
    assert_eq!(fired, vec!["V008", "V107"]);
}

// ---------------------------------------------------------------------------
// Acknowledgments
// ---------------------------------------------------------------------------

/// One `evolve_ack` assignment as an agent makes it: `edit_engram` with
/// `set_frontmatter`, key `evolve_ack`, the whole value as one string. Left
/// unwrapped so the tests that assert on a refusal can read the message.
async fn ack_edit(engine: &Engine, permalink: &str, value: &str) -> Result<Value, String> {
    engine
        .edit_engram_as(
            &crystalline_service::params::EditParams {
                identifier: permalink.to_string(),
                domain: "eng".to_string(),
                operation: "set_frontmatter".to_string(),
                key: Some("evolve_ack".to_string()),
                value: Some(value.to_string()),
                ..Default::default()
            },
            Some("agent:test"),
            &Scope::Unrestricted,
        )
        .await
        .map_err(|e| e.to_string())
}

/// Acknowledge `rule` on `permalink` the way an agent does: `edit_engram` with
/// `set_frontmatter`, key `evolve_ack`, the rule and note as one value.
async fn acknowledge(engine: &Engine, permalink: &str, value: &str) -> Value {
    ack_edit(engine, permalink, value).await.unwrap()
}

/// The queue rows for one engram, whatever the rule.
fn rows_on<'a>(v: &'a Value, permalink: &str) -> Vec<&'a Value> {
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["permalink"] == permalink)
        .collect()
}

/// The whole acknowledgment lifecycle over a real file domain: the write path
/// computes the scope from detection, the entry lands in the file with the
/// caller's identity and an instant, the finding leaves the queue counted, and
/// the same rule acknowledged twice replaces rather than doubles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_acknowledged_finding_leaves_the_queue_counted() {
    let (tmp, engine) = fixture().await;

    let receipt = acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;
    let entry = &receipt["evolve_ack"];
    assert_eq!(entry["rule"], "V101");
    assert_eq!(
        entry["scope"], "eng/retired-thing",
        "the server computed what the finding fired on"
    );
    assert_eq!(entry["note"], "lineage citation, keep");
    assert_eq!(entry["by"], "agent:test");
    assert!(entry["at"].as_str().unwrap().contains('T'), "{entry}");

    // It lives in the file, which is what makes it travel and survive a resync.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/live-doc.md")).unwrap();
    assert!(on_disk.contains("evolve_ack:"), "{on_disk}");
    assert!(on_disk.contains("rule: V101"), "{on_disk}");

    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert!(
        !rows_on(&v, "live-doc").iter().any(|r| r["rule"] == "V101"),
        "the acknowledged finding is gone from the queue"
    );
    assert_eq!(v["acknowledged"]["total"], 1);
    assert_eq!(v["acknowledged"]["by_family"]["structure"], 1);
    assert_eq!(v["acknowledged"]["by_family"]["temporal"], 0);

    // Acknowledged again, with a different note: one entry, not two.
    let second = acknowledge(&engine, "live-doc", "V101 still deliberate").await;
    assert_eq!(second["evolve_ack"]["note"], "still deliberate");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/live-doc.md")).unwrap();
    assert_eq!(on_disk.matches("rule: V101").count(), 1, "{on_disk}");

    // And the audit view returns the row it suppressed, marked, with the note.
    let audited = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            include_acknowledged: true,
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    let row = rows_on(&audited, "live-doc")[0];
    assert_eq!(row["acknowledged"], true);
    assert_eq!(row["ack_note"], "still deliberate");
    assert!(row.get("ack_stale").is_none(), "{row}");
}

/// A rule that is not firing is acknowledged scope-less, which is the generous
/// entry that matches whatever it finds later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledging_a_rule_that_is_not_firing_stores_no_scope() {
    let (_tmp, engine) = fixture().await;
    let receipt = acknowledge(&engine, "live-doc", "V104").await;
    assert_eq!(receipt["evolve_ack"]["rule"], "V104");
    assert_eq!(receipt["evolve_ack"]["scope"], Value::Null);
    assert_eq!(receipt["evolve_ack"]["note"], Value::Null);
}

/// A rule id the catalog does not hold is refused rather than stored: an
/// acknowledgment that can never suppress anything would read as work done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_rule_is_refused() {
    let (_tmp, engine) = fixture().await;
    let err = engine
        .edit_engram_as(
            &crystalline_service::params::EditParams {
                identifier: "live-doc".to_string(),
                domain: "eng".to_string(),
                operation: "set_frontmatter".to_string(),
                key: Some("evolve_ack".to_string()),
                value: Some("V999 nope".to_string()),
                ..Default::default()
            },
            None,
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("V999"), "{err}");
    assert!(err.contains("V101"), "the catalog is named: {err}");
}

/// The evidence changes, so the acknowledgment stops matching and the finding
/// comes back marked stale carrying the old note - never silently forgotten.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ack_whose_evidence_changed_comes_back_stale() {
    let (tmp, engine) = fixture().await;
    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;

    // A second retired target: the same rule, different evidence.
    let path = tmp.path().join("eng/live-doc.md");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace(
            "- relates_to [[Retired thing]]",
            "- relates_to [[Retired thing]]\n- relates_to [[Old deploy pipeline]]",
        ),
    )
    .unwrap();
    // The old pipeline is what V005 says should have been retired; retire it so
    // the second link really points at retired knowledge.
    let old = tmp.path().join("eng/deploy/old-pipeline.md");
    let source = std::fs::read_to_string(&old).unwrap();
    std::fs::write(&old, source.replace("status: stable", "status: deprecated")).unwrap();
    engine.sync(None).await.unwrap();

    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    let row = rows_on(&v, "live-doc")[0];
    assert_eq!(row["ack_stale"], true);
    assert_eq!(row["ack_note"], "lineage citation, keep");
    assert_eq!(
        row["ack_scope"], "eng/retired-thing",
        "the row says what was acknowledged"
    );
    // And the finding's own columns say what it fires on now, so a reader sees
    // both sides of the drift rather than one of them twice.
    assert!(
        row["evidence"]
            .as_str()
            .unwrap()
            .contains("eng/deploy/old-pipeline"),
        "{row}"
    );
    assert!(
        row["evidence"]
            .as_str()
            .unwrap()
            .contains("eng/retired-thing"),
        "{row}"
    );
    assert_eq!(v["acknowledged"]["total"], 0, "nothing was suppressed");

    // Re-acknowledged, it takes the new evidence and goes quiet again.
    let again = acknowledge(&engine, "live-doc", "V101 both are deliberate").await;
    assert_eq!(
        again["evolve_ack"]["scope"], "eng/deploy/old-pipeline, eng/retired-thing",
        "the scope is the sorted set of what it now points at"
    );
    let after = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert!(rows_on(&after, "live-doc").is_empty(), "{after}");
    assert_eq!(after["acknowledged"]["total"], 1);
}

/// A single-scope rule keeps exactly one acknowledgment however often its
/// evidence moves, so the drift row survives a re-acknowledgment: acknowledge,
/// drift, re-acknowledge, drift again, and the finding is still stale and still
/// carries the note somebody wrote for it.
///
/// The pair-scoped `V301` is the one rule that stores a second entry, and it
/// stores it per pair. Every other rule replacing its entry is what keeps this
/// path working: two entries for one rule would leave the second drift with no
/// entry to point at and downgrade it to a fresh finding, losing "somebody
/// ruled this intentional and the evidence has since changed" for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_re_acknowledged_rule_keeps_one_entry_and_stays_stale_on_the_next_drift() {
    let (tmp, engine) = fixture().await;
    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;

    // First drift: a second retired target.
    let path = tmp.path().join("eng/live-doc.md");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace(
            "- relates_to [[Retired thing]]",
            "- relates_to [[Retired thing]]\n- relates_to [[Old deploy pipeline]]",
        ),
    )
    .unwrap();
    let old = tmp.path().join("eng/deploy/old-pipeline.md");
    let source = std::fs::read_to_string(&old).unwrap();
    std::fs::write(&old, source.replace("status: stable", "status: deprecated")).unwrap();
    engine.sync(None).await.unwrap();

    // Re-acknowledged on the new evidence: one entry in the file, the fresh one.
    acknowledge(&engine, "live-doc", "V101 both are deliberate").await;
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        on_disk.matches("rule: V101").count(),
        1,
        "one entry per single-scope rule: {on_disk}"
    );

    // Second drift: a third retired target.
    std::fs::write(
        tmp.path().join("eng/legacy-note.md"),
        "---\ntype: engram\ntitle: Legacy note\npermalink: legacy-note\ntags:\n  - legacy-notes\nstatus: superseded\nrecorded_at: 2026-07-25\n---\n\nKept only for the record.\n\n- [context] nothing points here any more\n",
    )
    .unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace(
            "- relates_to [[Old deploy pipeline]]",
            "- relates_to [[Old deploy pipeline]]\n- relates_to [[Legacy note]]",
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    let row = rows_on(&v, "live-doc")[0];
    assert_eq!(row["ack_stale"], true, "{row}");
    assert_eq!(row["ack_note"], "both are deliberate");
    assert_eq!(
        row["ack_scope"], "eng/deploy/old-pipeline, eng/retired-thing",
        "the row says what was acknowledged, not what it fires on now"
    );
    assert_eq!(v["acknowledged"]["total"], 0, "nothing was suppressed");
}

/// A hand-written entry with no scope suppresses whatever the rule finds, and
/// withdrawing it brings the finding straight back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hand_written_ack_holds_until_it_is_withdrawn() {
    let (tmp, engine) = fixture().await;
    let path = tmp.path().join("eng/live-doc.md");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace(
            "status: stable",
            "status: stable\nevolve_ack:\n- { rule: V101, note: kept by hand, by: \"human:jordi\" }",
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            ..EvolveParams::default()
        },
    )
    .await;
    assert!(rows_on(&v, "live-doc").is_empty(), "{v}");
    assert_eq!(v["acknowledged"]["total"], 1);

    let removed = engine
        .unacknowledge_finding_as(
            "eng",
            "live-doc",
            "v101",
            None,
            Some("human:jordi"),
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(removed);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("evolve_ack"), "{on_disk}");

    let back = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(rows_on(&back, "live-doc").len(), 1);
    assert_eq!(back["acknowledged"]["total"], 0);

    // Withdrawing what is not there reports exactly that, rather than a
    // rewrite that changed nothing.
    assert!(
        !engine
            .unacknowledge_finding_as(
                "eng",
                "live-doc",
                "V101",
                None,
                None,
                &crystalline_service::Scope::Unrestricted,
            )
            .await
            .unwrap()
    );
}

/// A note pasted out of a chat window carries newlines. They are folded to
/// single spaces at intake, so the stored entry is one line of prose and the
/// engram it lands in still parses - the corruption path a raw `\n---\n` in a
/// note would otherwise open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_note_with_newlines_is_folded_and_the_engram_still_parses() {
    let (tmp, engine) = fixture().await;
    let receipt = acknowledge(
        &engine,
        "live-doc",
        "V101 first line\n---\ntype: injected\nstatus: evil",
    )
    .await;
    assert_eq!(
        receipt["evolve_ack"]["note"], "first line --- type: injected status: evil",
        "the newlines are folded to spaces rather than written into the file"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/live-doc.md")).unwrap();
    assert_eq!(
        on_disk.lines().filter(|l| l.trim() == "---").count(),
        2,
        "the note never opens a second frontmatter block: {on_disk}"
    );
    crystalline_core::parse_engram(&on_disk).expect("the engram still parses");

    // And the sweep still sees it: an engram nothing can parse is invisible.
    engine.sync(None).await.unwrap();
    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(v["unparsed"], 0, "{v}");
    assert_eq!(v["acknowledged"]["total"], 1);

    // The second acknowledgment - the ordinary re-acknowledge flow - rewrites
    // the block rather than orphaning a continuation line.
    acknowledge(&engine, "live-doc", "V104 also deliberate").await;
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/live-doc.md")).unwrap();
    crystalline_core::parse_engram(&on_disk).expect("the rewrite parses too");
}

/// An acknowledgment onto an engram that no longer parses is refused rather
/// than appended: stacking a second `evolve_ack` key onto broken frontmatter
/// compounds the damage instead of reporting it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ack_refuses_an_engram_that_no_longer_parses() {
    let (tmp, engine) = fixture().await;
    let path = tmp.path().join("eng/live-doc.md");
    // Broken on disk but still indexed, which is exactly the state a hand edit
    // or an older corruption leaves behind.
    let broken = "---\ntitle: Live doc\npermalink: live-doc\nstatus: \"unclosed\n---\n\nBody.\n";
    std::fs::write(&path, broken).unwrap();

    let err = engine
        .edit_engram_as(
            &crystalline_service::params::EditParams {
                identifier: "live-doc".to_string(),
                domain: "eng".to_string(),
                operation: "set_frontmatter".to_string(),
                key: Some("evolve_ack".to_string()),
                value: Some("V101 keep".to_string()),
                ..Default::default()
            },
            Some("agent:test"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("parse"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        broken,
        "a refused acknowledgment writes nothing at all"
    );
}

/// A hand-written engram spells its provenance as a block mapping, which is
/// what YAML invites and what the parser reads back happily. Both an ordinary
/// edit and an acknowledgment refresh that provenance on the way to disk, and
/// neither may cost the engram its parse.
///
/// This pins the reachability the ack path cannot defend on its own: the merge
/// is parse-validated inside `apply`, and `touch_generated` then rewrites the
/// blessed bytes before they are written, so the only place the block form can
/// be handled correctly is the emitter. An engram that stops parsing here goes
/// invisible to the sweep, to reads and to search - the exact failure the ack
/// validation was built to prevent, arriving one step later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_and_an_ack_both_survive_a_block_form_generated_mapping() {
    let (tmp, engine) = fixture().await;
    let path = tmp.path().join("eng/live-doc.md");
    let source = std::fs::read_to_string(&path).unwrap();
    let hand_written = source.replace(
        "status: stable",
        "generated:\n  by: human:jordi\n  at: 2026-01-01T00:00:00+00:00\nstatus: stable",
    );
    assert!(hand_written.contains("by: human:jordi"), "{hand_written}");
    std::fs::write(&path, &hand_written).unwrap();
    engine.sync(None).await.unwrap();

    // (a) an ordinary edit.
    engine
        .edit_engram_as(
            &crystalline_service::params::EditParams {
                identifier: "live-doc".to_string(),
                domain: "eng".to_string(),
                operation: "append".to_string(),
                content: Some("One more line.".to_string()),
                ..Default::default()
            },
            Some("agent:test"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let on_disk = std::fs::read_to_string(&path).unwrap();
    let e = crystalline_core::parse_engram(&on_disk).expect("the edited engram still parses");
    assert_eq!(e.frontmatter.generated.as_ref().unwrap().by, "agent:test");
    assert!(!on_disk.contains("human:jordi"), "{on_disk}");
    assert_eq!(on_disk.matches("generated:").count(), 1, "{on_disk}");

    // (b) the acknowledgment path, on the same hand-written shape again.
    std::fs::write(&path, &hand_written).unwrap();
    engine.sync(None).await.unwrap();

    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;
    let on_disk = std::fs::read_to_string(&path).unwrap();
    let e = crystalline_core::parse_engram(&on_disk).expect("the acknowledged engram still parses");
    assert_eq!(e.frontmatter.generated.as_ref().unwrap().by, "agent:test");
    assert!(!on_disk.contains("human:jordi"), "{on_disk}");
    assert!(on_disk.contains("evolve_ack:"), "{on_disk}");

    // And the sweep still sees it: an engram nothing can parse is invisible.
    engine.sync(None).await.unwrap();
    let v = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(v["unparsed"], 0, "{v}");
}

/// The audit view carries the scope the acknowledgment was given for, which is
/// what lets a reader see why a stale one stopped matching.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_audit_row_carries_the_acknowledged_scope() {
    let (_tmp, engine) = fixture().await;
    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;
    let audited = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            include_acknowledged: true,
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    let row = rows_on(&audited, "live-doc")[0];
    assert_eq!(row["acknowledged"], true);
    assert_eq!(row["ack_scope"], "eng/retired-thing");
    assert_eq!(row["ack_note"], "lineage citation, keep");
}

/// A hand-written entry spells its rule id however the person typed it, so
/// withdrawing one folds case exactly as matching it does. Without this the
/// Unacknowledge action is permanently broken for that entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lowercase_hand_written_rule_id_can_still_be_withdrawn() {
    let (tmp, engine) = fixture().await;
    let path = tmp.path().join("eng/live-doc.md");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace(
            "status: stable",
            "status: stable\nevolve_ack:\n- { rule: v101, note: kept by hand, by: \"human:jordi\" }",
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    assert!(
        engine
            .unacknowledge_finding_as(
                "eng",
                "live-doc",
                "V101",
                None,
                Some("human:jordi"),
                &crystalline_service::Scope::Unrestricted,
            )
            .await
            .unwrap()
    );
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("evolve_ack"), "{on_disk}");
}

/// The removal value form takes one acknowledgment back and leaves the other
/// entries exactly as they were, which is what makes it usable on an engram
/// that carries a considered list rather than a single entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removal_value_takes_one_acknowledgment_back() {
    let (tmp, engine) = fixture().await;
    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;
    acknowledge(&engine, "live-doc", "V104 deliberate orphan").await;

    let receipt = ack_edit(&engine, "live-doc", "remove V101").await.unwrap();
    assert_eq!(
        receipt["evolve_ack_removed"], "V101",
        "the receipt names what it took back: {receipt}"
    );
    assert!(
        receipt["evolve_ack"].is_null(),
        "and records nothing: {receipt}"
    );

    let on_disk = std::fs::read_to_string(tmp.path().join("eng/live-doc.md")).unwrap();
    assert!(!on_disk.contains("rule: V101"), "{on_disk}");
    assert!(
        on_disk.contains("rule: V104"),
        "the other one stays: {on_disk}"
    );

    // The finding it silenced is back in the queue, which is the whole point.
    let back = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert_eq!(rows_on(&back, "live-doc").len(), 1, "{back}");
}

/// Taking the last acknowledgment back removes the frontmatter key rather than
/// leaving an empty one behind: asserted by re-parsing the file, because an
/// `evolve_ack:` with nothing under it is what an emitter bug would leave.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_the_last_acknowledgment_removes_the_frontmatter_block() {
    let (tmp, engine) = fixture().await;
    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;

    ack_edit(&engine, "live-doc", "remove v101").await.unwrap();

    let path = tmp.path().join("eng/live-doc.md");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    let parsed = crystalline_core::parse_engram(&on_disk).expect("the engram still parses");
    assert!(
        parsed.frontmatter.extra.get("evolve_ack").is_none(),
        "the key is gone, not emptied: {on_disk}"
    );
}

/// A removal naming a rule the engram never acknowledged is refused rather
/// than answered as a no-op: an agent that mistyped the rule has to hear it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_an_acknowledgment_that_is_not_there_is_refused() {
    let (_tmp, engine) = fixture().await;
    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;

    let err = ack_edit(&engine, "live-doc", "remove V104")
        .await
        .unwrap_err();
    assert!(err.contains("V104"), "{err}");
    assert!(err.contains("live-doc"), "the engram is named: {err}");
    assert!(err.contains("nothing to remove"), "{err}");
}

/// The removal form is exactly `remove <rule-id>`: no rule, an unknown rule or
/// trailing text are each refused with the form named, so a note meant for a
/// record never lands as a half-read removal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removal_value_takes_exactly_one_rule_id() {
    let (_tmp, engine) = fixture().await;
    acknowledge(&engine, "live-doc", "V101 lineage citation, keep").await;

    let bare = ack_edit(&engine, "live-doc", "remove").await.unwrap_err();
    assert!(bare.contains("remove <rule-id>"), "{bare}");

    let extra = ack_edit(&engine, "live-doc", "remove V101 because I changed my mind")
        .await
        .unwrap_err();
    assert!(extra.contains("remove <rule-id>"), "{extra}");
    assert!(extra.contains("drop the extra text"), "{extra}");

    let unknown = ack_edit(&engine, "live-doc", "remove V999")
        .await
        .unwrap_err();
    assert!(unknown.contains("V999"), "{unknown}");

    // None of it touched the acknowledgment they were aimed at.
    let audited = sweep(
        &engine,
        TODAY,
        EvolveParams {
            domains: vec!["eng".to_string()],
            rules: vec!["V101".to_string()],
            limit: Some(100),
            ..EvolveParams::default()
        },
    )
    .await;
    assert!(rows_on(&audited, "live-doc").is_empty(), "{audited}");
}

/// An `evolve_ack` assignment with no value is where an agent learns the key's
/// two forms, so the refusal names both: the record form it probably meant, and
/// the take-back form it has no other way to discover.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_ack_value_names_both_forms() {
    let (_tmp, engine) = fixture().await;

    let empty = ack_edit(&engine, "live-doc", "   ").await.unwrap_err();
    assert!(
        empty.contains("V101 lineage citation, keep"),
        "the record form is shown: {empty}"
    );
    assert!(
        empty.contains("remove <rule-id>"),
        "and so is the take-back: {empty}"
    );
    assert!(
        empty.contains("take an acknowledgment back"),
        "in words that say what it does: {empty}"
    );
}

/// `remove` is only the removal verb as the whole first token: a note that
/// happens to start with the word still records, because the rule id comes
/// first in the record form.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_note_starting_with_remove_still_records() {
    let (_tmp, engine) = fixture().await;
    let receipt = acknowledge(&engine, "live-doc", "V101 remove this later, not now").await;
    assert_eq!(receipt["evolve_ack"]["rule"], "V101", "{receipt}");
    assert_eq!(
        receipt["evolve_ack"]["note"], "remove this later, not now",
        "{receipt}"
    );
    assert!(receipt["evolve_ack_removed"].is_null(), "{receipt}");
}

/// The instant `alpha.md` is backdated to in [`team_fixture`], as seconds since
/// the epoch: `2026-07-01T00:00:00Z`, a month and a day before [`TODAY`].
///
/// An absolute instant rather than "now minus a month" so the age the sweep
/// computes is the same number on every run and on every machine: the engine
/// supplies `today`, the file supplies the mtime, and both ends of the
/// subtraction are pinned here.
const OLDEST_CHANGE_EPOCH_SECS: u64 = 1_782_864_000;

/// An engine over a TEAM domain holding one unshared engram whose own mtime is
/// [`OLDEST_CHANGE_EPOCH_SECS`].
///
/// Deliberately not the big fixture: `V009` is the one rule whose facts come
/// from outside the index - the working tree measured against the recorded
/// base snapshot - so it needs a domain with an origin, a state file and a
/// tree that disagrees with it, which is a different shape of fixture rather
/// than another engram in the existing one.
///
/// The snapshot is empty, which is what a freshly connected domain has before
/// its first share: every substantive file in the tree is then work the team
/// has not seen.
async fn team_fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("kb");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# kb\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- Route here for team questions\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("alpha.md"),
        "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-06-01\n---\n\nWhat the team already agreed.\n",
    )
    .unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains.insert(
        "kb".to_string(),
        DomainEntry {
            origin: Some(OriginConfig {
                repo: "acme/kb".to_string(),
                path: None,
                branch: None,
                poll_secs: None,
            }),
            ..DomainEntry::file(dir.clone())
        },
    );
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    OriginState::new("acme/kb", "main")
        .save(&root.join("origins").join("kb"))
        .unwrap();

    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            // Load-bearing: the registration and the sweep both read origin
            // state, and it must land in the temp dir rather than in the
            // developer's real state directory.
            .with_origins_dir(root.join("origins")),
    );
    engine.sync(None).await.unwrap();
    // Backdated AFTER the sync, which writes the generated folder index and
    // would otherwise be the newest thing in the tree - and, more to the
    // point, because a sync that touched this file would reset exactly what
    // the rule measures.
    std::fs::File::options()
        .write(true)
        .open(dir.join("alpha.md"))
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(OLDEST_CHANGE_EPOCH_SECS))
        .unwrap();
    (tmp, engine)
}

/// The engine hands the sweep a real domain's unshared work, and `V009` comes
/// back out of it.
///
/// The one test that crosses the seam: the detector's own unit tests build
/// `ShareFacts` by hand and the origin helper's tests stop at the delta, so
/// without this nothing proves that `Engine::share_facts` reaches a registered
/// team domain, walks its tree, and lands on the `share` field of the
/// `SweepInput` this rule reads. Both directions are asserted from the same
/// fixture, because a `share` field wired to a constant would pass either half
/// alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v009_comes_out_of_a_real_team_domains_unshared_work() {
    let (_tmp, engine) = team_fixture().await;

    let v = sweep(&engine, TODAY, EvolveParams::default()).await;
    let row = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["rule"] == "V009")
        .unwrap_or_else(|| panic!("V009 fires on a month-old unshared delta: {v}"));
    assert_eq!(row["domain"], "kb");
    assert_eq!(row["class"], "judgment", "sharing is proposed, never done");
    assert_eq!(row["fix"], "share_changes domain=kb", "{row}");
    // The age is read from the file rather than from the frontmatter, so the
    // evidence names the instant the fixture pinned.
    let evidence = row["evidence"].as_str().unwrap();
    assert!(
        evidence.contains("oldest change 2026-07-01"),
        "the backdated mtime is what the rule aged from: {evidence}"
    );
    assert!(
        evidence.contains("threshold 7 days"),
        "and the window it compared it against: {evidence}"
    );

    // The day after that change, the same domain through the same call path
    // says nothing: the field carries the tree's real age rather than a
    // constant, and the rule only speaks once the work has actually sat.
    let fresh = sweep(&engine, "2026-07-02", EvolveParams::default()).await;
    assert!(
        !rules(&fresh).contains(&"V009".to_string()),
        "a day-old delta is not stale: {fresh}"
    );
}

// --- Task 9: the sweep runs in the invoking actor's dimension ---------------

/// One engram of a reviewed domain, as the team's own files hold it.
fn reviewed(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\n{body}\n"
    )
}

/// A file domain `team` in review mode, with a state directory of its own so a
/// draft can be mirrored.
///
/// Two engrams the team reviewed: `charter` carries a prose link that answers
/// to no permalink and no title anywhere (ledger L308 - a fixture target has to
/// fail both readings or the index resolves it), so it is a `V102` finding for
/// everybody; `runbook` is whole, so it is a finding for nobody until somebody
/// drafts one into it.
async fn review_fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("team");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: team\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\n# team\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for team work\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("charter.md"),
        reviewed(
            "Charter",
            "charter",
            "How the team works, and what [[Nobody Ever Wrote This Down]] would have said.",
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("runbook.md"),
        reviewed("Runbook", "runbook", "How the team restarts the importer."),
    )
    .unwrap();
    // One attachment the team's own text shows a reader, and one nothing
    // references at all, so `V108` has something to say either way.
    std::fs::write(
        dir.join("deck.md"),
        reviewed(
            "Deck",
            "deck",
            "The quarter's numbers are in the deck below.\n\n![Deck](assets/deck.png)",
        ),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    for name in ["deck.png", "stray.png"] {
        std::fs::write(
            dir.join("assets").join(name),
            format!("PNG bytes of {name}"),
        )
        .unwrap();
    }

    let mut cfg = GlobalConfig::default();
    let mut entry = DomainEntry::file(dir);
    entry.review = Some(crystalline_core::config::ReviewMode::Overlay);
    cfg.domains.insert("team".to_string(), entry);
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

/// One account, as an authenticated surface resolves it.
fn account(name: &str) -> Scope {
    Scope::User {
        account: name.to_string(),
        admin: false,
    }
}

/// The permalinks `V102` fires on for one actor, sorted so the assertion is
/// about which engrams carry a dangling reference rather than about rank.
async fn dangling_for(engine: &Engine, scope: &Scope) -> Vec<String> {
    let v = engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["team".to_string()],
                rules: vec!["V102".to_string()],
                limit: Some(50),
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            scope,
        )
        .await
        .unwrap();
    let mut out: Vec<String> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["permalink"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

/// Append one line to an engram as somebody, which in a reviewed domain lands
/// as that person's own draft.
async fn append_as(engine: &Engine, permalink: &str, line: &str, scope: &Scope) {
    let value = engine
        .edit_engram_as(
            &EditParams {
                identifier: permalink.to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some(line.to_string()),
                ..EditParams::default()
            },
            None,
            scope,
        )
        .await
        .unwrap();
    assert_eq!(value["draft"], Value::Bool(true), "{value}");
}

/// A reference that dangles in a draft is a finding for the person drafting it
/// and for nobody else: the sweep reads the domain the way its caller reads it,
/// so the text under review is the author's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_drafts_unresolved_link_is_its_authors_finding_only() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    assert_eq!(
        dangling_for(&engine, &bob).await,
        vec!["charter".to_string()],
        "the reviewed files carry exactly one dangling reference"
    );

    // Alice writes a link nothing answers to into her own draft of the runbook.
    append_as(
        &engine,
        "runbook",
        "See [[How Alice Would Restart It]] for the rewrite.",
        &alice,
    )
    .await;

    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string(), "runbook".to_string()],
        "her own draft's broken reference is hers to fix"
    );
    assert_eq!(
        dangling_for(&engine, &bob).await,
        vec!["charter".to_string()],
        "and nobody else is told about a reference in text they cannot read"
    );

    // An acknowledgment is resolved and recorded in the acknowledger's own
    // dimension, so ruling her draft's reference intentional silences it for
    // her and says nothing to anybody else.
    engine
        .acknowledge_finding_as(
            "team",
            "runbook",
            "V102",
            Some("the rewrite lands with the target"),
            None,
            None,
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string()],
        "her own ruling silences her own finding"
    );
    assert_eq!(
        dangling_for(&engine, &bob).await,
        vec!["charter".to_string()],
        "and the team's sweep never saw it either way"
    );
}

/// A finding about the text the team reviewed is everybody's, whatever anybody
/// is drafting elsewhere - and one actor's acknowledgment of it is their own,
/// because the ack is written into their draft.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_base_finding_shows_for_every_actor() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    let charter = vec!["charter".to_string()];
    assert_eq!(dangling_for(&engine, &alice).await, charter);
    assert_eq!(dangling_for(&engine, &bob).await, charter);
    assert_eq!(
        dangling_for(&engine, &Scope::Unrestricted).await,
        charter,
        "and so does the machine owner, who is drafting nothing"
    );

    // A draft somewhere else changes nothing about a finding on the file.
    append_as(&engine, "runbook", "The importer restarts cleanly.", &alice).await;
    assert_eq!(dangling_for(&engine, &alice).await, charter);
    assert_eq!(dangling_for(&engine, &bob).await, charter);

    // Alice rules the dangling reference intentional. In a reviewed domain that
    // ruling lands in her draft of the charter, so it speaks for her alone.
    engine
        .acknowledge_finding_as(
            "team",
            "charter",
            "V102",
            Some("the target lives outside the archive"),
            None,
            None,
            &alice,
        )
        .await
        .unwrap();
    assert!(
        dangling_for(&engine, &alice).await.is_empty(),
        "her own ruling silences the finding for her"
    );
    assert_eq!(
        dangling_for(&engine, &bob).await,
        charter,
        "and never for somebody who has not read it"
    );
}

/// A date past every attachment's modified stamp, which is what lets the
/// attachment rules speak: a file is left alone on the day it arrives, so the
/// fixture's freshly written assets need a sweep dated after them.
const AFTER_THE_UPLOAD: &str = "2027-01-01";

/// The attachment paths `V108` calls orphaned for one actor, sorted. An
/// attachment finding carries its path as the title, since no engram owns it.
async fn orphans_for(engine: &Engine, scope: &Scope) -> Vec<String> {
    let v = engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["team".to_string()],
                rules: vec!["V108".to_string()],
                limit: Some(50),
                today: Some(AFTER_THE_UPLOAD.to_string()),
                ..EvolveParams::default()
            },
            scope,
        )
        .await
        .unwrap();
    let mut out: Vec<String> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["title"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

/// Delete an engram as somebody, which in a reviewed domain lands as that
/// person's own tombstone: the file stays and the path reads as absent for
/// them alone.
async fn delete_as(engine: &Engine, permalink: &str, scope: &Scope) {
    let value = engine
        .delete_engram_as(
            &crystalline_service::params::DeleteParams {
                identifier: permalink.to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            scope,
        )
        .await
        .unwrap();
    assert_eq!(value["draft"], Value::Bool(true), "{value}");
}

/// An attachment is shared state and deleting one is a shared act, so the
/// question `V108` asks - does anything in this domain reference this file -
/// is asked of the union: what this reader sees plus what the domain still
/// holds. An author who drops a reference in a draft is never told the file is
/// now unused, and a reference only their draft carries counts for them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_draft_dropping_a_reference_never_orphans_a_shared_attachment() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    let stray = vec!["assets/stray.png".to_string()];
    assert_eq!(
        orphans_for(&engine, &bob).await,
        stray,
        "the file nothing points at is the orphan, and the deck is not"
    );

    // Alice drafts the deck reference out of her copy.
    engine
        .edit_engram_as(
            &EditParams {
                identifier: "deck".to_string(),
                domain: "team".to_string(),
                operation: "find_replace".to_string(),
                find_text: Some("![Deck](assets/deck.png)".to_string()),
                content: Some("The deck moved to the shared drive.".to_string()),
                ..EditParams::default()
            },
            None,
            &alice,
        )
        .await
        .unwrap();

    assert_eq!(
        orphans_for(&engine, &alice).await,
        stray,
        "her unreviewed edit is no argument for deleting a file the team's own \
         text still shows"
    );
    assert_eq!(
        orphans_for(&engine, &bob).await,
        stray,
        "and it says nothing to anybody else either"
    );

    // The other direction: a reference only her draft carries answers for her.
    append_as(
        &engine,
        "runbook",
        "The stray shot is worth keeping: ![Stray](assets/stray.png)",
        &alice,
    )
    .await;
    assert!(
        orphans_for(&engine, &alice).await.is_empty(),
        "she is reading text that references it, so it is not unused for her"
    );
    assert_eq!(
        orphans_for(&engine, &bob).await,
        stray,
        "and nothing she has not shared reaches his queue"
    );
}

/// A reference is dangling when it answers to nothing the reader sees. For an
/// author that is base plus their own drafts, so two drafts of theirs answer
/// each other, and a path they have drafted a deletion of answers nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authors_own_drafts_resolve_each_others_links() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    // A draft-only engram of her own, linked from another of her drafts.
    engine
        .write_engram_as(
            &crystalline_service::params::WriteParams {
                domain: "team".to_string(),
                title: "Restart Ladder".to_string(),
                content: "The ladder the importer restart climbs.".to_string(),
                tags: vec!["team".to_string()],
                folder: None,
                engram_type: None,
                status: None,
                metadata: None,
                overwrite: false,
                share_link: None,
                model: None,
            },
            None,
            &alice,
        )
        .await
        .unwrap();
    append_as(
        &engine,
        "runbook",
        "The ladder is in [[Restart Ladder]].",
        &alice,
    )
    .await;
    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string()],
        "her link answers to an engram she is reading, so only the reviewed \
         file's own broken reference is left"
    );

    // And the other way: a path she has drafted a deletion of answers nothing,
    // however well the reviewed folder still answers it for everybody else.
    delete_as(&engine, "charter", &alice).await;
    append_as(&engine, "runbook", "History lives in [[Charter]].", &alice).await;
    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["runbook".to_string()],
        "the charter is gone from her view, so her link to it is the dangling one"
    );
    assert_eq!(
        dangling_for(&engine, &bob).await,
        vec!["charter".to_string()],
        "and the team still reads the charter, with its own broken reference"
    );
}

/// A path an author has drafted a deletion of is absent from their sweep
/// entirely: no engram, and so no finding about it. For everybody else the
/// reviewed file stands, and so does what it is doing wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_an_author_deleted_carries_no_finding_for_them() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string()],
        "the reviewed charter carries a broken reference for everybody"
    );

    delete_as(&engine, "charter", &alice).await;

    assert!(
        dangling_for(&engine, &alice).await.is_empty(),
        "she has deleted the engram the finding was about, so there is nothing \
         left to tell her"
    );
    assert_eq!(
        dangling_for(&engine, &bob).await,
        vec!["charter".to_string()],
        "and the deletion is hers alone until it is reviewed"
    );
}

// --- Task 11c: the sweep reads references in the caller's own view ----------

/// Write one engram as somebody, which in a reviewed domain lands as that
/// person's own draft.
async fn write_as(engine: &Engine, title: &str, body: &str, scope: &Scope) {
    let value = engine
        .write_engram_as(
            &crystalline_service::params::WriteParams {
                domain: "team".to_string(),
                title: title.to_string(),
                content: body.to_string(),
                folder: None,
                engram_type: None,
                tags: vec!["team".to_string()],
                status: None,
                metadata: None,
                overwrite: false,
                share_link: None,
                model: None,
            },
            None,
            scope,
        )
        .await
        .unwrap();
    assert_eq!(value["draft"], Value::Bool(true), "{value}");
}

/// Add one file to the folder the team shares, and index it.
async fn reviewed_file(engine: &Engine, dir: &std::path::Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    engine.sync(None).await.unwrap();
}

/// The rules one actor's sweep fires, for the finding shapes that carry no
/// engram of their own.
async fn rules_for(engine: &Engine, rule: &str, scope: &Scope) -> Vec<String> {
    let v = engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["team".to_string()],
                rules: vec![rule.to_string()],
                limit: Some(50),
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            scope,
        )
        .await
        .unwrap();
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["rule"].as_str().unwrap().to_string())
        .collect()
}

/// A finding about a team link that one reader's own draft answers is not
/// raised for that reader.
///
/// The sweep is what an author runs before sharing, so it has to speak about
/// the archive they are actually reading. Told that the charter's link is
/// broken when the page it names is open in front of them, they would go and
/// write it twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_base_links_finding_that_only_the_readers_draft_resolves_is_not_raised_for_them() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string()],
        "before she writes it, the charter's link is hers to fix like everybody's"
    );

    write_as(
        &engine,
        "Nobody Ever Wrote This Down",
        "- [decision] somebody did after all #team",
        &alice,
    )
    .await;

    assert!(
        dangling_for(&engine, &alice).await.is_empty(),
        "she has answered the team's link, so it is no longer her finding"
    );
    assert_eq!(
        dangling_for(&engine, &bob).await,
        vec!["charter".to_string()],
        "and it is still everybody else's, because her page is hers alone"
    );
}

/// A finding about a team link into a page one reader has deleted is raised for
/// that reader and for nobody else.
///
/// The other end of the same sentence. The file is whole and the link lands for
/// the team; for her the page it names is gone, and a sweep that said otherwise
/// would be reading somebody else's archive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_base_link_into_a_path_the_reader_deleted_is_raised_for_them_alone() {
    let (tmp, engine) = review_fixture().await;
    let dir = tmp.path().join("team");
    let alice = account("alice");
    let bob = account("bob");
    reviewed_file(
        &engine,
        &dir,
        "guide.md",
        &reviewed(
            "Guide",
            "guide",
            "Start at the runbook.\n\n- relates_to [[Runbook]]",
        ),
    )
    .await;

    let charter = vec!["charter".to_string()];
    assert_eq!(
        dangling_for(&engine, &alice).await,
        charter,
        "the guide's link lands for everybody while the runbook is there"
    );

    engine
        .delete_engram_as(
            &crystalline_service::params::DeleteParams {
                identifier: "runbook".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &alice,
        )
        .await
        .unwrap();

    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string(), "guide".to_string()],
        "her deletion is what broke the guide's link, and only for her"
    );
    assert_eq!(
        dangling_for(&engine, &bob).await,
        charter,
        "the team's own guide still points at the team's own runbook"
    );
}

/// A draft titled with a colon answers its author's own link, in the sweep.
///
/// `[[Log: Weekly]]` splits like `[[domain:Target]]` and only the registry
/// settles it, which is the reading the sweep's own draft pass never had: it
/// re-implemented the permalink and title forms in Rust and stopped there. It
/// is one resolver now, so the form the index has always understood is the form
/// the finding understands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_colon_titled_draft_answers_a_drafts_link_in_the_sweep() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");

    append_as(
        &engine,
        "runbook",
        "The week's numbers are in [[Log: Weekly]].",
        &alice,
    )
    .await;
    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string(), "runbook".to_string()],
        "her draft names a page nobody has written"
    );

    write_as(
        &engine,
        "Log: Weekly",
        "- [fact] what the week did #team",
        &alice,
    )
    .await;

    assert_eq!(
        dangling_for(&engine, &alice).await,
        vec!["charter".to_string()],
        "and now she has written it, under the title she linked to"
    );
}

/// The tag drift rule reads the tags its reader sees; the `vocabulary` tool
/// goes on answering the team's own list.
///
/// The two halves of the Task 9 ruling, and only one of them held. What a
/// person is SHOWN is the domain's agreement, because a word one author is
/// trying out in a draft is not the team's vocabulary. But a drift FINDING is
/// about what that author wrote, and read off the team's list it could only
/// ever say nothing about the spelling they had just invented.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v203_reads_the_tags_the_author_sees() {
    let (tmp, engine) = review_fixture().await;
    let dir = tmp.path().join("team");
    let alice = account("alice");
    let bob = account("bob");
    std::fs::write(
        dir.join("schema.md"),
        "---\ntype: engram\ntitle: Schema\npermalink: schema\ntags:\n  - database\nstatus: stable\nrecorded_at: 2026-07-25\n---\n\nHow the tables are laid out.\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    assert!(
        rules_for(&engine, "V203", &alice).await.is_empty(),
        "the team spells it one way and nothing has drifted"
    );

    append_as(
        &engine,
        "runbook",
        "- [decision] the importer writes straight to the store #data-base",
        &alice,
    )
    .await;

    assert_eq!(
        rules_for(&engine, "V203", &alice).await,
        vec!["V203".to_string()],
        "her own draft is where the second spelling is, so the drift is hers"
    );
    assert!(
        rules_for(&engine, "V203", &bob).await.is_empty(),
        "and nobody else is told about a word they cannot read"
    );

    let listed = async |scope: &Scope| {
        engine
            .vocabulary(
                &crystalline_service::params::VocabularyParams {
                    domain: Some("team".to_string()),
                },
                scope,
            )
            .await
            .unwrap()["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    for scope in [&alice, &bob] {
        assert!(
            listed(scope).await.contains(&"database".to_string()),
            "the tool answers the team's own vocabulary"
        );
        assert!(
            !listed(scope).await.contains(&"data-base".to_string()),
            "and never a word one person is trying out in a draft"
        );
    }
}

/// The attachment paths `V107` calls dangling for one actor, sorted. The
/// finding carries the path it could not find as its `fix`.
async fn dangling_attachments_for(engine: &Engine, scope: &Scope) -> Vec<String> {
    let v = engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["team".to_string()],
                rules: vec!["V107".to_string()],
                limit: Some(50),
                today: Some(TODAY.to_string()),
                ..EvolveParams::default()
            },
            scope,
        )
        .await
        .unwrap();
    let mut out: Vec<String> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["fix"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

/// **A file only one actor holds is theirs to be orphaned**, and nobody
/// else's to hear about.
///
/// The orphan rule reads the actor view, so a file somebody drafted and
/// referenced nowhere is reported to its author - which is the only person who
/// could do anything about it - and is not in the team's list at all, because
/// the team does not have it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v108_orphans_an_overlay_only_file_for_its_actor_alone() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    let team_only = vec!["assets/stray.png".to_string()];
    assert_eq!(orphans_for(&engine, &alice).await, team_only);
    assert_eq!(orphans_for(&engine, &bob).await, team_only);

    let written = engine
        .attachment_write_as(
            "team",
            "assets/mine.png",
            b"PNG bytes of mine".to_vec(),
            &alice,
        )
        .await
        .unwrap();
    assert!(written.draft, "the upload landed as alice's draft");

    assert_eq!(
        orphans_for(&engine, &alice).await,
        vec![
            "assets/mine.png".to_string(),
            "assets/stray.png".to_string()
        ],
        "her own unreferenced file is hers to answer for, beside the team's"
    );
    assert_eq!(
        orphans_for(&engine, &bob).await,
        team_only,
        "and the team is told nothing about a file the team does not have"
    );
}

/// **A file an actor deleted in review mode reads as absent for them**, and
/// that absence speaks only about the text they are reading.
///
/// Her deletion is a draft like any other: the file is still the team's until
/// the deletion is reviewed. So `V107` tells HER that the reviewed page still
/// showing the deck points at a file she no longer has - which is the finding
/// that would make her fix the page before she shares the deletion - and says
/// nothing of the sort to anybody else. And `V108` never calls the deck an
/// orphan for anybody: for her it is not a file she holds at all, and for the
/// team it is referenced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tombstoned_base_attachment_is_absent_for_the_actor_and_v107_says_so_for_their_text_only()
{
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    assert!(dangling_attachments_for(&engine, &alice).await.is_empty());
    let team_only = vec!["assets/stray.png".to_string()];
    assert_eq!(orphans_for(&engine, &bob).await, team_only);

    let draft = engine
        .attachment_delete_as("team", "assets/deck.png", &alice)
        .await
        .unwrap();
    assert!(draft, "the deletion landed as alice's draft");

    assert_eq!(
        dangling_attachments_for(&engine, &alice).await,
        vec!["assets/deck.png".to_string()],
        "the page she reads shows a file she has taken away"
    );
    assert!(
        dangling_attachments_for(&engine, &bob).await.is_empty(),
        "and nobody else's reading of the same page changed"
    );
    assert_eq!(
        orphans_for(&engine, &alice).await,
        team_only,
        "a file she is not holding is not a file of hers to be orphaned"
    );
    assert_eq!(
        orphans_for(&engine, &bob).await,
        team_only,
        "and the team's sweep is what it was"
    );
}

/// **The union rule survived the listing change.**
///
/// Task 9 pinned it against the base attachment listing: an author who drafts a
/// reference away is never told the shared file is now unused. `V108` reads the
/// actor view now, so the same claim has to be pinned again with an overlay
/// file standing beside the shared one - the half that reads the actor's own
/// dimension is the LISTING, and the half that asks "does anything reference
/// this" is still the union of what this reader sees and what the domain's own
/// text says.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reference_dropped_only_in_a_draft_still_never_orphans_a_shared_file() {
    let (_tmp, engine) = review_fixture().await;
    let alice = account("alice");

    // An overlay file of her own, so the listing this rule reads is genuinely
    // her dimension's rather than the base one.
    engine
        .attachment_write_as(
            "team",
            "assets/mine.png",
            b"PNG bytes of mine".to_vec(),
            &alice,
        )
        .await
        .unwrap();

    engine
        .edit_engram_as(
            &EditParams {
                identifier: "deck".to_string(),
                domain: "team".to_string(),
                operation: "find_replace".to_string(),
                find_text: Some("![Deck](assets/deck.png)".to_string()),
                content: Some("the deck is elsewhere now".to_string()),
                key: None,
                value: None,
                expected_replacements: None,
                section: None,
                include_subsections: false,
                expected_checksum: None,
                ack_scope: None,
                share_link: None,
                model: None,
            },
            None,
            &alice,
        )
        .await
        .unwrap();

    assert_eq!(
        orphans_for(&engine, &alice).await,
        vec![
            "assets/mine.png".to_string(),
            "assets/stray.png".to_string()
        ],
        "the deck is still referenced by the text the team has, so dropping the \
         reference in a draft never orphans it: {:?}",
        orphans_for(&engine, &alice).await
    );
}

/// **A file somebody has drafted answers their own reference to it.**
///
/// The sweep reads every other input in the caller's own dimension; its
/// attachment set has to be read there too, or review mode's whole reason for
/// existing - work on something privately before the team sees it - hands the
/// author a finding about a file they are looking at. The stranger's sweep is
/// the other half of the sentence: the team really does not have this file, so
/// the team's own reference to it really is dangling, and nothing about alice's
/// overlay may quiet that.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_draft_only_attachment_answers_its_authors_reference_in_the_sweep() {
    let (tmp, engine) = review_fixture().await;
    let dir = tmp.path().join("team");
    let alice = account("alice");
    let bob = account("bob");

    // A reviewed engram pointing at a file nobody has uploaded: dangling for
    // everybody, which is what the team's own sweep should go on saying.
    reviewed_file(
        &engine,
        &dir,
        "shot.md",
        &reviewed(
            "Shot",
            "shot",
            "The dashboard as it looked:\n\n![Shot](assets/shot.png)",
        ),
    )
    .await;

    let missing = vec!["assets/shot.png".to_string()];
    assert_eq!(dangling_attachments_for(&engine, &alice).await, missing);
    assert_eq!(dangling_attachments_for(&engine, &bob).await, missing);

    // Alice drafts the file. It is hers alone until the change is shared.
    let written = engine
        .attachment_write_as(
            "team",
            "assets/shot.png",
            b"PNG bytes of shot".to_vec(),
            &alice,
        )
        .await
        .unwrap();
    assert!(written.draft, "the upload landed as alice's draft");

    assert!(
        dangling_attachments_for(&engine, &alice).await.is_empty(),
        "the reference she can follow is not a dangling one for her"
    );
    assert_eq!(
        dangling_attachments_for(&engine, &bob).await,
        missing,
        "and the team's reference goes on dangling, because the team has no such file"
    );
}
