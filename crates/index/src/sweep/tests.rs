//! Rule-by-rule tests for the consolidation sweep, plus one named test for
//! every false-positive guard the design commits to.

use super::*;
use crate::store::{EdgeKind, EngramId, GraphEdge, GraphNode, RETIRED_STATUSES};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const DOMAIN: &str = "engineering";

fn day(s: &str) -> NaiveDate {
    s.parse().expect("fixture dates are valid ISO dates")
}

fn today() -> NaiveDate {
    day("2026-08-02")
}

/// A body with `lines` non-blank content lines, short enough that the
/// near-duplicate clusterer never looks at it.
fn short_body(lines: usize) -> String {
    (1..=lines)
        .map(|i| format!("Body line {i}."))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A body long enough to clear the near-duplicate floor, spread over enough
/// lines that the stub rule stays quiet.
fn long_body(subject: &str) -> String {
    format!(
        "The {subject} job runs on every push to the main branch.\n\
         It builds the whole workspace then runs the full test suite before\n\
         it uploads any artifact at all to the release bucket.\n\
         A failure anywhere in that chain stops the release outright and\n\
         pages whoever happens to be on call at the time."
    )
}

/// A stable engram recorded a month ago with three body lines and nothing else
/// set, so no rule fires on it by default.
fn fact(id: i64, permalink: &str) -> EngramFacts {
    let mut f = EngramFacts::new(EngramId(id), DOMAIN, permalink);
    f.title = permalink.replace('-', " ");
    f.body = short_body(3);
    f.recorded_at = Some(day("2026-07-01"));
    f
}

fn node_of(f: &EngramFacts) -> GraphNode {
    GraphNode {
        id: f.id,
        domain: f.domain.clone(),
        permalink: f.permalink.clone(),
        title: f.title.clone(),
        engram_type: f.engram_type.clone(),
        salience: f.salience,
        status: f.status.clone(),
    }
}

fn rel(from: i64, to: i64, rel_type: &str) -> GraphEdge {
    GraphEdge {
        from: EngramId(from),
        to: EngramId(to),
        rel_type: rel_type.to_string(),
        kind: EdgeKind::Relation,
    }
}

fn wikilink(from: i64, to: i64) -> GraphEdge {
    GraphEdge {
        from: EngramId(from),
        to: EngramId(to),
        rel_type: "links_to".to_string(),
        kind: EdgeKind::Link,
    }
}

/// An attachment the domain holds, modified on `modified` at nine in the
/// morning UTC. The sha256 is a repeated pattern so its first eight characters
/// are recognizable in evidence.
fn attachment(path: &str, modified: &str) -> AttachmentRow {
    AttachmentRow {
        path: path.to_string(),
        sha256: "ab".repeat(32),
        mime: "image/png".to_string(),
        size: 2048,
        modified: format!("{modified}T09:12:00+00:00"),
    }
}

fn tag(name: &str, engrams: i64) -> TagCount {
    TagCount {
        name: name.to_string(),
        engrams,
        observations: 0,
    }
}

/// A tag carried on observations as well as on frontmatter.
fn tag_used(name: &str, engrams: i64, observations: i64) -> TagCount {
    TagCount {
        name: name.to_string(),
        engrams,
        observations,
    }
}

/// A sweep input over `facts`, with one graph node per fact and the sweep's own
/// domain registered.
fn input(facts: Vec<EngramFacts>) -> SweepInput {
    let mut input = SweepInput::new(DOMAIN, today());
    input.graph.nodes = facts.iter().map(node_of).collect();
    input.engrams = facts;
    input.known_domains = vec![DOMAIN.to_string()];
    input
}

/// Every rule that fired, in queue order.
fn fired(report: &SweepReport) -> Vec<&str> {
    report.findings.iter().map(|f| f.rule).collect()
}

/// Every rule that fired against one permalink, in queue order.
fn fired_on<'a>(report: &'a SweepReport, permalink: &str) -> Vec<&'a str> {
    report
        .findings
        .iter()
        .filter(|f| f.permalink == permalink)
        .map(|f| f.rule)
        .collect()
}

fn only(report: &SweepReport, rule: &str) -> Finding {
    let matches: Vec<&Finding> = report.findings.iter().filter(|f| f.rule == rule).collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one {rule}; the queue held {:?}",
        fired(report)
    );
    matches[0].clone()
}

// ---------------------------------------------------------------------------
// V0xx - temporal and lifecycle
// ---------------------------------------------------------------------------

#[test]
fn v005_flags_a_replacement_whose_retirement_was_never_finished() {
    let old = fact(1, "old-runbook");
    let new = fact(2, "fresh-guide");
    let mut sweep = input(vec![old, new]);
    sweep.graph.edges = vec![rel(2, 1, "supersedes")];

    let report = detect(&sweep);
    let finding = only(&report, "V005");
    assert_eq!(finding.permalink, "old-runbook");
    assert_eq!(finding.class, Class::Mechanical);
    assert_eq!(finding.family, Family::Temporal);
    assert_eq!(finding.priority, 90);
    assert!(finding.evidence.contains("engineering/fresh-guide"));
    assert_eq!(finding.fix, "set_frontmatter status=superseded");
}

#[test]
fn v005_is_quiet_once_the_target_is_retired() {
    let mut old = fact(1, "old-runbook");
    old.status = "superseded".to_string();
    let mut sweep = input(vec![old, fact(2, "fresh-guide")]);
    sweep.graph.edges = vec![rel(2, 1, "supersedes"), rel(1, 2, "superseded_by")];

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V005"), "{:?}", fired(&report));
}

