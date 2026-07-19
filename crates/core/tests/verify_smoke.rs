//! A fast, synthetic smoke test for the verify engine, ahead of the larger
//! fixture-corpus integration tests owned by the CLI crate. Exercises the
//! scanner, a representative rule from each family and the JSON/github
//! reporters against a couple of temp-dir domains.

use std::fs;
use std::path::Path;

use crystalline_core::parse_engram;
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
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n## Scope\n\n- Facts about the solar system\n\n## When to Use\n\n- When asked about moons or planets\n",
    );
    write(
        dir.path(),
        "phobos.md",
        "---\ntype: engram\ntitle: Phobos\npermalink: phobos\ntags:\n- moons\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n# Phobos\n\nThe larger and closer of the two small moons that orbit Mars. It completes\nan orbit in under eight hours.\n",
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
        "---\ntype: engram\ntitle: Phobos\npermalink: phobos\ntags:\n- moons\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody text with enough lines.\nSecond line.\nThird line.\n",
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
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Gardening facts\n\n## When to Use\n\n- When asked about plants\n",
    );
    // Missing tags/status/recorded_at, near-empty body -> E004, T001 (x2), Q001.
    write(
        dir.path(),
        "bad.md",
        "---\ntype: engram\ntitle: Bad Engram\npermalink: bad\n---\n\nOne line only.\n",
    );

    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let rules: Vec<&str> = report.issues.iter().map(|i| i.rule).collect();
    assert!(rules.contains(&"E004"), "{rules:?}");
    assert!(rules.contains(&"T001"), "{rules:?}");
    assert!(rules.contains(&"Q001"), "{rules:?}");
}

#[test]
fn a_timestamp_in_a_date_field_is_a_t003_error() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Facts about time\n\n## When to Use\n\n- When asked about dates\n",
    );
    // A day-granular field carrying an RFC 3339 timestamp: the parser parks it
    // in `extra`, so the temporal rule flags it as not a plain ISO date.
    write(
        dir.path(),
        "timey.md",
        "---\ntype: engram\ntitle: Timey\npermalink: timey\ntags:\n- t\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\nvalid_from: 2026-07-15T10:30:00Z\n---\n\n# Timey\n\nA date field must be a plain ISO date, never a full timestamp with a time.\n",
    );

    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let t003 = report
        .issues
        .iter()
        .find(|i| i.rule == "T003" && i.path.ends_with("timey.md"))
        .expect("T003 present");
    assert_eq!(t003.severity, Severity::Error);
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn check_temporal_reports_timestamp_and_sentinel_together() {
    // `valid_from` carries a timestamp (T003) and `valid_to` a sentinel
    // far-future date (T008); the per-engram check surfaces both.
    let engram = parse_engram(
        "---\ntype: engram\ntitle: Timey\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\nvalid_from: 2026-07-15T10:30:00Z\nvalid_to: 9000-01-01\n---\n\nBody.\n",
    )
    .unwrap();
    let issues = verify::check_temporal(Path::new("timey.md"), &engram);
    let rules: Vec<&str> = issues.iter().map(|i| i.rule).collect();
    assert!(rules.contains(&"T003"), "{rules:?}");
    assert!(rules.contains(&"T008"), "{rules:?}");
}

#[test]
fn domain_prefixed_permalink_is_e008_warning() {
    let dir = tempdir().unwrap();
    // The domain root's folder name IS the domain name the scanner derives.
    let root = dir.path().join("astronomy");
    write(
        &root,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Facts about the solar system\n\n## When to Use\n\n- When asked about moons or planets\n",
    );
    // The permalink repeats the domain name without a folder to justify it.
    write(
        &root,
        "phobos.md",
        "---\ntype: engram\ntitle: Phobos\npermalink: astronomy/phobos\ntags:\n- moons\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Phobos\n\nThe larger and closer of the two small moons that orbit Mars. It completes\nan orbit in under eight hours.\n",
    );

    let report = verify::verify_paths([&root], &VerifyOptions::default()).unwrap();
    let e008 = report
        .issues
        .iter()
        .find(|i| i.rule == "E008")
        .expect("E008 present");
    assert_eq!(e008.severity, Severity::Warning);
    assert!(e008.message.contains("domain-relative"), "{}", e008.message);
    // A warning alone never fails the run.
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn a_real_subfolder_sharing_the_domain_name_is_not_e008() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("astronomy");
    write(
        &root,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Facts about the solar system\n\n## When to Use\n\n- When asked about moons or planets\n",
    );
    // The file genuinely lives in a subfolder named like the domain, so the
    // permalink is the path made explicit - correct by path, not a prefix.
    write(
        &root,
        "astronomy/deimos.md",
        "---\ntype: engram\ntitle: Deimos\npermalink: astronomy/deimos\ntags:\n- moons\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Deimos\n\nThe smaller and farther of the two moons of Mars. It completes an orbit\nin about thirty hours.\n",
    );

    let report = verify::verify_paths([&root], &VerifyOptions::default()).unwrap();
    assert!(
        !report.issues.iter().any(|i| i.rule == "E008"),
        "a path-explained prefix must not warn: {:#?}",
        report.issues
    );
}

