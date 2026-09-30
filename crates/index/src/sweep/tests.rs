//! Rule-by-rule tests for the consolidation sweep, plus one named test for
//! every false-positive guard the design commits to.

use super::*;
use crate::nli::OrderAggregation;
use crate::nli::candidates::observation_hash;
use crate::store::{ContradictionRow, EdgeKind, EngramId, GraphEdge, GraphNode, RETIRED_STATUSES};

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
        actor: String::new(),
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
        raw: "Newer Checklist".to_string(),
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

/// A sweep input whose domain owes its team origin `unshared` substantive
/// changes, the oldest of them written `days` ago.
fn sharing(unshared: usize, days: i64) -> SweepInput {
    let mut sweep = input(vec![fact(1, "alpha")]);
    sweep.share = Some(ShareFacts {
        unshared,
        oldest_change: Some(today() - chrono::TimeDelta::days(days)),
    });
    sweep
}

#[test]
fn v009_asks_for_work_that_has_sat_unshared_for_a_week() {
    let report = detect(&sharing(3, 10));

    let finding = only(&report, "V009");
    assert_eq!(finding.family, Family::Temporal);
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.domain, DOMAIN);
    assert_eq!(
        finding.permalink, "",
        "V009 is about the domain rather than one engram"
    );
    assert!(finding.finding.contains('3'), "{}", finding.finding);
    assert!(finding.finding.contains("10 days"), "{}", finding.finding);
    assert!(
        finding.evidence.contains("oldest change 2026-07-23"),
        "{}",
        finding.evidence
    );
    assert!(
        finding.fix.contains("share_changes"),
        "the next action names the tool: {}",
        finding.fix
    );
    let info = rule_info("V009").expect("V009 is in the catalog");
    assert!(info.instruction.contains("share_changes"), "{info:?}");
    assert!(info.instruction.contains("origin share"), "{info:?}");
}

#[test]
fn v009_stays_quiet_while_the_unshared_work_is_still_fresh() {
    let report = detect(&sharing(3, SHARE_STALE_DAYS - 1));
    assert!(!fired(&report).contains(&"V009"), "{:?}", fired(&report));

    // The window is inclusive at its edge, so a week-old change does fire.
    let report = detect(&sharing(3, SHARE_STALE_DAYS));
    assert!(fired(&report).contains(&"V009"), "{:?}", fired(&report));
}

#[test]
fn v009_stays_quiet_when_only_generated_listings_changed() {
    // The assembler counts substantive changes only, so an index-only delta
    // reaches the detector as nothing unshared however old it is.
    let report = detect(&sharing(0, 90));
    assert!(!fired(&report).contains(&"V009"), "{:?}", fired(&report));
}

#[test]
fn v009_never_speaks_about_a_domain_with_no_origin() {
    let report = detect(&input(vec![fact(1, "alpha")]));
    assert!(!fired(&report).contains(&"V009"), "{:?}", fired(&report));
}

#[test]
fn v009_stays_quiet_when_no_changed_file_carries_a_date() {
    let mut sweep = input(vec![fact(1, "alpha")]);
    sweep.share = Some(ShareFacts {
        unshared: 4,
        oldest_change: None,
    });

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V009"), "{:?}", fired(&report));
}

/// A retired engram carrying `observations`, each written as the body bullet
/// it was parsed from so the body and the facts agree.
fn retired_with(id: i64, permalink: &str, observations: &[(usize, &str)]) -> EngramFacts {
    let mut f = fact(id, permalink);
    f.status = "superseded".to_string();
    f.body = observations
        .iter()
        .map(|(_, text)| format!("- [fact] {text}"))
        .collect::<Vec<_>>()
        .join("\n");
    f.observations = observations
        .iter()
        .map(|(line, text)| FactObservation {
            line: *line,
            text: (*text).to_string(),
        })
        .collect();
    f
}

#[test]
fn v010_names_observations_that_survive_in_no_live_engram() {
    let retired = retired_with(
        1,
        "mix-b-decision",
        &[
            (7, "Run the coolant loop on glycol mix B"),
            (8, "The loop needs a 40 minute purge before a mix swap"),
        ],
    );
    let mut carried = fact(2, "purge-procedure");
    carried.body =
        "## Observations\n\n- [fact] The loop needs a 40 minute purge before a mix swap\n"
            .to_string();

    let report = detect(&input(vec![retired, carried]));
    let finding = only(&report, "V010");
    assert_eq!(finding.permalink, "mix-b-decision");
    assert_eq!(finding.family, Family::Temporal);
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.line, Some(7));
    assert!(
        finding.evidence.contains("glycol mix B"),
        "the missing bullet is quoted: {}",
        finding.evidence
    );
    assert!(
        !finding.evidence.contains("40 minute purge"),
        "the carried bullet is not a finding: {}",
        finding.evidence
    );
    assert_eq!(finding.fix, "split_engram observations=7");
}

#[test]
fn v010_is_quiet_when_every_observation_was_carried_forward() {
    // Case, spacing and the tags trailing the live bullet all differ; the
    // normalized text is what has to survive, not the bytes.
    let retired = retired_with(1, "mix-b-decision", &[(7, "Run the loop on glycol mix B")]);
    let mut carried = fact(2, "mix-c-decision");
    carried.body = "- [decision]   RUN the loop   on glycol mix b #coolant #cooling\n".to_string();

    let report = detect(&input(vec![retired, carried]));
    assert!(!fired(&report).contains(&"V010"), "{:?}", fired(&report));
}

#[test]
fn v010_is_quiet_when_a_live_bullet_carries_the_same_text() {
    // The other haystack: the live engram's own observations, matched as whole
    // normalized texts rather than scanned for inside its body. This is the
    // shape `split_engram` itself writes, so it is the one that has to be free.
    let retired = retired_with(1, "mix-b-decision", &[(7, "Run the loop on glycol mix B")]);
    let mut carried = retired_with(2, "mix-c-decision", &[(9, "Run the loop on glycol MIX b")]);
    carried.status = "stable".to_string();
    // Nothing to find in the body text, so a hit can only come from the set.
    carried.body = short_body(3);

    let report = detect(&input(vec![retired, carried]));
    assert!(!fired(&report).contains(&"V010"), "{:?}", fired(&report));
}

#[test]
fn v010_never_builds_a_corpus_for_a_domain_with_nothing_retired() {
    // Behaviorally the same silence as any other quiet case; pinned separately
    // because the early return it stands on is what keeps the rule free in the
    // common domain.
    let mut live = fact(1, "mix-c-decision");
    live.observations = vec![FactObservation {
        line: 7,
        text: "Run the loop on glycol mix C".to_string(),
    }];
    let report = detect(&input(vec![live, fact(2, "purge-procedure")]));
    assert!(!fired(&report).contains(&"V010"), "{:?}", fired(&report));
}

#[test]
fn v010_is_quiet_once_the_split_landed() {
    // The split already happened, so what is left in the retired engram is
    // what expired, on purpose. The archive records that intent as the pair
    // `split_engram` writes, and the rule reads it.
    let retired = retired_with(1, "mix-b-decision", &[(7, "Run the loop on glycol mix B")]);
    let mut sweep = input(vec![retired, fact(2, "purge-procedure")]);
    sweep.graph.edges = vec![rel(1, 2, "split_into"), rel(2, 1, "derived_from")];

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V010"), "{:?}", fired(&report));
}

#[test]
fn v010_never_speaks_about_a_live_engram() {
    let mut live = retired_with(1, "mix-b-decision", &[(7, "Run the loop on glycol mix B")]);
    live.status = "stable".to_string();
    let report = detect(&input(vec![live]));
    assert!(!fired(&report).contains(&"V010"), "{:?}", fired(&report));
}

