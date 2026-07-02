//! Integration tests for `crystalline verify` against the golden
//! `domain-good`/`domain-bad` fixture corpora.

mod common;

use std::collections::BTreeSet;

use assert_cmd::Command;
use predicates::prelude::*;

use common::fixtures_dir;

type IssueKey = (String, String, String);

fn issue_set(v: &serde_json::Value) -> BTreeSet<IssueKey> {
    v["issues"]
        .as_array()
        .expect("issues array")
        .iter()
        .map(|i| {
            (
                i["path"].as_str().unwrap().to_string(),
                i["rule"].as_str().unwrap().to_string(),
                i["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn domain_good_passes_with_zero_issues() {
    Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-good"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 error(s), 0 warning(s), 0 info"));
}

#[test]
fn domain_good_json_has_no_issues() {
    let output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-good", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(v["summary"]["errors"], 0);
    assert_eq!(v["summary"]["warnings"], 0);
    assert_eq!(v["summary"]["infos"], 0);
    assert!(v["issues"].as_array().unwrap().is_empty());
}

#[test]
fn domain_bad_exits_1_with_the_expected_issue_set() {
    let output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-bad", "--format", "json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let actual: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let actual_set = issue_set(&actual);

    let expected_raw =
        std::fs::read_to_string(fixtures_dir().join("domain-bad/expected-issues.json")).unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected_raw).unwrap();
    let expected_set = issue_set(&expected);

    let missing: Vec<_> = expected_set.difference(&actual_set).collect();
    let extra: Vec<_> = actual_set.difference(&expected_set).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "issue sets differ.\nmissing (expected but not produced): {missing:#?}\nextra (produced but not expected): {extra:#?}"
    );
}

#[test]
fn domain_bad_covers_every_rule_family() {
    let output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-bad", "--format", "json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let actual: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let rules: BTreeSet<String> = actual["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["rule"].as_str().unwrap().to_string())
        .collect();

    for family in ["E", "T", "M", "L", "S", "Q"] {
        assert!(
            rules.iter().any(|r| r.starts_with(family)),
            "no issue from rule family `{family}` was produced: {rules:?}"
        );
    }
}

#[test]
fn json_format_matches_the_stable_schema() {
    let output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-bad", "--format", "json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(v["version"], 1);
    for key in ["errors", "warnings", "infos", "files_scanned", "domains"] {
        assert!(
            v["summary"][key].is_number(),
            "summary.{key} missing or not a number"
        );
    }
    let issues = v["issues"].as_array().unwrap();
    assert!(!issues.is_empty());
    for issue in issues {
        assert!(issue["path"].is_string());
        assert!(issue["rule"].is_string());
        assert!(issue["severity"].is_string());
        assert!(issue["message"].is_string());
    }
}

#[test]
fn github_format_emits_workflow_commands() {
    Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-bad", "--format", "github"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("::error"))
        .stdout(predicate::str::contains("::warning"))
        .stdout(predicate::str::contains("::notice"));
}

#[test]
fn strict_promotes_a_warning_rule_to_error() {
    // `domain-good` is clean at default severities, so it stays clean under
    // `--strict` too (there is nothing to promote).
    Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-good", "--strict"])
        .assert()
        .success();

    // `domain-bad` carries real Warning-severity issues (for example
    // `M101`), so `--strict` must move some of them into the error count
    // without changing the total number of issues produced.
    let default_output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-bad", "--format", "json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let default_report: serde_json::Value = serde_json::from_slice(&default_output).unwrap();

    let strict_output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["verify", "domain-bad", "--strict", "--format", "json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let strict_report: serde_json::Value = serde_json::from_slice(&strict_output).unwrap();

    let default_errors = default_report["summary"]["errors"].as_u64().unwrap();
    let strict_errors = strict_report["summary"]["errors"].as_u64().unwrap();
    assert!(
        strict_errors > default_errors,
        "expected --strict to promote at least one Warning to Error: default={default_errors} strict={strict_errors}"
    );

    let default_total = default_report["issues"].as_array().unwrap().len();
    let strict_total = strict_report["issues"].as_array().unwrap().len();
    assert_eq!(
        default_total, strict_total,
        "--strict must not change the number of issues, only their severity"
    );

    let m101_severity = strict_report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["rule"] == "M101")
        .expect("M101 present in domain-bad")["severity"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(m101_severity, "error");
}

#[test]
fn global_json_flag_is_shorthand_for_format_json() {
    Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir())
        .args(["--json", "verify", "domain-bad"])
        .assert()
        .code(1)
        .stdout(predicate::str::starts_with("{"));
}

#[test]
fn nonexistent_path_is_a_usage_error() {
    Command::cargo_bin("crystalline")
        .unwrap()
        .args(["verify", "/no/such/path/at/all/really"])
        .assert()
        .code(2);
}

#[test]
fn defaults_to_current_directory_when_no_paths_given() {
    Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir().join("domain-good"))
        .args(["verify"])
        .assert()
        .success();
}