#[test]
fn v001_flags_an_expired_window_on_a_current_engram() {
    let mut expired = fact(1, "quarter-plan");
    expired.valid_to = Some(day("2026-06-30"));
    let report = detect(&input(vec![expired]));

    let finding = only(&report, "V001");
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.priority, 85);
    assert!(finding.finding.contains("2026-06-30"));
    assert!(finding.evidence.contains("today=2026-08-02"));
}

#[test]
fn v001_ignores_a_window_that_has_not_closed_yet() {
    let mut open = fact(1, "quarter-plan");
    open.valid_from = Some(day("2026-01-01"));
    open.valid_to = Some(day("2026-12-31"));
    let report = detect(&input(vec![open]));
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn absent_validity_window_fires_nothing() {
    // Absence is the contract: no valid_from means always valid and no valid_to
    // means valid forever, so neither absence is ever a finding.
    let mut open = fact(1, "evergreen-note");
    open.valid_from = None;
    open.valid_to = None;
    let report = detect(&input(vec![open]));
    assert!(
        fired(&report).is_empty(),
        "absent temporal bounds must stay silent: {:?}",
        fired(&report)
    );
}

#[test]
fn v001_is_suppressed_by_an_inbound_supersedes() {
    // V005 owns an engram whose replacement already landed, so one engram never
    // draws two findings for one underlying fact.
    let mut old = fact(1, "old-runbook");
    old.valid_to = Some(day("2026-06-30"));
    let mut sweep = input(vec![old, fact(2, "fresh-guide")]);
    sweep.graph.edges = vec![rel(2, 1, "supersedes")];

    let report = detect(&sweep);
    assert_eq!(fired_on(&report, "old-runbook"), vec!["V005"]);
}

#[test]
fn v002_flags_elapsed_staleness_with_no_verification_since() {
    let mut stale = fact(1, "tls-settings");
    stale.stale_on = Some(day("2026-05-01"));
    let report = detect(&input(vec![stale]));

    let finding = only(&report, "V002");
    assert_eq!(finding.priority, 70);
    assert!(finding.evidence.contains("never verified"));
}

#[test]
fn v002_counts_a_verification_older_than_the_staleness_date() {
    let mut stale = fact(1, "tls-settings");
    stale.stale_on = Some(day("2026-05-01"));
    stale.verified_on = Some(day("2026-04-01"));
    let report = detect(&input(vec![stale]));
    assert!(only(&report, "V002").evidence.contains("2026-04-01"));
}

#[test]
fn v002_is_quiet_when_verification_followed_the_staleness_date() {
    let mut stale = fact(1, "tls-settings");
    stale.stale_on = Some(day("2026-05-01"));
    stale.verified_on = Some(day("2026-05-02"));
    let report = detect(&input(vec![stale]));
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn v003_flags_old_knowledge_with_no_verification_and_no_bound() {
    let mut old = fact(1, "shipping-rules");
    old.recorded_at = Some(day("2025-01-05"));
    let report = detect(&input(vec![old]));

    let finding = only(&report, "V003");
    assert_eq!(finding.priority, 25);
    assert!(finding.evidence.contains("recorded_at=2025-01-05"));
}

#[test]
fn v003_is_quiet_once_a_staleness_bound_or_a_verification_exists() {
    let mut bounded = fact(1, "shipping-rules");
    bounded.recorded_at = Some(day("2025-01-05"));
    bounded.stale_on = Some(day("2027-01-01"));

    let mut verified = fact(2, "packing-rules");
    verified.recorded_at = Some(day("2025-01-05"));
    verified.verified_on = Some(day("2026-06-01"));

    let report = detect(&input(vec![bounded, verified]));
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn v003_is_capped_at_the_ten_oldest_and_reports_it() {
    let names = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliett", "kilo", "lima",
    ];
    let facts: Vec<EngramFacts> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut f = fact(i as i64 + 1, name);
            // One day older per position, so the cut is unambiguous.
            f.recorded_at = Some(day("2025-01-01") - chrono::Duration::days(i as i64));
            f
        })
        .collect();

    let report = detect(&input(facts));
    let capped: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V003")
        .collect();
    assert_eq!(capped.len(), V003_CAP);
    assert_eq!(
        report.truncations,
        vec!["V003 capped at the 10 oldest of 12".to_string()]
    );
    let reported: Vec<&str> = capped.iter().map(|f| f.permalink.as_str()).collect();
    assert!(
        reported.contains(&"lima"),
        "the oldest must be in: {reported:?}"
    );
    assert!(
        !reported.contains(&"alpha"),
        "the two newest fall outside the cap: {reported:?}"
    );
}

#[test]
fn v004_distinguishes_a_missing_relation_from_an_unresolved_one() {
    let mut missing = fact(1, "old-runbook");
    missing.status = "superseded".to_string();
    let mut dangling = fact(2, "stale-checklist");
    dangling.status = "superseded".to_string();

    let mut sweep = input(vec![missing, dangling]);
    sweep.unresolved = vec![UnresolvedRef {
        from: EngramId(2),
        rel_type: "superseded_by".to_string(),
        kind: EdgeKind::Relation,
        target_domain: None,
        target: "Newer Checklist".to_string(),
        line: Some(7),
    }];

    let report = detect(&sweep);
    let findings: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V004")
        .collect();
    assert_eq!(findings.len(), 2, "{:?}", fired(&report));

    let by_permalink = |p: &str| {
        *findings
            .iter()
            .find(|f| f.permalink == p)
            .expect("both engrams draw a V004")
    };
    assert!(
        by_permalink("old-runbook")
            .evidence
            .contains("no superseded_by relation")
    );
    let unresolved = by_permalink("stale-checklist");
    assert!(unresolved.evidence.contains("does not resolve"));
    assert_eq!(unresolved.line, Some(7));
    assert_eq!(unresolved.priority, 65);
}