#[test]
fn v010_scopes_an_acknowledgment_to_the_missing_bullets() {
    let retired = retired_with(1, "mix-b-decision", &[(7, "Run the loop on glycol mix B")]);
    let report = detect(&input(vec![retired]));
    assert_eq!(only(&report, "V010").scope, "run the loop on glycol mix b");
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
    // By permalink: the suggestion is a link an agent will paste, and a
    // title carrying a colon would paste a link that does not resolve.
    assert_eq!(finding.fix, "repoint at [[fresh-guide]]");
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
            raw: "Deployment Pipline Runbook".to_string(),
            line: Some(12),
        },
        UnresolvedRef {
            from: EngramId(1),
            rel_type: "links_to".to_string(),
            kind: EdgeKind::Link,
            target_domain: None,
            target: "Nothing Like That At All".to_string(),
            raw: "Nothing Like That At All".to_string(),
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
        raw: "archive:Old Notes".to_string(),
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

/// A cross-domain reference whose prefix names a domain that IS registered: the
/// repair has to keep the prefix on both sides. A left side that is the bracket
/// text minus the prefix names a string that does not occur in the file, and a
/// right side without it repoints the link at the writer's own domain - the
/// same silent domain-drop the unregistered arm refuses.
#[test]
fn v102_keeps_a_registered_prefix_on_both_sides_of_the_repair() {
    let mut sweep = input(vec![fact(1, "runbook")]);
    // A second registered domain, holding the engram the typo meant.
    sweep.known_domains.push("eng".to_string());
    sweep.graph.nodes.push(GraphNode {
        id: EngramId(9),
        domain: "eng".to_string(),
        permalink: "incident-response-checklist".to_string(),
        title: "Incident Response Checklist".to_string(),
        engram_type: "engram".to_string(),
        salience: None,
        status: "stable".to_string(),
        actor: String::new(),
    });

    let mut reference = unresolved(1, "Incident Response Checklst");
    reference.target_domain = Some("eng".to_string());
    reference.raw = "eng:Incident Response Checklst".to_string();
    sweep.unresolved = vec![reference];

    let finding = only(&detect(&sweep), "V102");
    assert_eq!(
        finding.fix, "[[eng:Incident Response Checklst]] -> [[eng:Incident Response Checklist]]",
        "the left side is what the file says and the right side still points at eng"
    );
    assert_eq!(
        finding.class,
        Class::Mechanical,
        "the candidate is in the very domain the prefix names, so completing the \
         spelling changes nothing the archive claims"
    );
}

/// The same arm with no prefix at all: the bracket text and the raw are one
/// string, so the repair reads exactly as it did before.
#[test]
fn v102_repairs_a_plain_typo_without_inventing_a_prefix() {
    let mut target = fact(1, "release-checklist");
    target.title = "Release Checklist".to_string();
    let mut sweep = input(vec![target, fact(2, "runbook")]);
    sweep.unresolved = vec![unresolved(2, "Release Checklst")];

    let finding = only(&detect(&sweep), "V102");
    assert_eq!(finding.fix, "[[Release Checklst]] -> [[Release Checklist]]");
    assert_eq!(finding.class, Class::Mechanical);
}

/// A cross-domain reference into a domain the author means to register is not a
/// spelling mistake, and the fuzzy score cannot tell the two apart: a short
/// prefix costs almost nothing once the title is long, so
/// `[[ops:Incident Response Checklist]]` scores 0.93 against a local
/// `Incident Response Checklist`. The candidate is named either way, and the
/// class stays Judgment, because dropping a domain somebody named changes what
/// the archive claims and a person has to say yes to that.
#[test]
fn v102_keeps_an_unregistered_prefix_a_judgment_even_with_a_candidate() {
    let mut target = fact(1, "incident-response-checklist");
    target.title = "Incident Response Checklist".to_string();
    let writer = fact(2, "writer");

    let mut sweep = input(vec![target, writer]);
    let mut reference = unresolved(2, "Incident Response Checklist");
    reference.target_domain = Some("ops".to_string());
    reference.raw = "ops:Incident Response Checklist".to_string();
    sweep.unresolved = vec![reference];

    let report = detect(&sweep);
    let finding = only(&report, "V102");
    assert_eq!(
        finding.class,
        Class::Judgment,
        "an agent working the queue must not silently repoint a cross-domain \
         reference at a local engram: {}",
        finding.fix
    );
    assert!(
        finding.fix.contains("Incident Response Checklist"),
        "the candidate is still named, so the person deciding has it in hand: {}",
        finding.fix
    );
}

/// V102's repair reads a link the way the resolver does. An unresolved
/// `[[Murmur: the dispatch pipeline]]` is scored against the whole bracket
/// text, not against `the dispatch pipeline`: a prefix nobody registered is not
/// a prefix, so the resolver has already read the whole of it as a title at
/// home and the repair owes the same reading.
#[test]
fn v102_suggests_what_the_resolver_would_have_found() {
    let mut target = fact(1, "murmur-the-dispatch-pipeline");
    target.title = "Murmur: the dispatch pipelines".to_string();
    let writer = fact(2, "writer");

    let mut sweep = input(vec![target, writer]);
    let mut reference = unresolved(2, "the dispatch pipeline");
    reference.target_domain = Some("Murmur".to_string());
    reference.raw = "Murmur: the dispatch pipeline".to_string();
    sweep.unresolved = vec![reference];

    let report = detect(&sweep);
    let finding = only(&report, "V102");
    // Judgment, not Mechanical: an unregistered prefix is as often a domain
    // nobody connected yet as it is a title that happens to hold a colon, and
    // the score cannot tell them apart. See
    // `v102_keeps_an_unregistered_prefix_a_judgment_even_with_a_candidate`.
    assert_eq!(finding.class, Class::Judgment);
    assert!(
        finding.fix.contains("Murmur: the dispatch pipelines"),
        "the whole bracket text is what was scored: {}",
        finding.fix
    );
    assert!(
        finding
            .evidence
            .contains("target domain `Murmur` is not a registered domain"),
        "and the evidence still says why the prefix was dropped: {}",
        finding.evidence
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
    assert_eq!(finding.fix, "append `- summarized_by [[release-summary]]`");
}

#[test]
fn v103_flags_a_one_sided_split_pair() {
    // The new engram declares `derived_from`, so the source it was split out
    // of owes it a `split_into`. The finding attaches to the source, which is
    // the engram missing the line.
    let mut sweep = input(vec![fact(1, "purge-procedure"), fact(2, "mix-b-decision")]);
    sweep.graph.edges = vec![rel(1, 2, "derived_from")];

    let report = detect(&sweep);
    let finding = only(&report, "V103");
    assert_eq!(finding.permalink, "mix-b-decision");
    assert_eq!(finding.class, Class::Mechanical);
    assert_eq!(finding.fix, "append `- split_into [[purge-procedure]]`");
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

/// A `type: source` engram is held to four times the domain's budget.
/// Verbatim capture is what a source engram is FOR, so the rule that tells
/// a writer to move the full text into one must not then flag the engram it
/// just asked for. The factor is shared with verify's Q002 so the two keep
/// flagging the same size.
#[test]
fn a_source_engram_gets_four_times_the_budget() {
    let mut source = fact(1, "sources/transcript");
    source.engram_type = "source".to_string();
    source.token_budget = 2500;
    source.tokens = 9_000;
    let report = detect(&input(vec![source.clone()]));
    assert!(
        !report.findings.iter().any(|f| f.rule == "V105"),
        "9000 tokens is inside a source engram's 10000: {:?}",
        report.findings
    );

    source.tokens = 10_001;
    let report = detect(&input(vec![source]));
    let finding = report
        .findings
        .iter()
        .find(|f| f.rule == "V105")
        .expect("over even the source budget");
    assert!(
        finding.evidence.contains("10000"),
        "the evidence names the budget actually applied: {}",
        finding.evidence
    );
}

/// A budget of zero disables the rule for an engram whatever its type, so
/// the factor never turns an opt-out into a very large budget.
#[test]
fn a_zero_budget_still_disables_v105_for_a_source_engram() {
    let mut source = fact(1, "sources/transcript");
    source.engram_type = "source".to_string();
    source.token_budget = 0;
    source.tokens = 1_000_000;
    assert!(
        !detect(&input(vec![source]))
            .findings
            .iter()
            .any(|f| f.rule == "V105")
    );
}

/// The three wordings, one per case. A non-source over budget keeps the
/// split-by-granularity remedy; a source over its own larger budget cannot
/// be compressed and splits into sequential parts; an engram whose
/// observations alone blow the budget cannot be helped by either, because
/// no granularity split moves a bullet somewhere smaller.
#[test]
fn v105_prescribes_a_different_split_per_case() {
    let mut plain = fact(1, "big");
    plain.token_budget = 2500;
    plain.tokens = 4_000;
    let plain_fix = only(&detect(&input(vec![plain])), "V105").fix;
    assert!(plain_fix.contains("sources/"), "{plain_fix}");
    assert!(plain_fix.contains("distilled summary"), "{plain_fix}");

    let mut source = fact(2, "sources/transcript");
    source.engram_type = "source".to_string();
    source.token_budget = 2500;
    source.tokens = 12_000;
    let source_fix = only(&detect(&input(vec![source])), "V105").fix;
    assert!(
        source_fix.contains("Verbatim material does not compress"),
        "{source_fix}"
    );
    assert!(
        source_fix.contains("sequential part engrams"),
        "{source_fix}"
    );

    let mut bullets = fact(3, "observed");
    bullets.token_budget = 2500;
    bullets.tokens = 4_000;
    bullets.observations = vec![FactObservation {
        line: 3,
        text: "x".repeat(10_004),
    }];
    let bullets_fix = only(&detect(&input(vec![bullets])), "V105").fix;
    assert!(
        bullets_fix.contains("The observations alone exceed the budget"),
        "{bullets_fix}"
    );
    assert!(bullets_fix.contains("split_engram"), "{bullets_fix}");
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
// V3xx - meaning
// ---------------------------------------------------------------------------

/// A unit-length vector, the shape an embedding provider hands back.
fn unit(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

#[test]
fn v301_flags_a_twin_pair_on_lead_vectors_and_scopes_the_pair() {
    let mut a = fact(1, "retry-queue");
    a.lead_vector = Some(unit(&[1.0, 0.0, 0.0]));
    let mut b = fact(2, "retry-backoff");
    b.lead_vector = Some(unit(&[0.98, 0.2, 0.0]));
    let mut c = fact(3, "docking-clamps");
    c.lead_vector = Some(unit(&[0.0, 1.0, 0.0]));

    let report = detect(&input(vec![a, b, c]));
    let finding = only(&report, "V301");
    assert_eq!(finding.family, Family::Meaning);
    assert_eq!(finding.class, Class::Judgment);
    assert_eq!(finding.priority, 75);
    assert_eq!(
        finding.permalink, "retry-backoff",
        "equal salience: the smaller address leads"
    );
    assert_eq!(
        finding.scope,
        "engineering/retry-backoff, engineering/retry-queue"
    );
    assert!(finding.evidence.contains("0.98"), "{}", finding.evidence);
    assert!(finding.evidence.contains("engineering/retry-queue"));
    assert!(report.truncations.is_empty());
}

/// Two rows standing at one path are never each other's twin, whoever wrote
/// them.
///
/// A draft and the engram it is a draft of say close to the same thing by
/// construction - that is what makes it a draft of that engram rather than a
/// new one - so a twin finding on the pair would be the sweep telling an author
/// to merge their own work into the version they are rewriting. The same holds
/// for two actors' drafts of one path, which are two proposals for one engram
/// and not two engrams. `path` is the key rather than `actor`, because it is
/// the path that says the two rows are about the same engram; the actor is on
/// the facts so a reader of a finding can see whose row it fired on.
#[test]
fn twins_at_one_path_across_actors_are_skipped() {
    let mut base = fact(1, "retry-queue");
    base.lead_vector = Some(unit(&[1.0, 0.0, 0.0]));
    let mut drafted = fact(2, "retry-queue-revised");
    drafted.path = base.path.clone();
    drafted.actor = "alice".to_string();
    drafted.lead_vector = Some(unit(&[1.0, 0.0, 0.0]));
    let mut other = fact(3, "retry-queue-notes");
    other.path = base.path.clone();
    other.actor = "bob".to_string();
    other.lead_vector = Some(unit(&[1.0, 0.0, 0.0]));

    let report = detect(&input(vec![base, drafted, other]));
    assert!(
        !fired(&report).contains(&"V301"),
        "three rows at one path are one engram's rewrites, not twins: {:?}",
        fired(&report)
    );

    // And the rule still speaks about two rows that really are two engrams,
    // one of them a draft: the skip is about the path, not about drafts.
    let mut base = fact(1, "retry-queue");
    base.lead_vector = Some(unit(&[1.0, 0.0, 0.0]));
    let mut elsewhere = fact(2, "retry-backoff");
    elsewhere.actor = "alice".to_string();
    elsewhere.lead_vector = Some(unit(&[0.98, 0.2, 0.0]));
    let report = detect(&input(vec![base, elsewhere]));
    only(&report, "V301");
}

#[test]
fn v301_stays_quiet_below_the_threshold_without_vectors_and_across_widths() {
    let mut a = fact(1, "one");
    a.lead_vector = Some(unit(&[1.0, 0.0]));
    let mut b = fact(2, "two");
    b.lead_vector = Some(unit(&[0.8, 0.6]));
    let c = fact(3, "three");
    let mut d = fact(4, "four");
    d.lead_vector = Some(unit(&[1.0, 0.0, 0.0]));
    let report = detect(&input(vec![a, b, c, d]));
    assert!(!fired(&report).contains(&"V301"), "{:?}", fired(&report));
    assert!(report.truncations.is_empty(), "{:?}", report.truncations);

    // No provider installed at all: every lead vector absent. The rule is
    // silent, which means no finding AND no truncation line - a skip would
    // tell a reader something was cut when nothing was ever there.
    let quiet = detect(&input(vec![fact(5, "five"), fact(6, "six")]));
    assert!(!fired(&quiet).contains(&"V301"), "{:?}", fired(&quiet));
    assert!(quiet.truncations.is_empty(), "{:?}", quiet.truncations);
}

#[test]
fn v301_skips_retired_and_speculative_engrams() {
    let mut a = fact(1, "one");
    a.lead_vector = Some(unit(&[1.0, 0.0]));
    let mut b = fact(2, "two");
    b.lead_vector = Some(unit(&[1.0, 0.0]));
    b.status = "draft".to_string();
    let mut c = fact(3, "three");
    c.lead_vector = Some(unit(&[1.0, 0.0]));
    c.status = "superseded".to_string();
    let report = detect(&input(vec![a, b, c]));
    assert!(!fired(&report).contains(&"V301"), "{:?}", fired(&report));
}

#[test]
fn v301_counts_implemented_as_current() {
    let mut a = fact(1, "one");
    a.lead_vector = Some(unit(&[1.0, 0.0]));
    a.status = "implemented".to_string();
    let mut b = fact(2, "two");
    b.lead_vector = Some(unit(&[1.0, 0.0]));
    let report = detect(&input(vec![a, b]));
    only(&report, "V301");
}

#[test]
fn v301_is_suppressed_inside_a_v201_cluster() {
    let mut a = fact(1, "release-process");
    a.body = long_body("release");
    a.lead_vector = Some(unit(&[1.0, 0.0]));
    let mut b = fact(2, "release-copy");
    b.body = long_body("release").replace("pages whoever", "wakes whoever");
    b.lead_vector = Some(unit(&[1.0, 0.0]));
    let mut c = fact(3, "release-paraphrase");
    c.lead_vector = Some(unit(&[1.0, 0.0]));
    let report = detect(&input(vec![a, b, c]));
    assert!(fired(&report).contains(&"V201"));
    let twins: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V301")
        .collect();
    assert_eq!(
        twins.len(),
        2,
        "the paraphrase twins each cluster member; the cluster pair itself is V201's: {:?}",
        fired(&report)
    );
    assert!(
        twins
            .iter()
            .all(|f| f.scope.contains("engineering/release-paraphrase"))
    );
}

#[test]
fn v301_caps_findings_at_ten_keeping_the_highest_cosines() {
    let mut facts = Vec::new();
    for i in 0..12 {
        let mut f = fact(i as i64 + 1, &format!("twin-{i:02}"));
        f.lead_vector = Some(unit(&[1.0, 0.002 * i as f32]));
        facts.push(f);
    }
    let report = detect(&input(facts));
    assert_eq!(
        report.findings.iter().filter(|f| f.rule == "V301").count(),
        10
    );
    assert!(
        report
            .truncations
            .iter()
            .any(|t| t == "V301 findings capped at 10"),
        "{:?}",
        report.truncations
    );

    // Selection is by cosine: with a cap of one, the closest pair wins.
    let mut a = fact(1, "a");
    a.lead_vector = Some(unit(&[1.0, 0.0]));
    let mut b = fact(2, "b");
    b.lead_vector = Some(unit(&[1.0, 0.01]));
    let mut c = fact(3, "c");
    c.lead_vector = Some(unit(&[1.0, 0.3]));
    let mut input = input(vec![a, b, c]);
    input.options.max_twin_findings = 1;
    let report = detect(&input);
    let finding = only(&report, "V301");
    assert_eq!(finding.scope, "engineering/a, engineering/b");
}

#[test]
fn v301_reports_the_vector_cap_and_skips_the_rule() {
    let mut facts = Vec::new();
    for i in 0..4 {
        let mut f = fact(i as i64 + 1, &format!("twin-{i}"));
        f.lead_vector = Some(unit(&[1.0, 0.0]));
        facts.push(f);
    }
    let mut input = input(facts);
    input.options.max_twin_vectors = 3;
    let report = detect(&input);
    assert!(!fired(&report).contains(&"V301"));
    assert!(
        report
            .truncations
            .iter()
            .any(|t| t == "V301 skipped: 4 lead vectors over the 3 cap"),
        "{:?}",
        report.truncations
    );
}

#[test]
fn v301_acknowledgment_is_scoped_to_the_pair() {
    let mut hub = fact(1, "hub");
    hub.lead_vector = Some(unit(&[1.0, 0.0]));
    hub.salience = Some(9.0);
    hub.acks.push(AckEntry {
        rule: "V301".to_string(),
        scope: Some("engineering/hub, engineering/twin-one".to_string()),
        note: Some("distinct, linked".to_string()),
    });
    let mut one = fact(2, "twin-one");
    one.lead_vector = Some(unit(&[1.0, 0.0]));
    let mut two = fact(3, "twin-two");
    two.lead_vector = Some(unit(&[1.0, 0.0]));

    let report = detect(&input(vec![hub, one, two]));
    assert_eq!(report.acknowledged.meaning, 1);
    let twins: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V301")
        .collect();
    assert_eq!(twins.len(), 2, "{:?}", fired(&report));
    let on_hub = twins
        .iter()
        .find(|f| f.permalink == "hub")
        .expect("hub still leads its other pair");
    assert_eq!(on_hub.scope, "engineering/hub, engineering/twin-two");
    assert!(!on_hub.acknowledged);
    // And it is a plain finding, not a stale one: the entry on the hub was
    // given for the other pair, so its note says nothing about this one.
    assert!(!on_hub.ack_stale);
    assert_eq!(on_hub.ack_note, None);
    assert_eq!(on_hub.ack_scope, None);

    // A second acknowledgment, for this pair, stands beside the first and
    // silences it alone.
    let mut hub = fact(1, "hub");
    hub.lead_vector = Some(unit(&[1.0, 0.0]));
    hub.salience = Some(9.0);
    hub.acks.push(AckEntry {
        rule: "V301".to_string(),
        scope: Some("engineering/hub, engineering/twin-one".to_string()),
        note: Some("distinct, linked".to_string()),
    });
    hub.acks.push(AckEntry {
        rule: "V301".to_string(),
        scope: Some("engineering/hub, engineering/twin-two".to_string()),
        note: Some("also distinct".to_string()),
    });
    let mut one = fact(2, "twin-one");
    one.lead_vector = Some(unit(&[1.0, 0.0]));
    let mut two = fact(3, "twin-two");
    two.lead_vector = Some(unit(&[1.0, 0.0]));

    let report = detect(&input(vec![hub, one, two]));
    assert_eq!(report.acknowledged.meaning, 2);
    let twins: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V301")
        .collect();
    assert_eq!(twins.len(), 1, "{:?}", fired(&report));
    assert_eq!(twins[0].permalink, "twin-one");
    // The two acknowledged pairs are quiet and the third is a plain finding:
    // it hangs on an engram with no entry of its own, and two entries on the
    // hub lend nothing to anybody.
    assert!(!twins[0].ack_stale);
    assert_eq!(twins[0].ack_note, None);
    assert_eq!(twins[0].ack_scope, None);
}

#[test]
fn twins_cosine_is_symmetric_and_refuses_mismatched_widths() {
    use crate::sweep::twins::cosine;
    assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-9);
    assert!((cosine(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-9);
    assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0);
    assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    // A NaN component poisons the quotient, and NaN fails every threshold
    // comparison, so the pair is dropped rather than ranked.
    assert!(cosine(&[f32::NAN, 0.0], &[1.0, 0.0]).is_nan());
}

#[test]
fn twins_never_retain_more_pairs_than_the_pair_cap() {
    use crate::sweep::twins::find_twins;

    // A dense scope: twelve vectors fanned across a narrow arc, so all 66
    // pairs clear the threshold and the retention guard has real work to do.
    let vectors: Vec<Vec<f32>> = (0..12).map(|i| unit(&[1.0, 0.01 * i as f32])).collect();
    let refs: Vec<Option<&[f32]>> = vectors.iter().map(|v| Some(v.as_slice())).collect();

    let unbounded = SweepOptions {
        max_twin_pairs: 1000,
        ..SweepOptions::default()
    };
    let all = find_twins(&refs, &unbounded);
    assert_eq!(
        all.pairs.len(),
        66,
        "the fixture has to be dense to test this"
    );
    assert_eq!(all.compared, 12);

    // The guard discards from the bottom, so a bounded run is the prefix of
    // the unbounded one: the same findings in the same order, never a
    // different set.
    // Zero included: the guard must not panic on an empty heap's peek.
    for cap in [0usize, 1, 5, 12, 65] {
        let options = SweepOptions {
            max_twin_pairs: cap,
            ..SweepOptions::default()
        };
        let bounded = find_twins(&refs, &options);
        assert_eq!(bounded.pairs.len(), cap, "cap {cap}");
        assert_eq!(bounded.pairs, all.pairs[..cap], "cap {cap}");
        assert_eq!(bounded.compared, 12, "cap {cap}");
        assert!(
            !bounded.capped,
            "cap {cap}: the pair guard is not the vector cap"
        );
    }
}

#[test]
fn v301_keeps_its_findings_when_the_pair_guard_bites() {
    // The default guard leaves room far above the finding cap, so a domain
    // dense enough to trip it still fills the queue.
    let mut facts = Vec::new();
    for i in 0..12 {
        let mut f = fact(i as i64 + 1, &format!("twin-{i:02}"));
        f.lead_vector = Some(unit(&[1.0, 0.002 * i as f32]));
        facts.push(f);
    }
    // The closest two of the twelve are word-for-word near-duplicates as well.
    // Closest, so their pair is the first the guard retains and the first the
    // finding loop would reach - which is what makes this the combination
    // worth pinning: the retained set holds a pair `V201` already owns, and
    // the suppression drops it from inside the emitted window rather than the
    // cap quietly doing the work.
    facts[10].body = long_body("release");
    facts[11].body = long_body("release").replace("pages whoever", "wakes whoever");
    let mut sweep = input(facts);
    sweep.options.max_twin_pairs = 12;
    let report = detect(&sweep);
    assert!(fired(&report).contains(&"V201"), "{:?}", fired(&report));
    let twins: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V301")
        .collect();
    assert_eq!(twins.len(), 10);
    assert!(
        !twins.iter().any(|f| {
            f.scope.contains("engineering/twin-10") && f.scope.contains("engineering/twin-11")
        }),
        "the pair the cluster already prescribes a merge for is not doubled as a twin: {:?}",
        twins.iter().map(|f| f.scope.as_str()).collect::<Vec<_>>()
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

/// A reference the domain still holds at a path one reader's view replaced
/// keeps the file referenced. The facts are that reader's - their draft says
/// nothing about the deck - and the domain's own rows are what
/// `shadowed_asset_refs` carries.
#[test]
fn a_reference_the_domain_still_holds_keeps_an_attachment_referenced() {
    let mut sweep = input(vec![fact(1, "the-deck-rewritten")]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];

    let report = detect(&sweep);
    assert_eq!(
        fired(&report),
        vec!["V108"],
        "with nothing referencing it the file is an orphan"
    );

    sweep.shadowed_asset_refs = vec!["assets/deck.png".to_string()];
    let report = detect(&sweep);
    assert!(
        !fired(&report).contains(&"V108"),
        "deleting it is a shared act, so the shared reference answers for it: {:?}",
        fired(&report)
    );
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

// ---------------------------------------------------------------------------
// V111
// ---------------------------------------------------------------------------

/// An ingestion-record fact linking `permalink`, naming `resource` when given.
fn ingestion_record(id: i64, permalink: &str, resource: Option<&str>) -> EngramFacts {
    let mut f = fact(id, permalink);
    f.engram_type = "ingestion".to_string();
    f.resource = resource.map(str::to_string);
    f
}

#[test]
fn v111_flags_an_engram_an_ingestion_record_links_that_names_no_resource() {
    let record = ingestion_record(
        1,
        "sources/ops-wiki",
        Some("https://wiki.example/spaces/OPS"),
    );
    let derived = fact(2, "retry-policy");
    let mut sweep = input(vec![record, derived]);
    sweep.graph.edges = vec![wikilink(1, 2)];

    let report = detect(&sweep);
    let finding = only(&report, "V111");
    assert_eq!(finding.permalink, "retry-policy");
    assert_eq!(finding.class, Class::Judgment);
    assert!(
        finding
            .evidence
            .contains("ingestion record engineering/sources/ops-wiki"),
        "{}",
        finding.evidence
    );
    assert!(
        finding
            .evidence
            .contains("resource https://wiki.example/spaces/OPS"),
        "{}",
        finding.evidence
    );
    assert!(
        finding.evidence.ends_with("no resource"),
        "{}",
        finding.evidence
    );
    for cell in [&finding.finding, &finding.evidence, &finding.fix] {
        assert!(!cell.contains(','), "TOON cells carry no commas: {cell}");
    }
    assert!(
        finding.fix.contains("set_frontmatter key resource"),
        "{}",
        finding.fix
    );
}

#[test]
fn v111_stays_quiet_when_the_derived_engram_names_a_resource() {
    let record = ingestion_record(1, "sources/ops-wiki", None);
    let mut derived = fact(2, "retry-policy");
    derived.resource = Some("https://wiki.example/pages/42".to_string());
    let mut sweep = input(vec![record, derived]);
    sweep.graph.edges = vec![wikilink(1, 2)];
    assert!(!fired(&detect(&sweep)).contains(&"V111"));
}

#[test]
fn v111_ignores_links_from_an_ordinary_engram() {
    let hub = fact(1, "overview");
    let target = fact(2, "retry-policy");
    let mut sweep = input(vec![hub, target]);
    sweep.graph.edges = vec![wikilink(1, 2)];
    assert!(!fired(&detect(&sweep)).contains(&"V111"));
}

#[test]
fn v111_flags_an_ingested_from_relation_resolved_or_not() {
    let record = ingestion_record(1, "sources/ops-wiki", None);
    let derived = fact(2, "retry-policy");
    let mut sweep = input(vec![record, derived]);
    sweep.graph.edges = vec![rel(2, 1, "ingested_from")];
    assert_eq!(fired_on(&detect(&sweep), "retry-policy"), vec!["V111"]);

    let lonely = fact(3, "orphaned-derivation");
    let mut sweep = input(vec![lonely]);
    sweep.unresolved = vec![UnresolvedRef {
        rel_type: "ingested_from".to_string(),
        kind: EdgeKind::Relation,
        ..unresolved(3, "Gone Record")
    }];
    let finding = only(&detect(&sweep), "V111");
    assert!(
        finding.evidence.contains("ingested_from [[Gone Record]]"),
        "{}",
        finding.evidence
    );
}

#[test]
fn v111_never_flags_the_record_a_retired_engram_or_a_manifest() {
    let record = ingestion_record(1, "sources/ops-wiki", None);
    let other_record = ingestion_record(2, "sources/older-pass", None);
    let mut retired = fact(3, "old-policy");
    retired.status = "superseded".to_string();
    let mut manifest = fact(4, "manifest");
    manifest.engram_type = "manifest".to_string();
    let mut sweep = input(vec![record, other_record, retired, manifest]);
    sweep.graph.edges = vec![wikilink(1, 2), wikilink(1, 3), wikilink(1, 4)];
    assert!(
        !fired(&detect(&sweep)).contains(&"V111"),
        "{:?}",
        fired(&detect(&sweep))
    );
}

#[test]
fn v111_exempt_types_are_matched_case_insensitively() {
    let record = ingestion_record(1, "sources/ops-wiki", None);
    let mut manifest = fact(2, "manifest");
    manifest.engram_type = "Manifest".to_string();
    let mut schema = fact(3, "schema-def");
    schema.engram_type = "SCHEMA".to_string();
    let mut sweep = input(vec![record, manifest, schema]);
    sweep.graph.edges = vec![wikilink(1, 2), wikilink(1, 3)];
    assert!(
        !fired(&detect(&sweep)).contains(&"V111"),
        "an exempt type spelled in another case must still be exempt: {:?}",
        fired(&detect(&sweep))
    );
}

#[test]
fn the_catalog_carries_twenty_six_rules_and_v006_is_temporal() {
    assert_eq!(RULES.len(), 26);
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
            "V001", "V002", "V003", "V004", "V005", "V006", "V007", "V008", "V009", "V010", "V101",
            "V102", "V103", "V104", "V105", "V106", "V107", "V108", "V109", "V110", "V111", "V201",
            "V202", "V203", "V301", "V302",
        ]
    );
    for rule in RULES {
        assert!(rule_info(rule.id).is_some());
        assert!(!rule.summary.is_empty());
        assert!(!rule.instruction.is_empty());
        let expected = match &rule.id[..2] {
            "V0" => Family::Temporal,
            "V1" => Family::Structure,
            "V2" => Family::Redundancy,
            _ => Family::Meaning,
        };
        assert_eq!(rule.family, expected, "{}", rule.id);
    }
    assert_eq!(
        rule_info("V301").expect("V301 is in the catalog").family,
        Family::Meaning,
        "semantic twins compare what two engrams say, which is the meaning family"
    );
    assert_eq!(
        rule_info("V302").expect("V302 is in the catalog").family,
        Family::Meaning
    );
    assert_eq!(Family::ALL.len(), 4);
    assert_eq!(Family::parse("meaning"), Some(Family::Meaning));
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

// ---------------------------------------------------------------------------
// Acknowledgments
// ---------------------------------------------------------------------------

/// An acknowledgment for `rule`, scoped or not, with a note.
fn ack(rule: &str, scope: Option<&str>, note: &str) -> AckEntry {
    AckEntry {
        rule: rule.to_string(),
        scope: scope.map(str::to_string),
        note: Some(note.to_string()),
    }
}

/// The V101 fixture the acknowledgment tests work: one live engram linking to
/// one retired one, so the scope is a single address.
fn v101_fixture() -> SweepInput {
    let live = fact(1, "onboarding-guide");
    let mut retired = fact(2, "old-runbook");
    retired.status = "deprecated".to_string();
    let mut sweep = input(vec![live, retired]);
    sweep.graph.edges = vec![wikilink(1, 2)];
    sweep
}

#[test]
fn a_scoped_ack_suppresses_its_finding_and_is_counted() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![ack(
        "V101",
        Some("engineering/old-runbook"),
        "lineage citation; keep",
    )];

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V101"), "{:?}", fired(&report));
    assert_eq!(report.acknowledged.total, 1);
    assert_eq!(report.acknowledged.structure, 1);
    assert_eq!(report.acknowledged.temporal, 0);
    assert_eq!(report.acknowledged.redundancy, 0);
    assert_eq!(report.acknowledged.meaning, 0);
}

#[test]
fn a_second_retired_link_changes_the_scope_and_the_ack_goes_stale() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![ack(
        "V101",
        Some("engineering/old-runbook"),
        "lineage citation; keep",
    )];
    let mut also_retired = fact(3, "older-runbook");
    also_retired.status = "deprecated".to_string();
    sweep.graph.nodes.push(node_of(&also_retired));
    sweep.engrams.push(also_retired);
    sweep.graph.edges.push(wikilink(1, 3));

    let report = detect(&sweep);
    let finding = only(&report, "V101");
    assert!(finding.ack_stale, "the acknowledged evidence changed");
    assert!(!finding.acknowledged);
    assert_eq!(finding.ack_note.as_deref(), Some("lineage citation; keep"));
    assert_eq!(
        finding.ack_scope.as_deref(),
        Some("engineering/old-runbook"),
        "the row says what was acknowledged, not what the rule fires on now"
    );
    assert_eq!(
        finding.scope, "engineering/old-runbook, engineering/older-runbook",
        "while the finding's own scope has moved on"
    );
    assert_eq!(report.acknowledged.total, 0);
}

/// A rule that is not pair-scoped keeps one entry however many findings it
/// raises on one engram, and this is what that costs: acknowledging the second
/// `V103` finding replaces the entry the first was given for, so that one
/// comes back marked stale wearing the note it never asked for.
///
/// Deliberate, and pinned here because two documents claim it - the
/// `merged_acks` doc and the `edit_engram` description an agent reads. Only
/// [`is_pair_scoped`] buys the other behaviour.
#[test]
fn a_rule_firing_twice_on_one_engram_shares_the_one_acknowledgment() {
    // Two one-sided reciprocal pairs pointing at the same engram: a summary
    // and a split, so the hub owes two back-links and V103 fires twice on it.
    let hub = fact(1, "mix-b-decision");
    let summary = fact(2, "release-summary");
    let split_out = fact(3, "purge-procedure");
    let mut sweep = input(vec![hub, summary, split_out]);
    sweep.graph.edges = vec![rel(2, 1, "summarizes"), rel(3, 1, "derived_from")];
    assert_eq!(
        fired_on(&detect(&sweep), "mix-b-decision"),
        vec!["V103", "V103"],
        "the fixture has to raise both halves on the one engram"
    );

    sweep.engrams[0].acks = vec![ack(
        "V103",
        Some("engineering/release-summary"),
        "the summary owns the link",
    )];

    let report = detect(&sweep);
    let finding = only(&report, "V103");
    assert_eq!(finding.scope, "engineering/purge-procedure");
    assert!(
        finding.ack_stale,
        "the pair nobody acknowledged wears the other's entry as stale"
    );
    assert_eq!(
        finding.ack_note.as_deref(),
        Some("the summary owns the link")
    );
    assert_eq!(report.acknowledged.total, 1);
}

#[test]
fn a_scopeless_ack_matches_whatever_the_evidence_becomes() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![ack("V101", None, "hand written")];
    let mut also_retired = fact(3, "older-runbook");
    also_retired.status = "deprecated".to_string();
    sweep.graph.nodes.push(node_of(&also_retired));
    sweep.engrams.push(also_retired);
    sweep.graph.edges.push(wikilink(1, 3));

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V101"), "{:?}", fired(&report));
    assert_eq!(report.acknowledged.total, 1);
}

/// A scope-less entry acknowledged nothing in particular, so an audited row
/// carries no acknowledged scope rather than borrowing the finding's own.
#[test]
fn a_scopeless_ack_leaves_the_audited_row_without_an_acknowledged_scope() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![ack("V101", None, "hand written")];
    sweep.include_acknowledged = true;

    let finding = only(&detect(&sweep), "V101");
    assert!(finding.acknowledged);
    assert_eq!(finding.ack_scope, None);
    assert_eq!(finding.scope, "engineering/old-runbook");
}

#[test]
fn a_prose_edit_that_leaves_the_links_alone_keeps_the_ack_matching() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![ack(
        "V101",
        Some("engineering/old-runbook"),
        "lineage citation; keep",
    )];
    sweep.engrams[0].body = format!("{}\nA new paragraph nobody linked from.", short_body(3));

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V101"), "{:?}", fired(&report));
    assert_eq!(report.acknowledged.total, 1);
}

#[test]
fn include_acknowledged_returns_the_suppressed_finding_marked() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![ack(
        "V101",
        Some("engineering/old-runbook"),
        "lineage citation; keep",
    )];
    sweep.include_acknowledged = true;

    let report = detect(&sweep);
    let finding = only(&report, "V101");
    assert!(finding.acknowledged);
    assert!(!finding.ack_stale);
    assert_eq!(finding.ack_note.as_deref(), Some("lineage citation; keep"));
    assert_eq!(finding.scope, "engineering/old-runbook");
    assert_eq!(
        finding.ack_scope.as_deref(),
        Some("engineering/old-runbook"),
        "the two agree while the acknowledgment still matches"
    );
    assert_eq!(
        report.acknowledged.total, 1,
        "the count is what it suppressed, whether or not the row came back"
    );
}

#[test]
fn an_ack_for_a_rule_that_never_fires_counts_nothing() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![ack("V104", None, "standalone on purpose")];

    let report = detect(&sweep);
    assert!(fired(&report).contains(&"V101"), "{:?}", fired(&report));
    assert_eq!(report.acknowledged, AckCounts::default());
    let finding = only(&report, "V101");
    assert!(!finding.ack_stale);
    assert_eq!(finding.ack_note, None);
}

