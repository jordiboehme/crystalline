//! Integration tests for `crystalline prompt` against a fixture config and
//! workspace, snapshotted with `insta`.

mod common;

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
            "--workspace",
            "workspace",
            "--config",
            "config.yaml",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("gardening"));
}