#[test]
fn v004_is_quiet_once_the_successor_resolves() {
    let mut old = fact(1, "old-runbook");
    old.status = "superseded".to_string();
    let mut sweep = input(vec![old, fact(2, "fresh-guide")]);
    sweep.graph.edges = vec![rel(1, 2, "superseded_by")];

    let report = detect(&sweep);
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn retired_engrams_only_draw_v004() {
    // Retirement is terminal. A retired engram is never flagged for being old,
    // unverified, unlinked, stale or out of its validity window.
    for status in RETIRED_STATUSES {
        let mut retired = fact(1, "old-runbook");
        retired.status = status.to_string();
        retired.recorded_at = Some(day("2023-01-01"));
        retired.valid_to = Some(day("2024-01-01"));
        retired.stale_on = Some(day("2024-06-01"));
        retired.body = short_body(1);
        retired.tokens = 99_999;

        let report = detect(&input(vec![retired]));
        let expected: Vec<&str> = if status == "superseded" {
            vec!["V004"]
        } else {
            vec![]
        };
        assert_eq!(fired(&report), expected, "status {status}");
    }
}

#[test]
fn speculative_statuses_are_exempt_from_v001_v002_and_v003() {
    for status in SPECULATIVE_STATUSES {
        let mut speculative = fact(1, "half-baked-idea");
        speculative.status = status.to_string();
        speculative.recorded_at = Some(day("2024-01-01"));
        speculative.valid_to = Some(day("2025-01-01"));
        speculative.stale_on = Some(day("2025-06-01"));

        let report = detect(&input(vec![speculative]));
        assert!(
            fired(&report).is_empty(),
            "status {status} drew {:?}",
            fired(&report)
        );
    }
}

#[test]
fn v006_fires_on_an_unreviewed_human_capture() {
    let mut captured = fact(1, "incident-decision");
    captured.generated_by = Some("human:jordi".to_string());

    let report = detect(&input(vec![captured]));
    let finding = only(&report, "V006");
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.family, Family::Temporal);
    assert_eq!(finding.priority, 58, "base 50 plus the human boost of 8");
    assert!(finding.evidence.contains("generated.by human:jordi"));
    assert!(finding.evidence.contains("recorded 2026-07-01"));
    assert_eq!(
        fired(&report),
        vec!["V006"],
        "nothing else speaks about a fresh human capture"
    );

    // The actor prefix is read case-insensitively, so an actor a person typed
    // by hand counts the same as one Crystalline wrote.
    let mut shouted = fact(1, "incident-decision");
    shouted.generated_by = Some("Human:Jordi".to_string());
    let report = detect(&input(vec![shouted]));
    assert_eq!(only(&report, "V006").priority, 58);
}

#[test]
fn v006_stays_quiet_when_any_condition_is_unmet() {
    let human = || {
        let mut f = fact(1, "incident-decision");
        f.generated_by = Some("human:jordi".to_string());
        f
    };

    let mut agent = human();
    agent.generated_by = Some("claude-code/2.1".to_string());
    let mut anonymous = human();
    anonymous.generated_by = None;
    let mut reviewed = human();
    reviewed.verified_on = Some(day("2026-07-20"));
    let mut written_today = human();
    written_today.recorded_at = Some(today());
    // The sixth byte of this actor sits inside a three-byte character, which a
    // byte slice would panic on. It simply does not match instead.
    let mut split_character = human();
    split_character.generated_by = Some("huma\u{65e5}n:jordi".to_string());
    assert!(
        !split_character
            .generated_by
            .as_deref()
            .unwrap()
            .is_char_boundary(6)
    );

    for (label, quiet) in [
        ("an agent wrote it", agent),
        ("nothing records who wrote it", anonymous),
        ("somebody already verified it", reviewed),
        ("it is still being written today", written_today),
        (
            "the actor splits a character at the prefix",
            split_character,
        ),
    ] {
        let report = detect(&input(vec![quiet]));
        assert!(
            !fired(&report).contains(&"V006"),
            "{label} drew {:?}",
            fired(&report)
        );
    }
}

#[test]
fn v006_ignores_retired_and_speculative_statuses() {
    for status in RETIRED_STATUSES.iter().chain(SPECULATIVE_STATUSES.iter()) {
        let mut captured = fact(1, "incident-decision");
        captured.status = status.to_string();
        captured.generated_by = Some("human:jordi".to_string());

        let report = detect(&input(vec![captured]));
        assert!(
            !fired(&report).contains(&"V006"),
            "status {status} drew {:?}",
            fired(&report)
        );
    }
}

// ---------------------------------------------------------------------------
// V1xx - structural integrity
// ---------------------------------------------------------------------------

#[test]
fn v101_flags_a_live_reference_to_retired_knowledge() {
    let live = fact(1, "onboarding-guide");
    let mut retired = fact(2, "old-runbook");
    retired.status = "deprecated".to_string();
    let successor = fact(3, "fresh-guide");

    let mut sweep = input(vec![live, retired, successor]);
    sweep.graph.edges = vec![wikilink(1, 2), rel(3, 2, "supersedes")];

    let report = detect(&sweep);
    let finding = only(&report, "V101");
    assert_eq!(finding.permalink, "onboarding-guide");
    assert_eq!(finding.priority, 55);
    assert!(
        finding
            .evidence
            .contains("engineering/old-runbook is deprecated")
    );
    assert!(
        finding
            .evidence
            .contains("replaced by engineering/fresh-guide")
    );
    assert_eq!(finding.fix, "repoint at [[fresh guide]]");
}