#[test]
fn an_ack_on_one_engram_never_silences_another() {
    let mut sweep = v101_fixture();
    let mut second = fact(3, "second-guide");
    second.acks = vec![ack("V101", None, "not mine to give")];
    sweep.graph.nodes.push(node_of(&second));
    sweep.engrams.push(second);
    sweep.graph.edges.push(wikilink(3, 2));

    let report = detect(&sweep);
    assert_eq!(
        fired_on(&report, "onboarding-guide"),
        vec!["V101"],
        "the unacknowledged engram keeps its finding"
    );
    assert_eq!(report.acknowledged.total, 1);
}

#[test]
fn the_generous_entry_wins_over_a_stale_one() {
    let mut sweep = v101_fixture();
    sweep.engrams[0].acks = vec![
        ack("V101", Some("engineering/somewhere-else"), "outdated"),
        ack("V101", None, "hand written catch-all"),
    ];

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V101"), "{:?}", fired(&report));
    assert_eq!(report.acknowledged.total, 1);
}

#[test]
fn an_attachment_ack_is_scoped_to_its_path() {
    let mut shown = fact(1, "deck-notes");
    shown.asset_refs = vec!["assets/deck.png".to_string()];
    shown.acks = vec![ack("V007", Some("assets/deck.png"), "decorative")];
    let mut sweep = input(vec![shown]);
    sweep.attachments = vec![attachment("assets/deck.png", YESTERDAY)];
    sweep.include_acknowledged = true;

    let report = detect(&sweep);
    let finding = only(&report, "V007");
    assert!(finding.acknowledged);
    assert_eq!(finding.scope, "assets/deck.png");
    assert_eq!(report.acknowledged.total, 1);
    assert_eq!(report.acknowledged.temporal, 1);
}

