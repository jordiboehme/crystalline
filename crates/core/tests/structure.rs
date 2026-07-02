//! Structural assertions on parsed fixtures plus a few snapshot checks.

mod common;

use common::{fixtures_dir, read};
use crystalline_core::parse_engram;

fn parse_fixture(rel: &str) -> crystalline_core::Engram {
    let path = fixtures_dir().join(rel);
    parse_engram(&read(&path)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn full_frontmatter_fields_are_typed() {
    let e = parse_fixture("canonical/full-frontmatter.md");
    let fm = &e.frontmatter;
    assert_eq!(fm.engram_type, "guide");
    assert_eq!(fm.title, "Watering Schedules for Tomato Beds");
    assert_eq!(
        fm.permalink.as_deref(),
        Some("gardening/watering-schedules")
    );
    assert_eq!(fm.tags, ["gardening", "tomatoes", "watering"]);
    assert_eq!(fm.status.as_deref(), Some("current"));
    assert_eq!(fm.recorded_at.unwrap().to_string(), "2026-05-01");
    assert_eq!(fm.valid_from.unwrap().to_string(), "2026-05-01");
    assert_eq!(fm.valid_to.unwrap().to_string(), "2026-09-30");
    assert_eq!(fm.temporal_confidence.as_deref(), Some("explicit"));
    assert_eq!(
        fm.resource.as_deref(),
        Some("https://example.org/tomato-guide")
    );
    assert_eq!(
        fm.timestamp.unwrap().to_rfc3339(),
        "2026-05-01T09:15:00+00:00"
    );
    assert!(fm.extra.is_empty());
}

#[test]
fn unknown_keys_preserved_in_order() {
    let e = parse_fixture("canonical/unknown-keys-nested.md");
    let keys: Vec<&str> = e.frontmatter.extra.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["custom_source", "related_ids", "priority", "verified"]
    );
}

#[test]
fn temporal_absent_means_unbounded() {
    let e = parse_fixture("canonical/temporal-absent-both.md");
    assert!(e.frontmatter.valid_from.is_none());
    assert!(e.frontmatter.valid_to.is_none());
}

#[test]
fn observations_extract_tags_and_context() {
    let e = parse_fixture("canonical/multi-tag-observation.md");
    let obs = &e.observations[0];
    assert_eq!(obs.category, "requirement");
    assert_eq!(obs.content, "Water deeply once per week");
    assert_eq!(obs.tags, ["gardening", "tomatoes", "watering"]);
    assert_eq!(obs.context.as_deref(), Some("during peak summer"));
}

#[test]
fn nested_bullets_are_not_observations() {
    let e = parse_fixture("canonical/nested-bullets.md");
    // Only the two zero-indent `[step]` bullets are observations.
    assert_eq!(e.observations.len(), 2);
    assert!(e.observations.iter().all(|o| o.category == "step"));
}

#[test]
fn quoted_and_single_word_relations() {
    let e = parse_fixture("canonical/quoted-relation.md");
    let types: Vec<&str> = e.relations.iter().map(|r| r.rel_type.as_str()).collect();
    assert_eq!(types, ["relates to", "supersedes decision", "part_of"]);
    assert_eq!(
        e.relations[0].target.target,
        "Watering Schedules for Tomato Beds"
    );
    assert!(e.relations[0].target.domain.is_none());
}

#[test]
fn cross_domain_targets_carry_domain() {
    let e = parse_fixture("canonical/cross-domain-relation.md");
    let rel = e
        .relations
        .iter()
        .find(|r| r.rel_type == "relates_to")
        .unwrap();
    assert_eq!(rel.target.domain.as_deref(), Some("astronomy"));
    assert_eq!(rel.target.target, "Seasonal Sky Guide");
}

#[test]
fn cross_domain_and_bare_links() {
    let e = parse_fixture("canonical/cross-domain-links.md");
    let cross: Vec<(&str, &str)> = e
        .links
        .iter()
        .filter_map(|l| {
            l.target
                .domain
                .as_deref()
                .map(|d| (d, l.target.target.as_str()))
        })
        .collect();
    assert!(cross.contains(&("astronomy", "Seasonal Sky Guide")));
    assert!(cross.contains(&("product", "Greenhouse Overview")));
    // The bare link resolves with no domain prefix.
    assert!(
        e.links
            .iter()
            .any(|l| l.target.domain.is_none() && l.target.target == "Composting Basics")
    );
}

#[test]
fn links_inside_code_are_ignored() {
    let e = parse_fixture("canonical/links-in-code.md");
    let targets: Vec<&str> = e.links.iter().map(|l| l.target.target.as_str()).collect();
    assert_eq!(targets, ["Composting Basics", "Soil pH"]);
    // The fenced observation must not be captured either.
    assert!(e.observations.is_empty());
}

#[test]
fn wikilinks_deduplicated_per_line() {
    let e = parse_fixture("canonical/wikilink-dedup.md");
    let kepler = e
        .links
        .iter()
        .filter(|l| l.target.target == "Kepler Laws")
        .count();
    assert_eq!(kepler, 1);
    assert!(
        e.links
            .iter()
            .any(|l| l.target.target == "Newton Gravitation")
    );
}

#[test]
fn relation_target_not_double_counted_as_link() {
    let e = parse_fixture("canonical/single-word-relation.md");
    // The relation targets must not also appear as prose wikilinks.
    assert!(e.links.is_empty());
    assert_eq!(e.relations.len(), 2);
}

#[test]
fn headings_track_level_and_duplicates() {
    let e = parse_fixture("canonical/duplicate-headings.md");
    let terms = e
        .headings
        .iter()
        .filter(|h| h.text == "Terms" && h.level == 2)
        .count();
    assert_eq!(terms, 2);
}

#[test]
fn missing_frontmatter_yields_empty_fields() {
    let e = parse_fixture("canonical/no-frontmatter.md");
    assert_eq!(e.frontmatter.engram_type, "");
    assert!(e.frontmatter.title.is_empty());
    // The body still yields structure.
    assert_eq!(e.observations.len(), 1);
    assert_eq!(e.relations.len(), 1);
}

#[test]
fn snapshot_full_frontmatter() {
    let e = parse_fixture("canonical/full-frontmatter.md");
    insta::assert_yaml_snapshot!("full-frontmatter", e);
}

#[test]
fn snapshot_unknown_keys_nested() {
    let e = parse_fixture("canonical/unknown-keys-nested.md");
    insta::assert_yaml_snapshot!("unknown-keys-nested", e);
}

#[test]
fn snapshot_observations() {
    let e = parse_fixture("canonical/observations-tags-context.md");
    insta::assert_yaml_snapshot!("observations", e);
}

#[test]
fn snapshot_schema_engram() {
    let e = parse_fixture("schemas/schema-task.md");
    insta::assert_yaml_snapshot!("schema-engram", e);
}