#[test]
fn v101_never_flags_the_supersedes_edge_itself() {
    let fresh = fact(1, "fresh-guide");
    let mut retired = fact(2, "old-runbook");
    retired.status = "superseded".to_string();
    let mut sweep = input(vec![fresh, retired]);
    sweep.graph.edges = vec![rel(1, 2, "supersedes"), rel(2, 1, "superseded_by")];

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V101"), "{:?}", fired(&report));
}

#[test]
fn v102_is_mechanical_only_with_a_near_exact_candidate() {
    let mut writer = fact(1, "onboarding-guide");
    writer.title = "Onboarding Guide".to_string();
    let mut target = fact(2, "deployment-pipeline-runbook");
    target.title = "Deployment Pipeline Runbook".to_string();

    let mut sweep = input(vec![writer, target]);
    sweep.unresolved = vec![
        UnresolvedRef {
            from: EngramId(1),
            rel_type: "links_to".to_string(),
            kind: EdgeKind::Link,
            target_domain: None,
            target: "Deployment Pipline Runbook".to_string(),
            line: Some(12),
        },
        UnresolvedRef {
            from: EngramId(1),
            rel_type: "links_to".to_string(),
            kind: EdgeKind::Link,
            target_domain: None,
            target: "Nothing Like That At All".to_string(),
            line: Some(14),
        },
    ];

    let report = detect(&sweep);
    let findings: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V102")
        .collect();
    assert_eq!(findings.len(), 2, "{:?}", fired(&report));

    let typo = findings
        .iter()
        .find(|f| f.line == Some(12))
        .expect("the typo draws a finding");
    assert_eq!(typo.class, Class::Mechanical);
    assert_eq!(
        typo.fix,
        "[[Deployment Pipline Runbook]] -> [[Deployment Pipeline Runbook]]"
    );

    let unknown = findings
        .iter()
        .find(|f| f.line == Some(14))
        .expect("the unknown target draws a finding");
    assert_eq!(unknown.class, Class::Judgment);
    assert!(unknown.evidence.contains("no near match"));
    assert_eq!(unknown.fix, "[[Nothing Like That At All]]");
}

#[test]
fn v102_names_an_unregistered_target_domain() {
    let mut sweep = input(vec![fact(1, "onboarding-guide")]);
    sweep.unresolved = vec![UnresolvedRef {
        from: EngramId(1),
        rel_type: "links_to".to_string(),
        kind: EdgeKind::Link,
        target_domain: Some("archive".to_string()),
        target: "Old Notes".to_string(),
        line: None,
    }];

    let report = detect(&sweep);
    let finding = only(&report, "V102");
    assert_eq!(finding.class, Class::Judgment);
    assert!(
        finding
            .evidence
            .contains("target domain `archive` is not a registered domain")
    );
}

#[test]
fn v103_flags_a_one_sided_reciprocal() {
    let mut sweep = input(vec![fact(1, "release-summary"), fact(2, "raw-transcript")]);
    sweep.graph.edges = vec![rel(1, 2, "summarizes")];

    let report = detect(&sweep);
    let finding = only(&report, "V103");
    assert_eq!(finding.permalink, "raw-transcript");
    assert_eq!(finding.class, Class::Mechanical);
    assert_eq!(finding.priority, 35);
    assert_eq!(finding.fix, "append `- summarized_by [[release summary]]`");
}

