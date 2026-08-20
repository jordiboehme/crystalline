//! Surgical editor tests: section editing (including the subsection-preserved
//! regression), frontmatter field edits and the `generated` provenance touch.

mod common;

use chrono::{DateTime, FixedOffset};
use common::{fixtures_dir, read};
use crystalline_core::emit::{
    append_body, insert_after_section, insert_before_section, prepend_body,
    remove_frontmatter_field, replace_section, set_evolve_ack, set_frontmatter_field,
    set_frontmatter_number, set_stale_after, set_verified, touch_generated,
};
use crystalline_core::{EvolveAck, Verified, parse_engram};

fn nested_headings() -> String {
    read(&fixtures_dir().join("canonical/nested-headings.md"))
}

#[test]
fn replace_section_preserves_subsections_by_default() {
    let source = nested_headings();
    let out = replace_section(&source, "## Endpoints", "New endpoint prose.", false).unwrap();
    // The replaced prose is present.
    assert!(out.contains("New endpoint prose."));
    // The subsections and their content survive.
    assert!(out.contains("### Auth"));
    assert!(out.contains("Tokens are issued at the auth endpoint."));
    assert!(out.contains("### Data"));
    // The old prose is gone.
    assert!(!out.contains("The endpoints are grouped by concern."));
    // The following sibling section is untouched.
    assert!(out.contains("## Errors"));
    assert!(out.contains("Every error carries a stable code."));
}

#[test]
fn replace_section_can_include_subsections() {
    let source = nested_headings();
    let out = replace_section(&source, "## Endpoints", "Everything replaced.", true).unwrap();
    assert!(out.contains("Everything replaced."));
    assert!(!out.contains("### Auth"));
    assert!(!out.contains("### Data"));
    // A sibling section past the boundary is still preserved.
    assert!(out.contains("## Errors"));
}

#[test]
fn replace_nested_subsection_by_path() {
    let source = nested_headings();
    let out = replace_section(&source, "## Endpoints > ### Auth", "New auth text.", false).unwrap();
    assert!(out.contains("New auth text."));
    assert!(!out.contains("Tokens are issued at the auth endpoint."));
    // The sibling subsection is untouched.
    assert!(out.contains("Readings are served from the data endpoint."));
}

#[test]
fn replace_missing_section_errors() {
    let source = nested_headings();
    assert!(replace_section(&source, "## Nope", "x", false).is_err());
}

#[test]
fn insert_before_and_after_section() {
    let source = nested_headings();
    let before = insert_before_section(&source, "## Errors", "Injected before.").unwrap();
    let idx_inject = before.find("Injected before.").unwrap();
    let idx_heading = before.find("## Errors").unwrap();
    assert!(idx_inject < idx_heading);

    let after = insert_after_section(&source, "## Endpoints", "Injected after heading.").unwrap();
    let idx_heading = after.find("## Endpoints").unwrap();
    let idx_inject = after.find("Injected after heading.").unwrap();
    let idx_body = after.find("The endpoints are grouped by concern.").unwrap();
    assert!(idx_heading < idx_inject && idx_inject < idx_body);
}

#[test]
fn append_and_prepend_body() {
    let source = nested_headings();
    let appended = append_body(&source, "Appended line.");
    assert!(appended.trim_end().ends_with("Appended line."));

    let prepended = prepend_body(&source, "Prepended line.");
    let idx_prepend = prepended.find("Prepended line.").unwrap();
    let idx_title = prepended.find("# API Overview").unwrap();
    let idx_fm = prepended.find("type: reference").unwrap();
    assert!(idx_fm < idx_prepend, "prepend must stay after frontmatter");
    assert!(idx_prepend < idx_title);
}

#[test]
fn set_frontmatter_field_replaces_existing() {
    let source = read(&fixtures_dir().join("canonical/full-frontmatter.md"));
    let out = set_frontmatter_field(&source, "status", "deprecated");
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.status.as_deref(), Some("deprecated"));
    // Only status changed; the rest still parses identically.
    assert_eq!(e.frontmatter.title, "Watering Schedules for Tomato Beds");
    assert!(out.contains("status: deprecated"));
    assert!(!out.contains("status: current"));
}

#[test]
fn set_frontmatter_field_inserts_when_absent() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    let out = set_frontmatter_field(&source, "status", "draft");
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.status.as_deref(), Some("draft"));
    assert_eq!(e.frontmatter.engram_type, "engram");
}