#[test]
fn strict_promotes_warning_rules_to_error() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Gardening facts\n",
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

// --- Provisioning rules (M005, M104, M105, M106) and scan exclusion ----------

/// A structurally valid harbor MANIFEST whose `## Provisioning` section carries
/// `prov`, so a provisioning rule fires in isolation from the other M-rules.
fn manifest_with_provisioning(prov: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Charts of the harbor\n\n## When to Use\n\n- When asked about the harbor\n\n## Provisioning\n\n{prov}"
    )
}

#[test]
fn invalid_provisioning_path_is_m005_error() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: /abs\n"),
    );
    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let m005 = report
        .issues
        .iter()
        .find(|i| i.rule == "M005")
        .expect("M005 present");
    assert_eq!(m005.severity, Severity::Error);
    assert_eq!(report.exit_code(), 1);

    // A valid in-root decl (its folder present) does not trigger M005.
    let ok = tempdir().unwrap();
    write(
        ok.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: skills\n"),
    );
    std::fs::create_dir_all(ok.path().join("skills")).unwrap();
    let report = verify::verify_paths([ok.path()], &VerifyOptions::default()).unwrap();
    assert!(!report.issues.iter().any(|i| i.rule == "M005"));
}

#[test]
fn unknown_provisioning_type_is_m104_warning() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- prompts: ../prompts\n"),
    );
    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let m104 = report
        .issues
        .iter()
        .find(|i| i.rule == "M104")
        .expect("M104 present");
    assert_eq!(m104.severity, Severity::Warning);
    assert!(m104.message.contains("prompts"), "{}", m104.message);
    assert_eq!(report.exit_code(), 0);

    // A known type does not trigger M104.
    let ok = tempdir().unwrap();
    write(
        ok.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: ../skills\n"),
    );
    let report = verify::verify_paths([ok.path()], &VerifyOptions::default()).unwrap();
    assert!(!report.issues.iter().any(|i| i.rule == "M104"));
}

#[test]
fn missing_in_root_provisioning_folder_is_m105_warning() {
    // In-root decl whose folder is absent on disk: warns.
    let missing = tempdir().unwrap();
    write(
        missing.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: skills\n"),
    );
    let report = verify::verify_paths([missing.path()], &VerifyOptions::default()).unwrap();
    let m105 = report
        .issues
        .iter()
        .find(|i| i.rule == "M105")
        .expect("M105 present");
    assert_eq!(m105.severity, Severity::Warning);
    assert!(m105.message.contains("skills"), "{}", m105.message);

    // The same decl with the folder present does not warn.
    let present = tempdir().unwrap();
    write(
        present.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: skills\n"),
    );
    std::fs::create_dir_all(present.path().join("skills")).unwrap();
    let report = verify::verify_paths([present.path()], &VerifyOptions::default()).unwrap();
    assert!(!report.issues.iter().any(|i| i.rule == "M105"));

    // An out-of-root decl is never disk-checked, so a missing `../skills` does
    // not warn.
    let out = tempdir().unwrap();
    write(
        out.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: ../skills\n"),
    );
    let report = verify::verify_paths([out.path()], &VerifyOptions::default()).unwrap();
    assert!(!report.issues.iter().any(|i| i.rule == "M105"));
}