#[test]
fn an_anchorless_finding_takes_no_ack() {
    let mut holder = fact(1, "unrelated-note");
    holder.acks = vec![ack("V108", None, "cannot reach it")];
    let mut sweep = input(vec![holder]);
    sweep.attachments = vec![attachment("assets/stray.png", YESTERDAY)];

    let report = detect(&sweep);
    assert!(fired(&report).contains(&"V108"), "{:?}", fired(&report));
    assert_eq!(report.acknowledged, AckCounts::default());
}

#[test]
fn scope_is_sorted_deduplicated_and_empty_where_identity_is_the_engram() {
    let unsorted = vec![
        "engineering/beta".to_string(),
        "engineering/alpha".to_string(),
        "engineering/beta".to_string(),
    ];
    for rule in ["V010", "V101", "V102", "V103", "V107", "V201", "V202"] {
        assert_eq!(
            scope_for(rule, unsorted.clone()),
            "engineering/alpha, engineering/beta",
            "{rule} scopes a set"
        );
    }
    for rule in ["V007", "V008", "V110"] {
        assert_eq!(
            scope_for(rule, vec!["assets/deck.png".to_string()]),
            "assets/deck.png",
            "{rule} scopes one item"
        );
    }
    for rule in [
        "V001", "V002", "V003", "V004", "V005", "V006", "V104", "V105", "V106", "V108", "V203",
    ] {
        assert_eq!(
            scope_for(rule, unsorted.clone()),
            "",
            "{rule}'s identity is the engram and the rule"
        );
    }
}