#[test]
fn touch_generated_records_the_actor_and_an_rfc3339_instant() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    let now: DateTime<FixedOffset> =
        DateTime::parse_from_rfc3339("2026-07-02T10:00:00+00:00").unwrap();
    let out = touch_generated(&source, "claude-code/1.0.5", now);
    assert!(
        out.contains("generated: { by: claude-code/1.0.5, at: 2026-07-02T10:00:00+00:00 }"),
        "{out}"
    );
    let e = parse_engram(&out).unwrap();
    let g = e.frontmatter.generated.unwrap();
    assert_eq!(g.by, "claude-code/1.0.5");
    assert_eq!(g.at.unwrap().to_rfc3339(), "2026-07-02T10:00:00+00:00");
}

#[test]
fn touch_generated_migrates_a_legacy_timestamp_line_in_place() {
    // A file written before the `generated` migration carries `timestamp`.
    // Editing it swaps that one line for the provenance block and leaves every
    // other byte, including the frontmatter order, exactly where it was.
    let source = read(&fixtures_dir().join("canonical/full-frontmatter.md"));
    let now: DateTime<FixedOffset> =
        DateTime::parse_from_rfc3339("2026-07-02T10:00:00+00:00").unwrap();
    let out = touch_generated(&source, "human:jordi", now);
    let expected = source.replace(
        "timestamp: 2026-05-01T09:15:00+00:00",
        "generated: { by: human:jordi, at: 2026-07-02T10:00:00+00:00 }",
    );
    assert_eq!(out, expected);
    let e = parse_engram(&out).unwrap();
    assert!(e.frontmatter.timestamp.is_none());
    assert_eq!(e.frontmatter.generated.unwrap().by, "human:jordi");
}

#[test]
fn touch_generated_updates_the_generated_line_and_leaves_a_legacy_timestamp_alone() {
    // Both keys present: the canonical one is refreshed and the legacy line is
    // left where it is rather than a second provenance block appearing.
    let source = "---
type: engram
timestamp: 2020-01-01T00:00:00+00:00
generated: { by: old/1, at: 2021-01-01T00:00:00+00:00 }
---

body
";
    let now: DateTime<FixedOffset> =
        DateTime::parse_from_rfc3339("2026-07-02T10:00:00+00:00").unwrap();
    let out = touch_generated(source, "new/2", now);
    assert!(
        out.contains("timestamp: 2020-01-01T00:00:00+00:00"),
        "{out}"
    );
    assert_eq!(out.matches("generated:").count(), 1, "{out}");
    let e = parse_engram(&out).unwrap();
    let fm = e.frontmatter;
    assert_eq!(fm.generated.as_ref().unwrap().by, "new/2");
    // Recency reads the provenance block, not the stale legacy key.
    assert_eq!(
        fm.written_at().unwrap().to_rfc3339(),
        "2026-07-02T10:00:00+00:00"
    );
}

#[test]
fn set_stale_after_migrates_a_legacy_review_after_line_in_place() {
    // A file written before the `stale_after` migration carries `review_after`.
    // Setting the bound swaps that one line and leaves every other byte,
    // including the frontmatter order, exactly where it was.
    let source = read(&fixtures_dir().join("canonical/full-frontmatter.md"));
    let date = chrono::NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
    let out = set_stale_after(&source, date);
    let expected = source.replace("review_after: 2026-08-01", "stale_after: 2026-12-01");
    assert_eq!(out, expected);
    let e = parse_engram(&out).unwrap();
    assert!(e.frontmatter.review_after.is_none());
    assert_eq!(e.frontmatter.stale_on().unwrap().to_string(), "2026-12-01");
}

#[test]
fn set_stale_after_appends_when_neither_spelling_is_present() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    let date = chrono::NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
    let out = set_stale_after(&source, date);
    assert!(out.contains("stale_after: 2026-12-01"), "{out}");
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.stale_after.unwrap().to_string(), "2026-12-01");
}

#[test]
fn remove_frontmatter_field_removes_exactly_one_line() {
    let source = read(&fixtures_dir().join("canonical/full-frontmatter.md"));
    let out = remove_frontmatter_field(&source, "valid_to");
    // Exactly the `valid_to:` line is gone; every other byte is preserved.
    let expected = source.replace("valid_to: 2026-09-30\n", "");
    assert_eq!(out, expected);
    // The neighboring fields whose names share the `valid_` prefix survive.
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.valid_to, None);
    assert_eq!(
        e.frontmatter.valid_from,
        chrono::NaiveDate::from_ymd_opt(2026, 5, 1)
    );
}

