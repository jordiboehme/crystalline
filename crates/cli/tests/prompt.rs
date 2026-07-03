//! Integration tests for `crystalline prompt system` against a fixture
//! config and workspace, snapshotted with `insta`.

mod common;

use std::time::Instant;

use assert_cmd::Command;
use predicates::prelude::*;

use common::fixtures_dir;

#[test]
fn prompt_text_matches_snapshot() {
    let output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(output).unwrap();
    insta::assert_snapshot!(text);
}

#[test]
fn prompt_json_matches_snapshot() {
    let output = Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json = String::from_utf8(output).unwrap();
    insta::assert_snapshot!(json);
}

#[test]
fn missing_manifest_warns_on_stderr_but_still_exits_0() {
    Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(fixtures_dir().join("prompt-fixture"))
        .args([
            "prompt",
            "system",
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("gardening"));
}

/// Determinism contract, part (b): two entirely separate invocations of the
/// binary against the same on-disk fixture must produce byte-identical
/// stdout. This is the process-boundary counterpart to the in-process
/// double-render test in `crystalline_core::prompt`, catching anything
/// (stray env var, hash-based ordering) the in-process test would miss.
#[test]
fn prompt_system_output_is_byte_identical_across_separate_invocations() {
    let run = || {
        Command::cargo_bin("crystalline")
            .unwrap()
            .current_dir(fixtures_dir().join("prompt-fixture"))
            .args([
                "prompt",
                "system",
                "--workspace",
                "workspace",
                "--config",
                "config.yaml",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    };

    let first = run();
    let second = run();
    assert_eq!(
        first, second,
        "crystalline prompt system must produce byte-identical stdout across separate process invocations"
    );
}

/// Bare `crystalline prompt` (no kind) is a missing-subcommand error: clap
/// prints its standard subcommand help and exits non-zero, never silently
/// doing nothing.
#[test]
fn bare_prompt_without_a_kind_fails_with_subcommand_help() {
    Command::cargo_bin("crystalline")
        .unwrap()
        .args(["prompt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("system"));
}

/// Latency contract, part (c): a CI-safe guard, generous against runner
/// noise, that `prompt system` stays well under budget even at 30 scaffolded
/// domains. The real target (under 50ms wall-clock in a release build) is
/// measured and reported separately, since a debug test binary under a
/// possibly loaded CI runner is not a fair proxy for that number.
#[test]
fn prompt_system_scaffolded_30_domains_stays_under_500ms() {
    let tmp = tempfile::tempdir().unwrap();

    let mut config_yaml = String::from("domains:\n");
    for i in 0..30 {
        let name = format!("domain{i:02}");
        let dir = tmp.path().join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            format!(
                "---\ntype: manifest\ntitle: MANIFEST\npermalink: {name}/manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n# {name}\n\n## Scope\n\n- scope for {name}\n\n## When to Use\n\n- When a task touches {name}\n"
            ),
        )
        .unwrap();
        config_yaml.push_str(&format!("  {name}:\n    path: {name}\n"));
    }
    std::fs::write(tmp.path().join("config.yaml"), config_yaml).unwrap();

    let start = Instant::now();
    Command::cargo_bin("crystalline")
        .unwrap()
        .current_dir(tmp.path())
        .args(["prompt", "system", "--config", "config.yaml"])
        .assert()
        .success();
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 500,
        "prompt system took {elapsed:?} for 30 domains, expected well under the 500ms CI-safe bound"
    );
}