#[test]
fn every_rule_in_the_catalog_has_a_decided_scope() {
    // The match is exhaustive by construction: a new rule id lands in the
    // empty-scope arm, which is the safe default, and this pins that the
    // catalog and the scope function are read together.
    let scoped = [
        "V007", "V008", "V010", "V101", "V102", "V103", "V107", "V109", "V110", "V201", "V202",
        "V301", "V302",
    ];
    for info in RULES {
        let produced = scope_for(info.id, vec!["one".to_string()]);
        if scoped.contains(&info.id) {
            assert_eq!(produced, "one", "{} carries a scope", info.id);
        } else {
            assert!(produced.is_empty(), "{} carries no scope", info.id);
        }
    }
}

/// Every rule that carries a scope hands the right material to `scope_for` at
/// its own call site. `scope_for` itself is unit tested above; these pin what
/// each detector puts into it, because a wrong or unstable list there is what
/// would make an acknowledgment flap between sweeps.
#[test]
fn v102_scopes_the_sorted_set_of_unresolved_targets() {
    let mut fact = fact(1, "link-typo");
    fact.body = short_body(3);
    let mut sweep = input(vec![fact]);
    sweep.unresolved = vec![unresolved(1, "Zulu Target"), unresolved(1, "Alpha Target")];

    let report = detect(&sweep);
    let scopes: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V102")
        .map(|f| f.scope.as_str())
        .collect();
    assert_eq!(
        scopes,
        vec!["Alpha Target, Zulu Target", "Alpha Target, Zulu Target"],
        "every V102 finding on one engram shares the sorted set, so one \
         acknowledgment covers what is broken now and expires when that changes"
    );

    // A third broken link changes the set, so an acknowledgment given for the
    // old one stops matching.
    sweep.unresolved.push(unresolved(1, "Mike Target"));
    let report = detect(&sweep);
    let finding = report
        .findings
        .iter()
        .find(|f| f.rule == "V102")
        .expect("V102 still fires");
    assert_eq!(
        finding.scope, "Alpha Target, Mike Target, Zulu Target",
        "a new broken link moves the scope"
    );
}