#[test]
fn remove_frontmatter_field_missing_key_is_a_noop() {
    let source = read(&fixtures_dir().join("canonical/full-frontmatter.md"));
    let out = remove_frontmatter_field(&source, "nonexistent");
    assert_eq!(out, source);
}

#[test]
fn remove_frontmatter_field_without_a_block_is_a_noop() {
    let source = "# Just a body\n\nNo frontmatter here at all.\n";
    let out = remove_frontmatter_field(source, "valid_to");
    assert_eq!(out, source);
}

#[test]
fn set_frontmatter_number_writes_a_bare_yaml_number() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    let out = set_frontmatter_number(&source, "salience", 7.0);
    // A whole value carries no fractional part and no quotes, so the index
    // reads it as a number rather than a string.
    assert!(out.contains("salience: 7"), "{out}");
    let e = parse_engram(&out).unwrap();
    assert_eq!(
        e.frontmatter.extra.get("salience"),
        Some(&crystalline_core::YamlValue::Int(7))
    );

    // Replacing keeps one line, and a fractional value survives as a float.
    let out = set_frontmatter_number(&out, "salience", 7.5);
    assert_eq!(out.matches("salience:").count(), 1, "{out}");
    let e = parse_engram(&out).unwrap();
    assert_eq!(
        e.frontmatter.extra.get("salience"),
        Some(&crystalline_core::YamlValue::Float(7.5))
    );
}

fn verification(by: &str, at: &str) -> Verified {
    Verified {
        by: by.to_string(),
        at: DateTime::parse_from_rfc3339(at).ok(),
    }
}

#[test]
fn set_verified_replaces_a_single_entry_in_place() {
    let source = "---\ntype: engram\nverified: { by: old/1, at: 2025-01-01T00:00:00+00:00 }\nstatus: stable\n---\n\nbody\n";
    let out = set_verified(
        source,
        &[verification("human:jordi", "2026-08-02T09:00:00+00:00")],
    );
    assert_eq!(out.matches("verified:").count(), 1, "{out}");
    // The following key keeps its place, so only the one line moved.
    assert!(out.contains("\nstatus: stable\n"), "{out}");
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.verified.len(), 1);
    assert_eq!(e.frontmatter.verified[0].by, "human:jordi");
}

#[test]
fn set_verified_replaces_a_block_sequence_without_orphaning_items() {
    // The multi-entry form is a block sequence, so the old items have to go
    // with the key line or the frontmatter stops parsing.
    let source = "---\ntype: engram\nverified:\n- { by: a/1, at: 2025-01-01T00:00:00+00:00 }\n- { by: b/2, at: 2025-02-01T00:00:00+00:00 }\nstatus: stable\n---\n\nbody\n";
    let out = set_verified(
        source,
        &[
            verification("a/1", "2026-01-01T00:00:00+00:00"),
            verification("c/3", "2026-02-01T00:00:00+00:00"),
        ],
    );
    assert!(!out.contains("b/2"), "the old entries must be gone: {out}");
    assert!(out.contains("\nstatus: stable\n"), "{out}");
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.verified.len(), 2);
    assert_eq!(e.frontmatter.verified[1].by, "c/3");
    assert_eq!(
        e.frontmatter.latest_verified().unwrap().at.to_rfc3339(),
        "2026-02-01T00:00:00+00:00"
    );
}

#[test]
fn set_verified_appends_and_leaves_a_legacy_last_verified_alone() {
    // `last_verified` is a bare date with no actor, so it is left exactly where
    // it is rather than being overwritten by a record it cannot express.
    let source = read(&fixtures_dir().join("canonical/full-frontmatter.md"));
    let out = set_verified(
        &source,
        &[verification("human:jordi", "2026-08-02T09:00:00+00:00")],
    );
    assert!(out.contains("last_verified: 2026-05-01"), "{out}");
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.verified.len(), 1);
    // The newer typed entry is what recency reads.
    let latest = e.frontmatter.latest_verified().unwrap();
    assert_eq!(latest.by, Some("human:jordi"));
    assert_eq!(latest.at.to_rfc3339(), "2026-08-02T09:00:00+00:00");
}

