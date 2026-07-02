//! Integration tests for `crystalline import` against the
//! `tests/fixtures/import-source` fixture: a small invented tree exhibiting
//! legacy type values, a source-prefixed permalink convention, a sentinel
//! `valid_to`/`valid_from`, missing temporal fields, comma-string tags, a
//! file that must stay untouched, a non-markdown asset and a permalink
//! collision.

mod common;

use std::path::Path;

use assert_cmd::Command;
use common::fixtures_dir;
use predicates::prelude::*;
use serde_json::Value;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Scaffold and register a fresh domain, returning its root path.
fn setup_domain(work: &Path, name: &str, config: &Path) -> std::path::PathBuf {
    let domain_dir = work.join(format!("kb-{name}"));
    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", name])
        .assert()
        .success();
    bin()
        .args(["domain", "add", name])
        .arg(&domain_dir)
        .arg("--config")
        .arg(config)
        .assert()
        .success();
    domain_dir
}

/// Every regular file under `dir`, relative and forward-slashed, sorted.
fn list_files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Recursively copy a directory tree.
fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

#[test]
fn dry_run_changes_nothing_on_disk() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = setup_domain(work.path(), "widgets", &config);

    let before = list_files(&domain_dir);

    bin()
        .args(["import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "widgets", "--dry-run", "--config"])
        .arg(&config)
        .assert()
        .success();

    let after = list_files(&domain_dir);
    assert_eq!(before, after, "dry run must not change the domain folder");
}

#[test]
fn dry_run_json_report_matches_the_fixture_conventions() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    setup_domain(work.path(), "widgets", &config);

    let output = bin()
        .args(["--json", "import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "widgets", "--dry-run", "--config"])
        .arg(&config)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(report["files_converted"], 10);
    assert_eq!(report["files_copied"], 1);
    assert_eq!(report["files_skipped"], 0);
    assert_eq!(report["type_mapped"], 7);
    assert_eq!(report["temporal_backfilled"], 2);
    assert_eq!(report["sentinels_dropped"], 2);
    assert_eq!(report["prefixes_stripped"], 9);
    assert_eq!(report["collisions"], 1);
}

#[test]
fn real_import_then_verify_passes_with_zero_errors() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = setup_domain(work.path(), "widgets", &config);

    bin()
        .args(["import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "widgets", "--config"])
        .arg(&config)
        .assert()
        .success();

    // Every source file landed at the same relative path, plus the
    // pre-existing MANIFEST.md.
    assert!(domain_dir.join("guides/setup-guide.md").exists());
    assert!(domain_dir.join("assets/README.txt").exists());
    assert!(domain_dir.join("MANIFEST.md").exists());

    let verify_output = bin()
        .args(["--json", "verify"])
        .arg(&domain_dir)
        .assert()
        .success() // exit 0: verify only fails the process on errors.
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&verify_output).unwrap();
    assert_eq!(
        report["summary"]["errors"], 0,
        "verify found errors: {report}"
    );
}

#[test]
fn collision_is_resolved_with_a_suffix() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = setup_domain(work.path(), "widgets", &config);

    bin()
        .args(["import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "widgets", "--config"])
        .arg(&config)
        .assert()
        .success();

    let first = std::fs::read_to_string(domain_dir.join("collision/first.md")).unwrap();
    let second = std::fs::read_to_string(domain_dir.join("collision/second.md")).unwrap();
    assert!(first.contains("permalink: shared-topic\n"));
    assert!(second.contains("permalink: shared-topic-imported-1\n"));
}

#[test]
fn a_file_with_valid_status_stays_untouched() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = setup_domain(work.path(), "widgets", &config);

    let before =
        std::fs::read_to_string(fixtures_dir().join("import-source/stable/current-fact.md"))
            .unwrap();

    bin()
        .args(["import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "widgets", "--config"])
        .arg(&config)
        .assert()
        .success();

    let after = std::fs::read_to_string(domain_dir.join("stable/current-fact.md")).unwrap();
    assert_eq!(
        before, after,
        "a fully canonical file must be byte-identical"
    );
}

#[test]
fn reimporting_the_converted_output_produces_zero_transformations() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = setup_domain(work.path(), "widgets", &config);

    bin()
        .args(["import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "widgets", "--config"])
        .arg(&config)
        .assert()
        .success();

    // Re-import the importer's own output (excluding the pre-existing
    // MANIFEST.md, which was never part of the source tree) and confirm the
    // dry-run report shows zero transformations of every kind.
    let reimport_src = work.path().join("reimport-src");
    copy_dir(&domain_dir, &reimport_src);
    std::fs::remove_file(reimport_src.join("MANIFEST.md")).unwrap();

    let output = bin()
        .args(["--json", "import"])
        .arg(&reimport_src)
        .args(["--domain", "widgets", "--dry-run", "--config"])
        .arg(&config)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).unwrap();

    assert_eq!(report["type_mapped"], 0);
    assert_eq!(report["temporal_backfilled"], 0);
    assert_eq!(report["sentinels_dropped"], 0);
    assert_eq!(report["prefixes_stripped"], 0);
    assert_eq!(report["collisions"], 0);
    for file in report["files"].as_array().unwrap() {
        assert!(
            file["changes"].as_array().unwrap().is_empty(),
            "unexpected change on re-import: {file}"
        );
    }
}

#[test]
fn map_override_replaces_a_default_mapping() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = setup_domain(work.path(), "widgets", &config);

    let map_file = work.path().join("types.yaml");
    std::fs::write(&map_file, "mappings:\n  class: architecture\n").unwrap();

    bin()
        .args(["import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "widgets", "--config"])
        .arg(&config)
        .args(["--map"])
        .arg(&map_file)
        .assert()
        .success();

    let out = std::fs::read_to_string(domain_dir.join("reference/api-reference.md")).unwrap();
    assert!(out.contains("type: architecture\n"));
    assert!(out.contains("- class\n"), "original type kept as a tag");
}

#[test]
fn missing_domain_registration_is_an_error_suggesting_domain_add() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");

    bin()
        .args(["import"])
        .arg(fixtures_dir().join("import-source"))
        .args(["--domain", "does-not-exist", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicate::str::contains("domain add"));
}