#[test]
fn v103_scopes_the_counterpart_and_v201_the_whole_cluster() {
    // V103: the counterpart that declared the forward half.
    let target = fact(1, "summary");
    let source = fact(2, "long-form");
    let mut sweep = input(vec![target, source]);
    sweep.graph.edges = vec![rel(2, 1, "summarizes")];
    let finding = only(&detect(&sweep), "V103");
    assert_eq!(finding.scope, "engineering/long-form");

    // V201: every member of the cluster, the lead included, so the scope reads
    // the same whichever member the finding hung on.
    let mut one = fact(1, "ci-pipeline");
    one.body = long_body("release");
    let mut two = fact(2, "ci-pipeline-copy");
    two.body = long_body("release");
    let sweep = input(vec![one, two]);
    let finding = only(&detect(&sweep), "V201");
    assert_eq!(
        finding.scope,
        "engineering/ci-pipeline, engineering/ci-pipeline-copy"
    );
}

#[test]
fn v202_scopes_the_colliding_titles() {
    let mut one = fact(1, "setup-a");
    one.title = "Setup guide".to_string();
    let mut two = fact(2, "setup-b");
    two.title = "Setup guides".to_string();
    let sweep = input(vec![one, two]);
    let finding = only(&detect(&sweep), "V202");
    assert_eq!(finding.scope, "Setup guide, Setup guides");
}

#[test]
fn v107_scopes_the_sorted_missing_paths_and_v008_the_claimed_one() {
    // V107: every path the engram points at that the domain does not hold.
    let mut fact = fact(1, "ghost-ref");
    fact.asset_refs = vec![
        "assets/zulu.png".to_string(),
        "assets/alpha.png".to_string(),
    ];
    let sweep = input(vec![fact]);
    let finding = only(&detect(&sweep), "V107");
    assert_eq!(finding.scope, "assets/alpha.png, assets/zulu.png");

    // V008: the claimed path, not the hash - acknowledging "this engram is
    // allowed to lag this file" survives the file changing again.
    let mut reader = fact_with_claim(2, "what-the-shot-shows", "assets/shot.png");
    reader.analyzed_hash = Some("cd".repeat(32));
    let mut sweep = input(vec![reader]);
    sweep.attachments = vec![attachment("assets/shot.png", YESTERDAY)];
    let finding = only(&detect(&sweep), "V008");
    assert_eq!(finding.scope, "assets/shot.png");
}

/// An engram claiming `path`, with a body that references it too.
fn fact_with_claim(id: i64, permalink: &str, path: &str) -> EngramFacts {
    let mut f = fact(id, permalink);
    f.analyzes = Some(path.to_string());
    f.asset_refs = vec![path.to_string()];
    f
}

/// An unresolved prose wikilink from `from` to `target`.
fn unresolved(from: i64, target: &str) -> UnresolvedRef {
    UnresolvedRef {
        from: EngramId(from),
        rel_type: "links_to".to_string(),
        kind: EdgeKind::Link,
        target_domain: None,
        target: target.to_string(),
        // The bracket text and the target are the same string when no prefix
        // was written, which is what an unprefixed link actually stores.
        raw: target.to_string(),
        line: Some(3),
    }
}

// ---------------------------------------------------------------------------
// V109 - permalink off its folder
// ---------------------------------------------------------------------------

/// A fact whose file sits at `path` and answers to `permalink`.
fn filed(id: i64, permalink: &str, path: &str) -> EngramFacts {
    let mut f = fact(id, permalink);
    f.path = path.to_string();
    f
}

#[test]
fn v109_fires_on_a_permalink_whose_folder_is_not_the_files() {
    let sweep = input(vec![
        filed(1, "velog/alpha", "projects/velog/alpha.md"),
        filed(2, "overview", "projects/velog/overview.md"),
    ]);
    let report = detect(&sweep);
    assert_eq!(fired_on(&report, "velog/alpha"), vec!["V109"]);
    assert_eq!(fired_on(&report, "overview"), vec!["V109"]);

    let drifted = report
        .findings
        .iter()
        .find(|f| f.permalink == "velog/alpha")
        .unwrap();
    assert_eq!(drifted.class, Class::Judgment);
    assert_eq!(drifted.family, Family::Structure);
    assert_eq!(
        drifted.finding,
        "permalink sits in velog/ while the file sits in projects/velog/"
    );
    assert_eq!(
        drifted.evidence,
        "permalink=velog/alpha; path=projects/velog/alpha.md"
    );
    assert!(
        drifted.fix.contains("destination projects/velog/alpha.md")
            && drifted.fix.contains("permalink \"path\"")
            && drifted.fix.contains("projects/velog/alpha"),
        "{}",
        drifted.fix
    );
    assert_eq!(
        drifted.scope, "velog/alpha, projects/velog/alpha.md",
        "the permalink and the path, in that order"
    );
    let flat = report
        .findings
        .iter()
        .find(|f| f.permalink == "overview")
        .unwrap();
    assert_eq!(
        flat.finding,
        "permalink sits in the domain root while the file sits in projects/velog/"
    );
}

#[test]
fn v109_stays_quiet_on_a_matching_folder_a_root_file_and_a_reserved_one() {
    let sweep = input(vec![
        // In step, including a different spelling of the same folder and a
        // last segment that differs from the file name.
        filed(1, "projects/velog/alpha", "projects/velog/alpha.md"),
        filed(2, "projects/velog/renamed", "Projects/Velog/Beta Notes.md"),
        // A root file answers to whatever it likes.
        filed(3, "handbook/gamma", "gamma.md"),
        // The generated listing and the log are structure.
        filed(4, "somewhere/else", "projects/index.md"),
        filed(5, "somewhere/log", "projects/log.md"),
    ]);
    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V109"), "{:?}", fired(&report));
}

#[test]
fn a_v109_ack_holds_until_the_permalink_or_the_path_changes() {
    let acked = |permalink: &str, path: &str| {
        let mut f = filed(1, permalink, path);
        f.acks = vec![ack(
            "V109",
            Some("handbook/alpha, projects/alpha.md"),
            "the handbook address is deliberate",
        )];
        detect(&input(vec![f]))
    };

    let silent = acked("handbook/alpha", "projects/alpha.md");
    assert!(!fired(&silent).contains(&"V109"), "{:?}", fired(&silent));
    assert_eq!(silent.acknowledged.structure, 1);

    for (permalink, path) in [
        ("handbook/alpha-notes", "projects/alpha.md"),
        ("handbook/alpha", "archive/alpha.md"),
    ] {
        let report = acked(permalink, path);
        let finding = only(&report, "V109");
        assert!(finding.ack_stale, "{permalink} at {path}");
        assert!(!finding.acknowledged);
    }
}

// ---------------------------------------------------------------------------
// V110 - a link spelled with a name only this machine uses
// ---------------------------------------------------------------------------

/// A reference from `from` whose written prefix is `spelling`, naming
/// `target` after the colon.
fn spelled(from: i64, spelling: &str, target: &str) -> SpelledRef {
    SpelledRef {
        from: EngramId(from),
        line: 7,
        spelling: spelling.to_string(),
        raw: format!("{spelling}:{target}"),
    }
}

#[test]
fn v110_fires_when_respell_names_a_spelled_references_prefix() {
    let mut sweep = input(vec![fact(1, "link-holder")]);
    sweep.respell = vec![("eng-knowledge".to_string(), "eng".to_string())];
    sweep.spelled_refs = vec![spelled(1, "eng-knowledge", "a")];

    let finding = only(&detect(&sweep), "V110");
    assert_eq!(finding.family, Family::Structure);
    assert_eq!(finding.class, Class::Mechanical);
    assert_eq!(finding.permalink, "link-holder");
    assert_eq!(finding.line, Some(7));
    assert_eq!(
        finding.evidence,
        "`[[eng-knowledge:a]]` names domain `eng-knowledge`, a name only this machine uses; \
         the domain's name is `eng`"
    );
    assert_eq!(
        finding.fix,
        "edit_engram with operation find_replace, find_text \"[[eng-knowledge:a]]\" and content \
         \"[[eng:a]]\"",
        "the fix names the real edit_engram interface (operation, find_text, content)"
    );
    assert_eq!(
        finding.scope, "eng-knowledge",
        "scoped by the spelling alone, like V007 and V008"
    );
}

/// Two different local-only spellings on one engram draw two findings, and
/// an acknowledgment given for one spelling reads as stale on the other
/// rather than silently covering it: `V110`'s scope is the spelling alone,
/// not a set of every spelling the engram carries.
#[test]
fn v110_scopes_by_the_spelling_so_two_local_only_spellings_are_acknowledged_separately() {
    let mut f = fact(1, "link-holder");
    f.acks = vec![ack(
        "V110",
        Some("eng-knowledge"),
        "the old spelling is fine to keep for now",
    )];
    let mut sweep = input(vec![f]);
    sweep.respell = vec![
        ("eng-knowledge".to_string(), "eng".to_string()),
        ("old-alias".to_string(), "eng".to_string()),
    ];
    sweep.spelled_refs = vec![
        spelled(1, "eng-knowledge", "a"),
        spelled(1, "old-alias", "b"),
    ];

    let report = detect(&sweep);
    let findings: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V110")
        .collect();
    assert_eq!(
        findings.len(),
        1,
        "the eng-knowledge finding is suppressed by its own ack; old-alias is not: {:?}",
        fired(&report)
    );
    let remaining = findings[0];
    assert_eq!(remaining.scope, "old-alias");
    assert!(
        remaining.ack_stale,
        "an ack scoped to a different spelling reads as stale rather than silently matching"
    );
    assert_eq!(
        report.acknowledged.structure, 1,
        "exactly the eng-knowledge finding was counted as suppressed"
    );
}

/// An empty `respell` - nothing the name table would ever rewrite - draws no
/// `V110` finding, whatever `spelled_refs` carries: the visibility rule
/// (hidden domains, and any other reason a spelling was left out of
/// `respell`) is enforced entirely by what the engine puts into `respell`
/// before the sweep runs.
#[test]
fn v110_stays_quiet_when_respell_is_empty() {
    let mut sweep = input(vec![fact(1, "link-holder")]);
    sweep.spelled_refs = vec![spelled(1, "eng-knowledge", "a")];

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V110"), "{:?}", fired(&report));
}