#[test]
fn set_verified_with_no_entries_is_a_noop() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    assert_eq!(set_verified(&source, &[]), source);
}

#[test]
fn set_frontmatter_field_quotes_ambiguous_values() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    let out = set_frontmatter_field(&source, "status", "true");
    // The value is an ambiguous scalar and must be quoted so it stays a string.
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.status.as_deref(), Some("true"));
}

// --- evolve_ack --------------------------------------------------------------

fn ack(rule: &str, scope: Option<&str>, note: Option<&str>) -> EvolveAck {
    EvolveAck {
        rule: rule.to_string(),
        scope: scope.map(str::to_string),
        note: note.map(str::to_string),
        by: "human:jordi".to_string(),
        at: DateTime::parse_from_rfc3339("2026-08-20T09:00:00+00:00").ok(),
    }
}

/// An engram carrying acknowledgments parses, emits and parses again with the
/// list intact - the round trip the frontmatter convention rests on.
#[test]
fn evolve_ack_round_trips_through_the_frontmatter() {
    let source = "---\ntitle: Lineage\ntype: engram\nstatus: stable\n---\n\nBody.\n";
    let entries = vec![
        ack(
            "V101",
            Some("eng/old-runbook"),
            Some("lineage citation, keep"),
        ),
        ack("V007", None, None),
    ];
    let out = set_evolve_ack(source, &entries);
    assert!(out.contains("evolve_ack:\n- { rule: V101"), "{out}");
    assert!(out.contains("scope: eng/old-runbook"), "{out}");
    assert!(out.contains("note: \"lineage citation, keep\""), "{out}");
    assert!(out.contains("by: human:jordi"), "{out}");
    assert!(out.contains("Body."), "{out}");

    let engram = parse_engram(&out).expect("an engram with acknowledgments parses");
    let value = engram
        .frontmatter
        .extra
        .get("evolve_ack")
        .expect("the key survives into extra");
    assert_eq!(EvolveAck::parse_list(value), entries);
}

/// One entry stays on the key's own line, and replacing the list replaces the
/// whole block rather than orphaning the old sequence.
#[test]
fn a_single_ack_is_one_line_and_a_rewrite_replaces_the_block() {
    let source = "---\ntitle: Lineage\nstatus: stable\n---\n\nBody.\n";
    let one = set_evolve_ack(source, &[ack("V101", Some("eng/old"), None)]);
    assert!(
        one.contains("evolve_ack: { rule: V101, scope: eng/old, by: human:jordi"),
        "{one}"
    );

    let two = set_evolve_ack(
        &one,
        &[ack("V101", Some("eng/old"), None), ack("V104", None, None)],
    );
    assert_eq!(two.matches("rule: V101").count(), 1, "{two}");
    assert!(two.contains("rule: V104"), "{two}");
    assert!(two.contains("status: stable"), "{two}");

    let back = set_evolve_ack(&two, &[ack("V104", None, None)]);
    assert!(!back.contains("V101"), "{back}");
    assert_eq!(back.matches("evolve_ack").count(), 1, "{back}");
}

/// Withdrawing the last acknowledgment removes the key and its continuation
/// lines, leaving every other byte alone.
#[test]
fn an_empty_ack_list_removes_the_key_whole() {
    let source = "---\ntitle: Lineage\nstatus: stable\n---\n\nBody.\n";
    let two = set_evolve_ack(source, &[ack("V101", None, None), ack("V104", None, None)]);
    let cleared = set_evolve_ack(&two, &[]);
    assert_eq!(cleared, source, "the source is byte-identical again");
}

/// A hand-written entry survives its neighbours being malformed, and an entry
/// with no rule is skipped rather than failing the read.
#[test]
fn malformed_ack_entries_are_skipped_never_an_error() {
    let source = "---\ntitle: Lineage\nstatus: stable\nevolve_ack:\n- { note: no rule here }\n- { rule: V101, note: keep }\n- just a string\n---\n\nBody.\n";
    let engram = parse_engram(source).expect("a malformed entry never breaks the parse");
    let parsed = EvolveAck::parse_list(
        engram
            .frontmatter
            .extra
            .get("evolve_ack")
            .expect("the key is there"),
    );
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].rule, "V101");
    assert_eq!(parsed[0].note.as_deref(), Some("keep"));
    assert_eq!(parsed[0].scope, None);
    assert_eq!(parsed[0].by, "");
}
