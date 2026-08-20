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

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::params::EvolveParams;
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
        .evolve_detect(&EvolveParams {
            today: Some(today.to_string()),
            ..p
        })
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
    assert_eq!(v["total"], 15);
    assert_eq!(v["count"], 15);
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
            { "family": "temporal", "findings": 6 },
            { "family": "structure", "findings": 6 },
            { "family": "redundancy", "findings": 3 },
        ])
    );

    // The prose instruction rides the legend once per rule, never a row.
    let actions = v["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 15);
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
    for page in 1..=3 {
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
        assert_eq!(v["total"], 15);
        assert_eq!(v["limit"], 5);
        assert_eq!(v["page"], page);
        assert_eq!(v["count"], 5, "fifteen findings fill three whole pages");
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
    assert_eq!(past["total"], 15);
    assert_eq!(past["count"], 0);
    assert!(past["queue"].as_array().unwrap().is_empty());
}

/// The `today` override moves the temporal comparisons and nothing else, which
/// is what makes a run reproducible. Evaluated before every planted date, the
/// age and window rules go silent while the structural and redundancy rules are
/// unchanged.
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
            "V004", "V005", "V101", "V102", "V103", "V105", "V106", "V201", "V202", "V203"
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
        vec!["V005", "V001", "V002", "V004", "V006", "V003"]
    );
    assert_eq!(temporal["total"], 6);
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
/// recovers in one step. The reserved `V3xx` range is not in the catalog, so
/// asking for it errors rather than returning silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_domain_family_and_rule_error_with_the_valid_set() {
    let (_tmp, engine) = fixture().await;

    let e = engine
        .evolve_engrams(&EvolveParams {
            domains: vec!["nope".to_string()],
            ..EvolveParams::default()
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("domain 'nope' not registered"), "{e}");
    assert!(e.contains("eng"), "{e}");

    let e = engine
        .evolve_engrams(&EvolveParams {
            families: vec!["lifecycle".to_string()],
            ..EvolveParams::default()
        })
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "unknown family 'lifecycle'; valid families: temporal, structure, redundancy"
    );

    let e = engine
        .evolve_engrams(&EvolveParams {
            rules: vec!["V301".to_string()],
            ..EvolveParams::default()
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("unknown rule 'V301'; valid rules: V001, V002"),
        "{e}"
    );
    assert!(e.ends_with("V203"), "{e}");

    let e = engine
        .evolve_engrams(&EvolveParams {
            today: Some("last tuesday".to_string()),
            ..EvolveParams::default()
        })
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
    assert_eq!(v["total"], 14);
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
        .evolve_detect(&EvolveParams {
            domains: vec!["eng".to_string()],
            today: Some(TODAY.to_string()),
            ..EvolveParams::default()
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(scratch.maintenance_path()).unwrap(),
        before,
        "detection must not write the maintenance state"
    );

    let v = engine
        .evolve_engrams(&EvolveParams {
            domains: vec!["eng".to_string()],
            today: Some(TODAY.to_string()),
            ..EvolveParams::default()
        })
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
        .evolve_engrams(&EvolveParams {
            today: Some(TODAY.to_string()),
            ..EvolveParams::default()
        })
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

/// Acknowledge `rule` on `permalink` the way an agent does: `edit_engram` with
/// `set_frontmatter`, key `evolve_ack`, the rule and note as one value.
async fn acknowledge(engine: &Engine, permalink: &str, value: &str) -> Value {
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
        )
        .await
        .unwrap()
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
        .unacknowledge_finding_as("eng", "live-doc", "v101", Some("human:jordi"))
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
            .unacknowledge_finding_as("eng", "live-doc", "V101", None)
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
            .unacknowledge_finding_as("eng", "live-doc", "V101", Some("human:jordi"))
            .await
            .unwrap()
    );
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("evolve_ack"), "{on_disk}");
}