#[test]
fn v103_is_quiet_once_the_converse_exists() {
    let mut sweep = input(vec![fact(1, "release-summary"), fact(2, "raw-transcript")]);
    sweep.graph.edges = vec![rel(1, 2, "summarizes"), rel(2, 1, "summarized_by")];
    let report = detect(&sweep);
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn v103_leaves_the_supersedes_half_to_v005() {
    // A current target belongs to V005, whose prescribed fix already appends
    // the back-link. Only a target V005 cannot speak about draws V103.
    let mut speculative = fact(1, "draft-runbook");
    speculative.status = "draft".to_string();
    let mut sweep = input(vec![speculative, fact(2, "fresh-guide")]);
    sweep.graph.edges = vec![rel(2, 1, "supersedes")];
    assert_eq!(fired_on(&detect(&sweep), "draft-runbook"), vec!["V103"]);

    let mut current = sweep.clone();
    current.engrams[0].status = "stable".to_string();
    assert_eq!(fired_on(&detect(&current), "draft-runbook"), vec!["V005"]);
}

#[test]
fn v104_flags_an_aged_orphan() {
    let mut facts = Vec::new();
    for (i, name) in ["alpha", "bravo", "charlie"].iter().enumerate() {
        let mut f = fact(i as i64 + 1, name);
        f.inbound = 1;
        f.outbound = 1;
        facts.push(f);
    }
    let mut orphan = fact(4, "lonely-note");
    orphan.tags = vec!["deploy".to_string()];
    facts.push(orphan);

    let report = detect(&input(facts));
    let finding = only(&report, "V104");
    assert_eq!(finding.permalink, "lonely-note");
    assert_eq!(finding.priority, 30);
    assert_eq!(finding.fix, "link it to a neighbour tagged #deploy");
}

#[test]
fn v104_is_skipped_below_the_density_gate() {
    // A domain nobody links inside is a style, not a defect, so the rule is
    // skipped whole rather than firing on every engram in it.
    let facts: Vec<EngramFacts> = ["alpha", "bravo", "charlie", "delta"]
        .iter()
        .enumerate()
        .map(|(i, name)| fact(i as i64 + 1, name))
        .collect();
    let report = detect(&input(facts));
    assert!(
        !fired(&report).contains(&"V104"),
        "{:?} at zero link density",
        fired(&report)
    );
}

#[test]
fn v104_skips_a_young_orphan_and_a_structural_file() {
    let mut linked = fact(1, "alpha");
    linked.inbound = 2;
    let mut young = fact(2, "just-captured");
    young.recorded_at = Some(day("2026-07-30"));
    let mut manifest = fact(3, "MANIFEST");
    manifest.engram_type = "manifest".to_string();

    let report = detect(&input(vec![linked, young, manifest]));
    assert!(!fired(&report).contains(&"V104"), "{:?}", fired(&report));
}

#[test]
fn v105_flags_an_oversized_body() {
    let mut big = fact(1, "everything-guide");
    big.tokens = 3200;
    let report = detect(&input(vec![big]));

    let finding = only(&report, "V105");
    assert_eq!(finding.priority, 60);
    assert!(finding.evidence.contains("budget=2500"));

    let mut unbounded = fact(1, "everything-guide");
    unbounded.tokens = 3200;
    unbounded.token_budget = 0;
    assert!(
        !fired(&detect(&input(vec![unbounded]))).contains(&"V105"),
        "a zero budget disables the rule"
    );
}

#[test]
fn v106_flags_a_stub_and_ignores_fenced_code() {
    let mut stub = fact(1, "thin-note");
    stub.body = "Only one real line.\n\n```\ncode\nmore code\nstill code\n```\n".to_string();
    let report = detect(&input(vec![stub]));

    let finding = only(&report, "V106");
    assert_eq!(finding.priority, 45);
    assert!(finding.finding.contains("1 non-blank body line"));

    let mut enough = fact(1, "thin-note");
    enough.body = "One.\nTwo.\nThree.\n".to_string();
    assert!(!fired(&detect(&input(vec![enough]))).contains(&"V106"));
}

// ---------------------------------------------------------------------------
// V2xx - redundancy and drift
// ---------------------------------------------------------------------------

#[test]
fn v201_attaches_one_finding_to_the_highest_salience_member() {
    let mut rich = fact(1, "release-process");
    rich.title = "Release process".to_string();
    rich.body = long_body("release");
    rich.salience = Some(8.0);

    let mut copy = fact(2, "shipping-checklist");
    copy.title = "Shipping checklist".to_string();
    copy.body = long_body("release").replace("pages whoever", "wakes whoever");
    copy.salience = Some(2.0);

    let report = detect(&input(vec![rich, copy]));
    let finding = only(&report, "V201");
    assert_eq!(finding.permalink, "release-process");
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.family, Family::Redundancy);
    assert_eq!(finding.priority, 88, "base 80 plus a salience boost of 8");
    assert!(finding.evidence.contains("engineering/shipping-checklist"));
}

#[test]
fn v201_leaves_unrelated_bodies_alone() {
    let mut one = fact(1, "release-process");
    one.title = "Release process".to_string();
    one.body = long_body("release");
    let mut two = fact(2, "cache-strategy");
    two.title = "Cache strategy".to_string();
    two.body = "Cache entries expire after an hour unless a write touches\n\
                the key first. The eviction pass runs every ten minutes and\n\
                logs whatever it removed to the audit stream for later.\n\
                Nothing about it is shared with the release tooling at all."
        .to_string();

    let report = detect(&input(vec![one, two]));
    assert!(!fired(&report).contains(&"V201"), "{:?}", fired(&report));
}

#[test]
fn v202_flags_a_title_collision() {
    let mut a = fact(1, "deploy-guide");
    a.title = "Deploy guide".to_string();
    let mut b = fact(2, "deploy-guide-second-take");
    b.title = "Deploy guides".to_string();

    let report = detect(&input(vec![a, b]));
    let finding = only(&report, "V202");
    assert_eq!(finding.priority, 55);
    assert!(finding.evidence.contains("engineering/deploy-guide"));
    assert!(
        finding
            .evidence
            .contains("engineering/deploy-guide-second-take")
    );
}

#[test]
fn v202_is_suppressed_inside_a_v201_cluster() {
    // The duplicate finding already prescribes a merge, which settles the
    // titles too, so reporting both would be two findings for one fact.
    let mut a = fact(1, "release-process");
    a.title = "Release process".to_string();
    a.body = long_body("release");
    let mut b = fact(2, "release-process-copy");
    b.title = "Release processes".to_string();
    b.body = long_body("release").replace("pages whoever", "wakes whoever");

    let report = detect(&input(vec![a, b]));
    assert!(fired(&report).contains(&"V201"), "{:?}", fired(&report));
    assert!(
        !fired(&report).contains(&"V202"),
        "the title collision sits inside the duplicate cluster: {:?}",
        fired(&report)
    );
}

#[test]
fn v203_hands_over_the_exact_merge_command() {
    let mut sweep = input(vec![fact(1, "alpha")]);
    sweep.tags = vec![tag("deploy", 9), tag("deploys", 2)];

    let report = detect(&sweep);
    let finding = only(&report, "V203");
    assert_eq!(finding.permalink, "", "V203 is about the vocabulary");
    assert_eq!(finding.title, "");
    assert_eq!(finding.domain, DOMAIN);
    assert_eq!(finding.priority, 30);
    assert_eq!(finding.fix, "crystalline tags merge deploys deploy");
    assert!(finding.evidence.contains("#deploy used 9 time(s)"));
}

#[test]
fn v203_counts_observation_tags_too() {
    // `guardrail` is carried only by observations, `guardrails` only by one
    // engram's frontmatter. Counting frontmatter alone would report the more
    // used spelling as `on 0 engram(s)` and merge the wrong way round.
    let mut sweep = input(vec![fact(1, "alpha")]);
    sweep.tags = vec![tag_used("guardrail", 0, 7), tag_used("guardrails", 1, 0)];

    let report = detect(&sweep);
    let finding = only(&report, "V203");
    assert_eq!(finding.fix, "crystalline tags merge guardrails guardrail");
    assert_eq!(
        finding.evidence,
        "#guardrail used 7 time(s); #guardrails used 1 time(s)"
    );
}