/// A `spelled_refs` entry whose prefix `respell` does not name is left alone
/// too: `detect_local_spellings` matches on `respell`, never on the mere
/// presence of a spelled reference, which is what keeps a hidden domain's
/// spelling silent even if a row for it somehow reached this input.
#[test]
fn v110_ignores_a_spelled_ref_whose_prefix_respell_does_not_name() {
    let mut sweep = input(vec![fact(1, "link-holder")]);
    sweep.respell = vec![("eng-knowledge".to_string(), "eng".to_string())];
    sweep.spelled_refs = vec![spelled(1, "other-local-name", "a")];

    let report = detect(&sweep);
    assert!(!fired(&report).contains(&"V110"), "{:?}", fired(&report));
}

// ---------------------------------------------------------------------------
// V302 - possible contradiction, from stored rows
// ---------------------------------------------------------------------------

fn observed(id: i64, permalink: &str, lines: &[(usize, &str)]) -> EngramFacts {
    let mut f = fact(id, permalink);
    f.observations = lines
        .iter()
        .map(|(line, text)| FactObservation {
            line: *line,
            text: text.to_string(),
        })
        .collect();
    f
}

#[allow(clippy::too_many_arguments)]
fn stored(
    a: i64,
    b: i64,
    la: usize,
    ta: &str,
    lb: usize,
    tb: &str,
    ab: f32,
    ba: f32,
) -> ContradictionRow {
    ContradictionRow {
        a: EngramId(a),
        b: EngramId(b),
        line_a: la,
        line_b: lb,
        hash_a: observation_hash(ta),
        hash_b: observation_hash(tb),
        score_ab: ab,
        score_ba: ba,
        period: false,
    }
}

/// The finding line is pinned here rather than read from the model table, so
/// the measurement moving a model's threshold never moves these tests.
fn meaning_input(facts: Vec<EngramFacts>, rows: Vec<ContradictionRow>) -> SweepInput {
    let mut sweep = input(facts);
    sweep.contradictions = rows;
    sweep.contradiction_model = "nli-x".to_string();
    sweep.options.contradiction_threshold = 0.85;
    sweep
}

#[test]
fn v302_quotes_both_lines_scopes_the_pair_of_lines_and_attaches_where_v301_would() {
    let a = observed(1, "node-version", &[(5, "The build uses Node 18")]);
    let b = observed(2, "ci-runtime", &[(7, "The build uses Node 20")]);
    let rows = vec![stored(
        1,
        2,
        5,
        "The build uses Node 18",
        7,
        "The build uses Node 20",
        0.93,
        0.89,
    )];
    let report = detect(&meaning_input(vec![a, b], rows));
    let f = only(&report, "V302");
    assert_eq!(f.family, Family::Meaning);
    assert_eq!(f.class, Class::Judgment);
    assert_eq!(f.priority, 85);
    assert_eq!(
        f.permalink, "ci-runtime",
        "equal salience: the smaller address leads, as for V301"
    );
    assert_eq!(f.line, Some(7));
    assert_eq!(
        f.finding,
        "\"The build uses Node 20\" (ci runtime) against \"The build uses Node 18\" (node version) read as a contradiction at probability 0.91"
    );
    assert_eq!(
        f.evidence,
        "engineering/ci-runtime line 7; engineering/node-version line 5; probability 0.91; model nli-x"
    );
    assert_eq!(
        f.fix,
        "read both then supersede or close a window or acknowledge V302"
    );
    // Each hash stays beside its engram: ordered by address, never sorted.
    assert_eq!(
        f.scope,
        format!(
            "engineering/ci-runtime, engineering/node-version, {}, {}",
            observation_hash("The build uses Node 20"),
            observation_hash("The build uses Node 18")
        )
    );
    let c = f
        .counterpart
        .expect("a V302 finding names the other engram");
    assert_eq!((c.permalink.as_str(), c.line), ("node-version", 5));
    assert_eq!(c.title, "node version");
    assert!((c.probability - 0.91).abs() < 1e-6);
    assert!(report.truncations.is_empty(), "{:?}", report.truncations);
}

#[test]
fn v302_is_quiet_below_the_line_when_off_and_on_a_retired_speculative_or_disjoint_pair() {
    let a = observed(1, "one", &[(5, "x")]);
    let b = observed(2, "two", &[(5, "y")]);
    let low = detect(&meaning_input(
        vec![a.clone(), b.clone()],
        vec![stored(1, 2, 5, "x", 5, "y", 0.84, 0.84)],
    ));
    assert!(!fired(&low).contains(&"V302"));
    let nan = detect(&meaning_input(
        vec![a.clone(), b.clone()],
        vec![stored(1, 2, 5, "x", 5, "y", f32::NAN, 0.99)],
    ));
    assert!(
        !fired(&nan).contains(&"V302"),
        "a NaN never clears the line"
    );
    let off = detect(&input(vec![a.clone(), b.clone()]));
    assert!(!fired(&off).contains(&"V302"));
    assert!(off.truncations.is_empty(), "an off check is silent");
    for status in ["superseded", "draft"] {
        let mut other = b.clone();
        other.status = status.to_string();
        let r = detect(&meaning_input(
            vec![a.clone(), other],
            vec![stored(1, 2, 5, "x", 5, "y", 0.95, 0.95)],
        ));
        assert!(!fired(&r).contains(&"V302"), "{status}");
    }
    let mut closed = a.clone();
    closed.valid_to = Some(day("2026-01-01"));
    let mut later = b.clone();
    later.valid_from = Some(day("2026-02-01"));
    let w = detect(&meaning_input(
        vec![closed, later],
        vec![stored(1, 2, 5, "x", 5, "y", 0.95, 0.95)],
    ));
    assert!(
        !fired(&w).contains(&"V302"),
        "a window closed before the other opened is two periods"
    );
}

#[test]
fn v302_drops_a_row_whose_line_was_edited_and_survives_a_renumbering() {
    let a = observed(1, "one", &[(5, "The build uses Node 18")]);
    // Moved from line 7 to line 9, and a whitespace change folds away.
    let b = observed(2, "two", &[(9, "The build  uses Node 20")]);
    let row = stored(
        1,
        2,
        5,
        "The build uses Node 18",
        7,
        "The build uses Node 20",
        0.9,
        0.9,
    );
    let moved = detect(&meaning_input(vec![a.clone(), b], vec![row.clone()]));
    let f = only(&moved, "V302");
    // Equal salience: `leader` picks the lower address, engineering/one.
    assert_eq!(f.permalink, "one");
    assert_eq!(f.line, Some(5), "the leader's own line");
    let c = f.counterpart.clone().expect("the other engram");
    assert_eq!(
        (c.permalink.as_str(), c.line),
        ("two", 9),
        "the line comes from the engram, the hash from the row"
    );
    assert!(
        f.evidence.contains("engineering/two line 9"),
        "{}",
        f.evidence
    );
    let edited = observed(2, "two", &[(9, "The build uses Node 22")]);
    let gone = detect(&meaning_input(vec![a, edited], vec![row]));
    assert!(
        !fired(&gone).contains(&"V302"),
        "an edited line is a new question"
    );
}

#[test]
fn v302_attaches_to_the_more_salient_engram_whatever_the_address() {
    let a = observed(1, "alpha", &[(5, "x")]);
    let mut b = observed(2, "beta", &[(8, "y")]);
    b.salience = Some(7.0);
    let f = only(
        &detect(&meaning_input(
            vec![a, b],
            vec![stored(1, 2, 5, "x", 8, "y", 0.9, 0.9)],
        )),
        "V302",
    );
    assert_eq!((f.permalink.as_str(), f.line), ("beta", Some(8)));
    let c = f.counterpart.clone().expect("the other engram");
    assert_eq!((c.permalink.as_str(), c.line), ("alpha", 5));
    // The scope is ordered by address, not by the anchor.
    assert_eq!(
        f.scope,
        format!(
            "engineering/alpha, engineering/beta, {}, {}",
            observation_hash("x"),
            observation_hash("y")
        )
    );
}

#[test]
fn the_order_aggregation_decides_what_clears_the_line() {
    let a = observed(1, "one", &[(5, "x")]);
    let b = observed(2, "two", &[(5, "y")]);
    let rows = vec![stored(1, 2, 5, "x", 5, "y", 0.95, 0.70)];
    let mut sweep = meaning_input(vec![a, b], rows);
    sweep.options.contradiction_threshold = 0.8;
    sweep.options.order_aggregation = OrderAggregation::Mean;
    assert!(
        fired(&detect(&sweep)).contains(&"V302"),
        "mean 0.825 clears 0.8"
    );
    sweep.options.order_aggregation = OrderAggregation::Min;
    assert!(
        !fired(&detect(&sweep)).contains(&"V302"),
        "min 0.70 does not"
    );
}

#[test]
fn a_line_that_names_a_period_points_at_the_window() {
    let a = observed(1, "one", &[(5, "Since 2024 the build uses Node 20")]);
    let b = observed(2, "two", &[(5, "The build uses Node 18")]);
    let mut row = stored(
        1,
        2,
        5,
        "Since 2024 the build uses Node 20",
        5,
        "The build uses Node 18",
        0.9,
        0.9,
    );
    row.period = true;
    let f = only(&detect(&meaning_input(vec![a, b], vec![row])), "V302");
    assert_eq!(
        f.fix,
        "One of these lines names a period; if both held at different times, close the older engram's validity window. read both then supersede or close a window or acknowledge V302"
    );
}

#[test]
fn v302_caps_at_ten_highest_score_first_and_names_what_is_not_scored() {
    let mut facts = vec![observed(99, "hub", &[(1, "hub fact")])];
    let mut rows = Vec::new();
    for i in 1..=12 {
        facts.push(observed(
            i,
            &format!("e{i:02}"),
            &[(1, &format!("fact {i}"))],
        ));
        let p = 0.85 + i as f32 * 0.01;
        rows.push(stored(i, 99, 1, &format!("fact {i}"), 1, "hub fact", p, p));
    }
    let mut sweep = meaning_input(facts, rows);
    sweep.contradictions_pending = 3;
    sweep.contradiction_candidates_capped = true;
    let report = detect(&sweep);
    let v302: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V302")
        .collect();
    assert_eq!(v302.len(), 10);
    assert!(
        v302.iter().all(|f| !f.evidence.contains("probability 0.86")
            && !f.evidence.contains("probability 0.87")),
        "the two lowest were cut"
    );
    assert!(
        report
            .truncations
            .contains(&"V302 findings capped at 10".to_string()),
        "{:?}",
        report.truncations
    );
    assert!(
        report.truncations.contains(
            &"V302: 3 related pairs not scored yet (the daemon scores them after embedding)"
                .to_string()
        )
    );
    assert!(report.truncations.contains(
        &"V302: related pairs capped at 2000, the least related are never scored".to_string()
    ));
}

