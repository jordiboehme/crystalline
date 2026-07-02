//! Manifest section extraction and routing bullets.

mod common;

use common::{fixtures_dir, read};
use crystalline_core::manifest::Manifest;
use crystalline_core::parse_engram;

fn manifest(rel: &str) -> (Manifest, String) {
    let source = read(&fixtures_dir().join(rel));
    let engram = parse_engram(&source).unwrap();
    (Manifest::from_engram(&engram, &source), source)
}

#[test]
fn valid_manifest_has_required_sections() {
    let (m, _) = manifest("manifests/manifest-valid.md");
    assert!(m.has_scope());
    assert!(m.has_when_to_use());
    assert!(m.missing_required_sections().is_empty());
    assert_eq!(m.scope().len(), 3);
    assert_eq!(m.when_to_use().len(), 3);
}

#[test]
fn routing_bullets_prefer_when_to_use() {
    let (m, _) = manifest("manifests/manifest-valid.md");
    let routing = m.routing_bullets();
    assert_eq!(routing, m.when_to_use());
    assert!(routing[0].starts_with("When a question is about growing"));
}

#[test]
fn invalid_manifest_missing_when_to_use() {
    let (m, _) = manifest("manifests/manifest-invalid.md");
    assert!(m.has_scope());
    assert!(!m.has_when_to_use());
    assert_eq!(m.missing_required_sections(), ["When to Use"]);
    // Routing falls back to Scope.
    assert_eq!(m.routing_bullets(), m.scope());
    assert_eq!(m.scope().len(), 2);
}

#[test]
fn section_matching_is_case_insensitive_and_first_wins() {
    let source = "\
---
type: manifest
title: MANIFEST
---

# KB

## scope

- first scope bullet

## Scope

- duplicate scope bullet that loses

## When To Use

- routing one
";
    let engram = parse_engram(source).unwrap();
    let m = Manifest::from_engram(&engram, source);
    assert!(m.has_scope());
    assert!(m.has_when_to_use());
    // First duplicate wins: only the earlier Scope bullet is kept.
    assert_eq!(m.scope(), ["first scope bullet"]);
    assert_eq!(m.when_to_use(), ["routing one"]);
}