#[test]
fn v203_respects_declared_tag_aliases() {
    let mut sweep = input(vec![fact(1, "alpha")]);
    sweep.tags = vec![tag("deploy", 9), tag("deploys", 2)];
    sweep.tag_aliases = vec![TagAlias {
        alias: "deploys".to_string(),
        canonical: "deploy".to_string(),
    }];

    let report = detect(&sweep);
    assert!(
        !fired(&report).contains(&"V203"),
        "a declared alias already explains the pair: {:?}",
        fired(&report)
    );
}

// ---------------------------------------------------------------------------
// Ranking, catalog and plumbing
// ---------------------------------------------------------------------------

#[test]
fn ranking_is_deterministic_and_clamped() {
    // The clamp holds at both ends: base plus every boost overshoots 100 and a
    // negative or non-finite salience never subtracts.
    assert_eq!(priority(90, Some(20.0), 10, false), MAX_PRIORITY);
    assert_eq!(
        priority(90, Some(10.0), HUB_INBOUND_DEGREE, false),
        MAX_PRIORITY
    );
    assert_eq!(priority(25, Some(-4.0), 0, false), 25);
    assert_eq!(priority(25, None, HUB_INBOUND_DEGREE, false), 30);
    assert_eq!(priority(25, None, HUB_INBOUND_DEGREE - 1, false), 25);
    assert_eq!(priority(25, Some(f64::NAN), 0, false), 25);
    assert_eq!(priority(25, Some(f64::INFINITY), 0, false), 25);
    assert_eq!(priority(0, None, 0, false), 0);
    assert_eq!(priority(25, None, 0, true), 33);
    assert_eq!(
        priority(95, Some(10.0), HUB_INBOUND_DEGREE, true),
        MAX_PRIORITY
    );

    let mut findings = vec![
        Finding::about_domain("V203", "zulu"),
        Finding::about_domain("V001", "alpha"),
        Finding::about_domain("V001", "bravo"),
        Finding::about_domain("V005", "alpha"),
    ];
    rank(&mut findings);
    let order: Vec<(&str, &str)> = findings
        .iter()
        .map(|f| (f.rule, f.domain.as_str()))
        .collect();
    assert_eq!(
        order,
        vec![
            ("V005", "alpha"),
            ("V001", "alpha"),
            ("V001", "bravo"),
            ("V203", "zulu"),
        ]
    );
}

// ---------------------------------------------------------------------------
// Attachments - V007, V008, V107, V108
// ---------------------------------------------------------------------------

/// Yesterday, so the grace period on a fresh upload has passed.
const YESTERDAY: &str = "2026-08-01";

#[test]
fn v007_flags_a_referenced_attachment_nobody_analyzed() {
    let mut shown = fact(1, "deck-notes");
    shown.asset_refs = vec!["assets/deck.png".to_string()];
    let mut later = fact(2, "second-mention");
    later.asset_refs = vec!["assets/deck.png".to_string()];

    let mut sweep = input(vec![shown, later]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let finding = only(&detect(&sweep), "V007");
    assert_eq!(finding.family, Family::Temporal);
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.priority, 50);
    assert_eq!(
        finding.permalink, "deck-notes",
        "the first engram that shows the file anchors the work"
    );
    assert_eq!(
        finding.evidence,
        "assets/deck.png; image/png, 2048 bytes; modified 2026-08-01; no engram claims it via analyzes"
    );
    assert!(finding.fix.starts_with("analyzes: assets/deck.png"));
}

