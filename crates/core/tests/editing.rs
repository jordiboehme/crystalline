//! Surgical editor tests: section editing (including the subsection-preserved
//! regression), frontmatter field edits and timestamp touch.

mod common;

use chrono::{DateTime, FixedOffset};
use common::{fixtures_dir, read};
use crystalline_core::emit::{
    append_body, insert_after_section, insert_before_section, prepend_body,
    remove_frontmatter_field, replace_section, set_frontmatter_field, touch_timestamp,
};
use crystalline_core::parse_engram;

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
fn touch_timestamp_sets_rfc3339() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    let now: DateTime<FixedOffset> =
        DateTime::parse_from_rfc3339("2026-07-02T10:00:00+00:00").unwrap();
    let out = touch_timestamp(&source, now);
    let e = parse_engram(&out).unwrap();
    assert_eq!(
        e.frontmatter.timestamp.unwrap().to_rfc3339(),
        "2026-07-02T10:00:00+00:00"
    );
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
fn set_frontmatter_field_quotes_ambiguous_values() {
    let source = read(&fixtures_dir().join("canonical/minimal-okf.md"));
    let out = set_frontmatter_field(&source, "status", "true");
    // The value is an ambiguous scalar and must be quoted so it stays a string.
    let e = parse_engram(&out).unwrap();
    assert_eq!(e.frontmatter.status.as_deref(), Some("true"));
}