#[test]
fn malformed_and_duplicate_provisioning_are_m106_warnings() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        &manifest_with_provisioning(
            "- skills without a colon\n- skills: ../skills\n- skills: ../other\n",
        ),
    );
    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let m106: Vec<_> = report.issues.iter().filter(|i| i.rule == "M106").collect();
    assert_eq!(m106.len(), 2, "malformed + duplicate: {:#?}", report.issues);
    assert!(m106.iter().all(|i| i.severity == Severity::Warning));

    // A clean section triggers no M106.
    let ok = tempdir().unwrap();
    write(
        ok.path(),
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: ../skills\n"),
    );
    let report = verify::verify_paths([ok.path()], &VerifyOptions::default()).unwrap();
    assert!(!report.issues.iter().any(|i| i.rule == "M106"));
}

#[test]
fn provisioned_in_root_folder_is_excluded_from_the_scan() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "MANIFEST.md",
        &manifest_with_provisioning("- skills: skills\n"),
    );
    // An artifact under the declared folder must not be scanned as an engram.
    write(
        root,
        "skills/tide-tables/SKILL.md",
        "---\ntype: skill\ntitle: Tide Tables\n---\n\n# Tide Tables\n",
    );
    // A sibling engram outside the folder is still collected.
    write(
        root,
        "notes/harbor-log.md",
        "---\ntype: engram\ntitle: Harbor Log\npermalink: notes/harbor-log\ntags:\n- log\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Harbor Log\n\nThe tide came in twice today, as it always does at this port.\n",
    );
    // A near-miss sibling whose name merely starts with `skills` is a normal
    // folder: exclusion matches whole path components, not string prefixes.
    write(
        root,
        "skills-tables/berth-notes.md",
        "---\ntype: engram\ntitle: Berth Notes\npermalink: skills-tables/berth-notes\ntags:\n- log\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Berth Notes\n\nBerth three is shallow at low tide and best avoided by deep keels.\n",
    );

    let report = verify::verify_paths([root], &VerifyOptions::default()).unwrap();
    // MANIFEST.md and both engrams are scanned; only the SKILL.md is pruned.
    assert_eq!(report.summary.files_scanned, 3, "{:#?}", report.issues);
    assert!(
        !report.issues.iter().any(|i| i.path.ends_with("SKILL.md")),
        "the excluded artifact must raise no issues: {:#?}",
        report.issues
    );
}

// --- Tag alias rule (M107) ---------------------------------------------------

/// A structurally valid harbor MANIFEST whose `## Tag Aliases` section carries
/// `aliases`, so the tag-alias rule fires in isolation from the other M-rules.
fn manifest_with_tag_aliases(aliases: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n## Scope\n\n- Charts of the harbor\n\n## When to Use\n\n- When asked about the harbor\n\n## Tag Aliases\n\n{aliases}"
    )
}

#[test]
fn every_tag_alias_problem_kind_is_one_m107_warning() {
    // One bullet of each problem kind: malformed, self, duplicate,
    // non-canonical target and chained - five problems, five M107 warnings.
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        &manifest_with_tag_aliases(
            "- no arrow\n- self -> self\n- dup -> one\n- dup -> two\n- old -> Bad_Target\n- x -> y\n- y -> z\n",
        ),
    );
    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    let m107: Vec<_> = report.issues.iter().filter(|i| i.rule == "M107").collect();
    assert_eq!(m107.len(), 5, "one per problem: {:#?}", report.issues);
    assert!(m107.iter().all(|i| i.severity == Severity::Warning));
    assert!(
        m107.iter().all(|i| i.message.contains("## Tag Aliases")),
        "{m107:#?}"
    );
    // Warnings alone never fail the run.
    assert_eq!(report.exit_code(), 0);
}

#[test]
fn clean_tag_aliases_section_emits_no_m107() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "MANIFEST.md",
        &manifest_with_tag_aliases("- multi_word -> multi-word\n- old-name -> new-name\n"),
    );
    let report = verify::verify_paths([dir.path()], &VerifyOptions::default()).unwrap();
    assert!(
        !report.issues.iter().any(|i| i.rule == "M107"),
        "{:#?}",
        report.issues
    );
    assert_eq!(report.exit_code(), 0);
}