#[test]
fn v007_is_quiet_once_an_engram_claims_the_attachment() {
    let mut shown = fact(1, "deck-notes");
    shown.asset_refs = vec!["assets/deck.png".to_string()];
    let mut reader = fact(2, "what-the-deck-says");
    reader.analyzes = Some("assets/deck.png".to_string());
    reader.analyzed_hash = Some("ab".repeat(32));

    let mut sweep = input(vec![shown, reader]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let report = detect(&sweep);
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn a_claimed_but_unembedded_attachment_is_a_kept_source_not_an_orphan() {
    let mut reader = fact(1, "what-the-deck-says");
    reader.analyzes = Some("assets/deck.png".to_string());

    let mut sweep = input(vec![reader]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let report = detect(&sweep);
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn v108_flags_an_orphan_with_the_path_as_its_subject() {
    let mut sweep = input(vec![fact(1, "unrelated-note")]);
    sweep.attachments = vec![attachment("assets/stray.png", YESTERDAY)];

    let report = detect(&sweep);
    let finding = only(&report, "V108");
    assert_eq!(finding.family, Family::Structure);
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.priority, 55);
    assert_eq!(
        finding.permalink, "",
        "an orphan has no engram to hang a link on"
    );
    assert_eq!(finding.title, "assets/stray.png");
    assert_eq!(finding.domain, DOMAIN);
    assert_eq!(
        finding.evidence,
        "assets/stray.png; image/png, 2048 bytes; modified 2026-08-01; no engram references or claims it"
    );
    assert!(!fired(&report).contains(&"V007"));
}

#[test]
fn a_retired_engram_still_counts_as_a_referent() {
    let mut retired = fact(1, "old-deck-notes");
    retired.status = "deprecated".to_string();
    retired.asset_refs = vec!["assets/deck.png".to_string()];

    let mut sweep = input(vec![retired]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let report = detect(&sweep);
    assert!(
        !fired(&report).contains(&"V108"),
        "retired knowledge is still knowledge"
    );
    assert!(
        !fired(&report).contains(&"V007"),
        "nothing live to hang the analysis on"
    );
}

#[test]
fn a_fresh_upload_is_quiet_until_the_day_turns() {
    let mut shown = fact(1, "deck-notes");
    shown.asset_refs = vec!["assets/deck.png".to_string()];

    let mut sweep = input(vec![shown]);
    sweep.attachments = vec![
        attachment("assets/deck.png", "2026-08-02"),
        attachment("assets/stray.png", "2026-08-02"),
    ];

    let report = detect(&sweep);
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn v007_and_v108_are_disjoint_over_one_domain() {
    let mut shown = fact(1, "deck-notes");
    shown.asset_refs = vec!["assets/deck.png".to_string()];

    let mut sweep = input(vec![shown]);
    sweep.attachments = vec![
        attachment("assets/deck.png", YESTERDAY),
        attachment("assets/stray.png", YESTERDAY),
    ];

    let report = detect(&sweep);
    assert_eq!(fired(&report), vec!["V108", "V007"], "one of each, ranked");
    assert_eq!(only(&report, "V007").permalink, "deck-notes");
    assert_eq!(only(&report, "V108").title, "assets/stray.png");
}

#[test]
fn v008_names_both_hash_prefixes() {
    let mut reader = fact(1, "what-the-deck-says");
    reader.analyzes = Some("assets/deck.png".to_string());
    reader.analyzed_hash = Some("0123456789abcdef".repeat(4));

    let mut sweep = input(vec![reader]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let finding = only(&detect(&sweep), "V008");
    assert_eq!(finding.family, Family::Temporal);
    assert_eq!(finding.priority, 60);
    assert_eq!(finding.permalink, "what-the-deck-says");
    assert_eq!(
        finding.evidence,
        "analyzes assets/deck.png; analyzed_hash 01234567.. but the attachment is now abababab.."
    );
    assert_eq!(finding.fix, format!("analyzed_hash: {}", "ab".repeat(32)));
}

#[test]
fn v008_stays_quiet_without_a_recorded_hash_or_with_a_matching_one() {
    let mut no_hash = fact(1, "no-hash");
    no_hash.analyzes = Some("assets/deck.png".to_string());
    let mut matching = fact(2, "matching");
    matching.analyzes = Some("assets/deck.png".to_string());
    matching.analyzed_hash = Some("AB".repeat(32));

    let mut sweep = input(vec![no_hash, matching]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let report = detect(&sweep);
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn v107_names_the_missing_path_and_how_it_is_referenced() {
    let mut body_only = fact(1, "shows-a-ghost");
    body_only.asset_refs = vec!["assets/gone.png".to_string()];
    let mut claim_only = fact(2, "claims-a-ghost");
    claim_only.analyzes = Some("assets/also-gone.pdf".to_string());
    let mut both = fact(3, "shows-and-claims");
    both.asset_refs = vec!["assets/missing.png".to_string()];
    both.analyzes = Some("assets/missing.png".to_string());

    let report = detect(&input(vec![body_only, claim_only, both]));
    let of = |permalink: &str| -> Finding {
        report
            .findings
            .iter()
            .find(|f| f.rule == "V107" && f.permalink == permalink)
            .cloned()
            .unwrap_or_else(|| panic!("no V107 on {permalink}: {:?}", fired(&report)))
    };
    assert_eq!(of("shows-a-ghost").priority, 45);
    assert_eq!(of("shows-a-ghost").family, Family::Structure);
    assert_eq!(
        of("shows-a-ghost").finding,
        "points at an attachment that is not there"
    );
    assert_eq!(
        of("shows-a-ghost").evidence,
        "assets/gone.png referenced in the body; nothing in engineering holds that path"
    );
    assert_eq!(of("shows-a-ghost").fix, "assets/gone.png");

    // A claim on its own is the same finding, and the evidence says the claim
    // is where the missing path was written.
    assert_eq!(
        of("claims-a-ghost").evidence,
        "assets/also-gone.pdf claimed by analyzes; nothing in engineering holds that path"
    );
    assert!(
        of("shows-and-claims")
            .evidence
            .starts_with("assets/missing.png referenced in the body and claimed by analyzes")
    );
}

#[test]
fn v107_reports_one_finding_per_engram_with_its_paths_sorted() {
    let mut fact = fact(1, "shows-two-ghosts");
    fact.asset_refs = vec![
        "assets/z-last.png".to_string(),
        "assets/a-first.png".to_string(),
    ];

    let report = detect(&input(vec![fact]));
    let finding = only(&report, "V107");
    assert_eq!(
        finding.finding,
        "points at 2 attachments that are not there"
    );
    assert!(
        finding
            .evidence
            .ends_with("nothing in engineering holds those paths"),
        "{}",
        finding.evidence
    );
    assert_eq!(finding.fix, "assets/a-first.png; assets/z-last.png");
}

#[test]
fn v107_is_quiet_once_the_attachment_is_there() {
    let mut shown = fact(1, "deck-notes");
    shown.asset_refs = vec!["assets/deck.png".to_string()];
    shown.analyzes = Some("assets/deck.png".to_string());
    shown.analyzed_hash = Some("ab".repeat(32));

    let mut sweep = input(vec![shown]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    assert!(!fired(&detect(&sweep)).contains(&"V107"));
}

#[test]
fn a_retired_engram_draws_no_attachment_work() {
    let mut retired = fact(1, "old-deck-notes");
    retired.status = "deprecated".to_string();
    retired.asset_refs = vec!["assets/gone.png".to_string()];
    retired.analyzes = Some("assets/deck.png".to_string());
    retired.analyzed_hash = Some("0123456789abcdef".repeat(4));

    let mut sweep = input(vec![retired]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let report = detect(&sweep);
    assert!(fired(&report).is_empty(), "{:?}", fired(&report));
}

#[test]
fn an_undatable_attachment_is_left_alone() {
    let mut sweep = input(vec![fact(1, "unrelated-note")]);
    let mut row = attachment("assets/stray.png", YESTERDAY);
    row.modified = "whenever".to_string();
    sweep.attachments = vec![row];

    assert!(fired(&detect(&sweep)).is_empty());
}

#[test]
fn the_human_boost_follows_the_anchor_and_an_orphan_has_none() {
    let mut shown = fact(1, "deck-notes");
    shown.asset_refs = vec!["assets/deck.png".to_string()];
    shown.generated_by = Some("human:jordi".to_string());
    shown.verified_on = Some(day("2026-07-02"));

    let mut sweep = input(vec![shown]);
    sweep.attachments = vec![
        attachment("assets/deck.png", YESTERDAY),
        attachment("assets/stray.png", YESTERDAY),
    ];

    let report = detect(&sweep);
    assert_eq!(
        only(&report, "V007").priority,
        58,
        "base 50 plus the human boost of 8"
    );
    assert_eq!(
        only(&report, "V108").priority,
        55,
        "an orphan has no engram whose provenance could lift it"
    );
}

#[test]
fn human_authored_boost_applies_to_every_rule_not_only_v006() {
    let mut big = fact(1, "everything-guide");
    big.tokens = 3200;
    big.generated_by = Some("human:jordi".to_string());

    let report = detect(&input(vec![big]));
    let finding = only(&report, "V105");
    assert_eq!(finding.priority, 68, "base 60 plus the human boost of 8");
}

#[test]
fn the_catalog_carries_nineteen_rules_and_v006_is_temporal() {
    assert_eq!(RULES.len(), 19);
    let info = rule_info("V006").expect("V006 is in the catalog");
    assert_eq!(info.family, Family::Temporal);
    assert_eq!(info.base, 50);
}

#[test]
fn a_whole_sweep_is_reproducible_and_ranked() {
    let mut old = fact(1, "old-runbook");
    old.valid_to = Some(day("2026-01-01"));
    let mut stale = fact(2, "tls-settings");
    stale.stale_on = Some(day("2026-02-01"));
    let mut fresh = fact(3, "fresh-guide");
    fresh.salience = Some(6.0);
    let mut stub = fact(4, "thin-note");
    stub.body = String::new();

    let mut sweep = input(vec![old, stale, fresh, stub]);
    sweep.graph.edges = vec![rel(3, 1, "supersedes")];
    sweep.tags = vec![tag("deploy", 4), tag("deploys", 1)];

    let first = detect(&sweep);
    assert!(first.findings.len() > 3, "{:?}", fired(&first));
    assert_eq!(first.engrams_scanned, 4);
    for _ in 0..5 {
        assert_eq!(detect(&sweep), first);
    }
    for pair in first.findings.windows(2) {
        assert!(pair[0].priority >= pair[1].priority, "{:?}", fired(&first));
    }
}

#[test]
fn the_catalog_covers_every_rule_id_exactly_once() {
    let mut ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "duplicate rule id in the catalog");
    assert_eq!(
        ids,
        vec![
            "V001", "V002", "V003", "V004", "V005", "V006", "V007", "V008", "V101", "V102", "V103",
            "V104", "V105", "V106", "V107", "V108", "V201", "V202", "V203",
        ]
    );
    for rule in RULES {
        assert!(rule_info(rule.id).is_some());
        assert!(!rule.summary.is_empty());
        assert!(!rule.instruction.is_empty());
        let expected = match &rule.id[..2] {
            "V0" => Family::Temporal,
            "V1" => Family::Structure,
            _ => Family::Redundancy,
        };
        assert_eq!(rule.family, expected, "{}", rule.id);
    }
    assert!(rule_info("V301").is_none(), "V3xx stays reserved");
}

#[test]
fn family_and_class_round_trip_their_wire_names() {
    for family in Family::ALL {
        assert_eq!(Family::parse(family.as_str()), Some(family));
        assert_eq!(Family::parse(&family.as_str().to_uppercase()), Some(family));
        assert_eq!(family.to_string(), family.as_str());
    }
    assert_eq!(Family::parse("lifecycle"), None);
    assert_eq!(Class::Mechanical.as_str(), "mechanical");
    assert_eq!(Class::Judgment.to_string(), "judgment");
}

#[test]
fn content_line_count_matches_the_q001_predicate() {
    assert_eq!(content_line_count(""), 0);
    assert_eq!(content_line_count("\n\n  \n"), 0);
    assert_eq!(content_line_count("one\ntwo\nthree"), 3);
    assert_eq!(content_line_count("one\n```\nin fence\n```\ntwo"), 2);
    // An unclosed fence swallows the rest of the body, exactly as the parser
    // treats it.
    assert_eq!(content_line_count("one\n```\nin fence\nstill in fence"), 1);
    // A tilde fence does not close a backtick fence.
    assert_eq!(content_line_count("```\n~~~\nstill inside\n```\nafter"), 1);
}

#[test]
fn an_empty_input_produces_an_empty_report() {
    let report = detect(&SweepInput::new(DOMAIN, today()));
    assert_eq!(report, SweepReport::default());
}

#[test]
fn engram_facts_defaults_are_quiet() {
    let f = EngramFacts::new(EngramId(1), DOMAIN, "alpha");
    assert!(f.is_current());
    assert!(!f.is_retired());
    assert!(!f.is_speculative());
    assert_eq!(f.age_days(today()), None);
    assert_eq!(f.address(), "engineering/alpha");
    assert_eq!(f.token_budget, DEFAULT_TOKEN_BUDGET);
}
