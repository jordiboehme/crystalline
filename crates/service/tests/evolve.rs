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
