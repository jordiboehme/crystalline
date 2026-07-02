//! A fast, synthetic smoke test for the verify engine, ahead of the larger
//! fixture-corpus integration tests owned by the CLI crate. Exercises the
//! scanner, a representative rule from each family and the JSON/github
//! reporters against a couple of temp-dir domains.

use std::fs;

use crystalline_core::verify::{self, Format, Severity, VerifyOptions};
use tempfile::tempdir;

fn write(dir: &std::path::Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn good_domain_has_no_issues() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: astronomy/manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n## Scope\n\n- Facts about the solar system\n\n## When to Use\n\n- When asked about moons or planets\n",
    );
    write(
        dir.path(),
        "phobos.md",
        "---\ntype: engram\ntitle: Phobos\npermalink: astronomy/phobos\ntags:\n- moons\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n# Phobos\n\nThe larger and closer of the two small moons that orbit Mars. It completes\nan orbit in under eight hours.\n",
    );

    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    assert_eq!(
        report.issues.iter().map(|i| i.rule).collect::<Vec<_>>(),
        Vec::<&str>::new(),
        "unexpected issues: {:#?}",
        report.issues
    );
    assert_eq!(report.exit_code(), 0);
    assert_eq!(report.summary.domains, 1);
    assert_eq!(report.summary.files_scanned, 2);
}

#[test]
fn missing_manifest_is_m001_error() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "phobos.md",
        "---\ntype: engram\ntitle: Phobos\npermalink: astronomy/phobos\ntags:\n- moons\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody text with enough lines.\nSecond line.\nThird line.\n",
    );

    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.rule == "M001" && i.severity == Severity::Error)
    );
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn required_field_and_encoding_and_temporal_and_quality_rules_fire() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: gardening/manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Gardening facts\n\n## When to Use\n\n- When asked about plants\n",
    );
    // Missing tags/status/recorded_at, near-empty body -> E004, T001 (x2), Q001.
    write(
        dir.path(),
        "bad.md",
        "---\ntype: engram\ntitle: Bad Engram\npermalink: gardening/bad\n---\n\nOne line only.\n",
    );

    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let rules: Vec<&str> = report.issues.iter().map(|i| i.rule).collect();
    assert!(rules.contains(&"E004"), "{rules:?}");
    assert!(rules.contains(&"T001"), "{rules:?}");
    assert!(rules.contains(&"Q001"), "{rules:?}");
}

#[test]
fn strict_promotes_warning_rules_to_error() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: gardening/manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Gardening facts\n",
    );

    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let m101 = report
        .issues
        .iter()
        .find(|i| i.rule == "M101")
        .expect("M101 present");
    assert_eq!(m101.severity, Severity::Warning);

    let strict = VerifyOptions {
        strict: true,
        ..Default::default()
    };
    let report = verify::verify_paths([dir.path()], &strict).unwrap();
    let m101 = report
        .issues
        .iter()
        .find(|i| i.rule == "M101")
        .expect("M101 present");
    assert_eq!(m101.severity, Severity::Error);
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn reporters_render_without_panicking() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "phobos.md",
        "---\ntype: engram\ntitle: Phobos\n---\n\nShort.\n",
    );
    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();

    let json = verify::to_json(&report);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["version"], 1);
    assert!(parsed["issues"].is_array());

    let github = verify::to_github(&report);
    assert!(
        github.contains("::error") || github.contains("::warning") || github.contains("::notice")
    );

    let human = verify::render(Format::Human, &report, false);
    assert!(human.contains("error(s)"));
}

#[test]
fn nonexistent_path_is_a_scan_error() {
    let err =
        verify::verify_paths(["/no/such/path/at/all"], &VerifyOptions::default()).unwrap_err();
    match err {
        verify::ScanError::NotFound(_) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}
