//! Smoke tests for the GitHub-origin CLI verbs, against a temp config and a
//! temp index, no daemon involved (the in-process path).
//!
//! Every scenario here is reachable without a network call: the CLI's own
//! flag validation (`--origin` combined with `--virtual` or `--no-sync`, or
//! `--branch` without `--origin`, or a malformed `--origin` value) runs
//! before anything talks to `crystalline-service`, and `github.enabled`
//! being off refuses before an engine method ever tries to build a GitHub
//! provider. The successful connect/update/status paths against a real (or
//! mocked) origin are covered at the engine level by
//! `crates/service/tests/origin.rs`, which injects a mock provider; there is
//! no HTTP-mocking harness in this crate to exercise them here, and
//! `connect github` needs a live GitHub connection to test end to end, so it
//! is not covered by an automated test in this crate (noted as a gap; its
//! auth building blocks are covered by `crates/remote`'s own
//! `github_auth.rs`/`github_client.rs` tests).

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

// --- domain add --origin: flag validation (no network) -----------------------

#[test]
fn domain_add_origin_and_virtual_conflict() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    bin()
        .args(["domain", "add", "brand", "--origin", "acme/brand-knowledge"])
        .args(["--virtual", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("--virtual"));
}

#[test]
fn domain_add_origin_and_no_sync_conflict() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    bin()
        .args(["domain", "add", "brand", "--origin", "acme/brand-knowledge"])
        .args(["--no-sync", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("--no-sync"));
}

#[test]
fn domain_add_branch_without_origin_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let dir = work.path().join("kb");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), "# Manifest").unwrap();
    bin()
        .args(["domain", "add", "eng"])
        .arg(&dir)
        .args(["--branch", "main", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("--origin"));
}

#[test]
fn domain_add_origin_rejects_a_malformed_spec() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    bin()
        .args(["domain", "add", "brand", "--origin", "not-a-repo"])
        .args(["--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("owner/repo"));
}

// --- gating: github.enabled, reached through the real CLI plumbing -----------

#[test]
fn domain_add_origin_refuses_when_github_is_not_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");
    bin()
        .args(["domain", "add", "brand", "--origin", "acme/brand-knowledge"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));
}

#[test]
fn origin_update_and_status_refuse_when_github_is_not_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["origin", "update", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));

    bin()
        .args(["origin", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));
}

#[test]
fn origin_update_and_status_succeed_with_no_team_domains_once_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["config", "set", "github.enabled", "true", "--config"])
        .arg(&config)
        .assert()
        .success();

    let out = bin()
        .args(["--json", "origin", "update", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["domains"].as_array().unwrap().len(), 0);
    assert_eq!(data["errors"].as_array().unwrap().len(), 0);

    let out = bin()
        .args(["--json", "origin", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["connection"]["connected"], false);
    assert_eq!(data["domains"].as_array().unwrap().len(), 0);

    // The human render mentions no domains and the disconnected state,
    // without panicking on the empty arrays.
    let human = bin()
        .args(["origin", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("not connected"), "{human}");
    assert!(human.contains("No team domains"), "{human}");
}