/// Lesson 37 and spec 5: a quiet `V302` never reads as a clean domain. An
/// engram that could take part but has no lead vector yet, and a scope over
/// the vector cap, each say so.
#[test]
fn v302_names_unembedded_engrams_and_a_scope_over_the_vector_cap() {
    let mut sweep = meaning_input(vec![observed(1, "one", &[(5, "x")])], Vec::new());
    sweep.contradiction_unembedded = 4;
    sweep.contradiction_vectors_capped = Some(5001);
    let report = detect(&sweep);
    assert!(
        report.truncations.contains(
            &"V302: 4 engrams have no embedding yet, their related pairs are not counted"
                .to_string()
        ),
        "{:?}",
        report.truncations
    );
    assert!(
        report.truncations.contains(
            &"V302 skipped: 5001 lead vectors over the 5000 cap, no related pairs are scored"
                .to_string()
        ),
        "{:?}",
        report.truncations
    );
}

/// Lesson 7: an unknown count is said out loud, and it is never a zero. An
/// off check carries the flag `false` and says nothing.
#[test]
fn an_uncounted_domain_says_so_and_an_off_check_says_nothing() {
    let mut sweep = meaning_input(vec![observed(1, "one", &[(5, "x")])], Vec::new());
    sweep.contradictions_uncounted = true;
    assert_eq!(
        detect(&sweep).truncations,
        vec!["V302: related pairs not counted yet (the daemon counts them after embedding)"]
    );
    let off = input(vec![observed(1, "one", &[(5, "x")])]);
    assert!(!off.contradictions_uncounted);
    assert!(detect(&off).truncations.is_empty());
}

#[test]
fn v302_acknowledgment_is_scoped_to_the_pair_of_lines() {
    let mut a = observed(1, "one", &[(5, "x"), (6, "p")]);
    let b = observed(2, "two", &[(5, "y"), (6, "q")]);
    let first = detect(&meaning_input(
        vec![a.clone(), b.clone()],
        vec![stored(1, 2, 5, "x", 5, "y", 0.9, 0.9)],
    ));
    let f = only(&first, "V302");
    assert_eq!(
        f.permalink, "one",
        "the smaller address anchors the pair, so the entry goes there"
    );
    a.acks = vec![AckEntry {
        rule: "V302".into(),
        scope: Some(f.scope),
        note: Some("different builds".into()),
    }];
    let rows = vec![
        stored(1, 2, 5, "x", 5, "y", 0.9, 0.9),
        stored(1, 2, 6, "p", 6, "q", 0.9, 0.9),
    ];
    let report = detect(&meaning_input(vec![a, b], rows));
    let left: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.rule == "V302")
        .collect();
    assert_eq!(
        left.len(),
        1,
        "a new disagreement between the same two engrams still surfaces"
    );
    assert_eq!(left[0].line, Some(6));
    assert!(
        !left[0].ack_stale,
        "a pair-scoped rule is never stale, only unanswered"
    );
    assert_eq!(report.acknowledged.meaning, 1);
    assert!(is_pair_scoped("V302"));
}

/// Review mode: a draft is a row of its own and never scored, so a stored
/// row never lands on the draft that shadows its base engram.
#[test]
fn a_draft_is_never_read_as_the_engram_its_row_names() {
    let a = observed(1, "one", &[(5, "x")]);
    let mut b = observed(2, "two", &[(5, "y")]);
    b.actor = "alice".to_string();
    let r = detect(&meaning_input(
        vec![a, b],
        vec![stored(1, 2, 5, "x", 5, "y", 0.95, 0.95)],
    ));
    assert!(!fired(&r).contains(&"V302"));
}

// ---------------------------------------------------------------------------
// The pair caps count only what nobody acknowledged
// ---------------------------------------------------------------------------

/// Acknowledge every `rule` finding `report` shows, on its anchor in `facts`,
/// for the scope it fired on.
fn acknowledge_shown(facts: &mut [EngramFacts], report: &SweepReport, rule: &str) -> usize {
    let mut n = 0;
    for f in report.findings.iter().filter(|f| f.rule == rule) {
        let anchor = facts
            .iter_mut()
            .find(|e| e.permalink == f.permalink)
            .expect("the anchor is in scope");
        anchor.acks.push(AckEntry {
            rule: rule.to_string(),
            scope: Some(f.scope.clone()),
            note: Some("read".to_string()),
        });
        n += 1;
    }
    n
}

/// Eleven twin pairs, the ten closest acknowledged: the eleventh surfaces, the
/// ten are counted, and nothing reads as capped. Capping before the
/// acknowledgments were known would hide it for ever.
#[test]
fn v301_an_acknowledged_pair_takes_no_slot_so_the_eleventh_surfaces() {
    let dims = 11;
    let mut facts = Vec::new();
    for k in 0..11usize {
        let mut base = vec![0.0f32; dims];
        base[k] = 1.0;
        let mut near = base.clone();
        near[(k + 1) % dims] = 0.01 * (k as f32 + 1.0);
        let mut a = fact(2 * k as i64 + 1, &format!("p{k:02}-a"));
        a.lead_vector = Some(unit(&base));
        let mut b = fact(2 * k as i64 + 2, &format!("p{k:02}-b"));
        b.lead_vector = Some(unit(&near));
        facts.push(a);
        facts.push(b);
    }
    let first = detect(&input(facts.clone()));
    assert!(
        first
            .truncations
            .contains(&"V301 findings capped at 10".to_string())
    );
    assert_eq!(acknowledge_shown(&mut facts, &first, "V301"), 10);
    assert!(
        !first.findings.iter().any(|f| f.permalink == "p10-a"),
        "the loosest pair was the one cut"
    );

    let next = detect(&input(facts));
    let left: Vec<&Finding> = next.findings.iter().filter(|f| f.rule == "V301").collect();
    assert_eq!(left.len(), 1, "{:?}", fired(&next));
    assert_eq!(left[0].permalink, "p10-a");
    assert_eq!(next.acknowledged.meaning, 10);
    assert!(
        !next.truncations.iter().any(|t| t.starts_with("V301")),
        "{:?}",
        next.truncations
    );
}

/// The same for `V302`: eleven stored rows, the ten highest acknowledged, and
/// the eleventh surfaces - "a new disagreement still surfaces" holds past the
/// cap.
#[test]
fn v302_an_acknowledged_pair_takes_no_slot_so_the_eleventh_surfaces() {
    let mut facts = Vec::new();
    let mut rows = Vec::new();
    for k in 0..11i64 {
        facts.push(observed(2 * k + 1, &format!("p{k:02}-a"), &[(3, "x")]));
        facts.push(observed(2 * k + 2, &format!("p{k:02}-b"), &[(3, "y")]));
        let p = 0.99 - 0.01 * k as f32;
        rows.push(stored(2 * k + 1, 2 * k + 2, 3, "x", 3, "y", p, p));
    }
    let first = detect(&meaning_input(facts.clone(), rows.clone()));
    assert!(
        first
            .truncations
            .contains(&"V302 findings capped at 10".to_string())
    );
    assert_eq!(acknowledge_shown(&mut facts, &first, "V302"), 10);

    let next = detect(&meaning_input(facts, rows));
    let left: Vec<&Finding> = next.findings.iter().filter(|f| f.rule == "V302").collect();
    assert_eq!(left.len(), 1, "{:?}", fired(&next));
    assert_eq!(left[0].permalink, "p10-a");
    assert_eq!(next.acknowledged.meaning, 10);
    assert!(
        !next.truncations.iter().any(|t| t.starts_with("V302")),
        "{:?}",
        next.truncations
    );
}

/// The text and the probability column come from one rounded value.
#[test]
fn v302_rounds_the_probability_once() {
    let a = observed(1, "one", &[(5, "x")]);
    let b = observed(2, "two", &[(5, "y")]);
    let f = only(
        &detect(&meaning_input(
            vec![a, b],
            vec![stored(1, 2, 5, "x", 5, "y", 0.905, 0.905)],
        )),
        "V302",
    );
    let p = f
        .counterpart
        .as_ref()
        .expect("the other engram")
        .probability;
    assert!(
        f.finding.ends_with(&format!("probability {p:.2}")),
        "{} against {p}",
        f.finding
    );
    assert_eq!(p, (p * 100.0).round() / 100.0, "already two decimals");
}

/// With the model unavailable the daemon runs no pass, so neither the
/// pending line nor the not-counted line may promise one.
#[test]
fn a_model_that_could_not_load_points_at_status_rather_than_a_pass() {
    let mut sweep = meaning_input(vec![observed(1, "one", &[(5, "x")])], Vec::new());
    sweep.contradiction_model_unavailable = true;
    sweep.contradictions_pending = 2;
    assert_eq!(
        detect(&sweep).truncations,
        vec![
            "V302: 2 related pairs not scored: the contradiction model could not be loaded (crystalline status and crystalline doctor say why; setting evolve.contradictions again or restarting the daemon retries)"
        ]
    );
    sweep.contradictions_pending = 0;
    sweep.contradictions_uncounted = true;
    assert_eq!(
        detect(&sweep).truncations,
        vec![
            "V302: related pairs not counted: the contradiction model could not be loaded (crystalline status and crystalline doctor say why; setting evolve.contradictions again or restarting the daemon retries)"
        ]
    );
    // A read-only daemon refuses the setting, so a restart is the remedy.
    sweep.contradiction_read_only = true;
    assert_eq!(
        detect(&sweep).truncations,
        vec![
            "V302: related pairs not counted: the contradiction model could not be loaded (crystalline status and crystalline doctor say why; the daemon is read-only, so restarting it retries)"
        ]
    );
}
