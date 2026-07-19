//! End-to-end tests for `crystalline tags rename` / `tags merge` against a temp
//! file domain: the surgical file rewrite (only the tag tokens change), the
//! dry-run that writes nothing and the non-terminal confirmation guard.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// Frontmatter carries `topic` (and `keep`) as a block sequence; the body has a
/// prose `#topic` that must never change and a trailing observation `#topic`
/// that must. Deliberately non-canonical spacing around the heading and prose so
/// a full-file equality assertion proves nothing but the tag tokens moved.
const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - topic\n  - keep\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nProse mentioning #topic stays put.\n\n- [decision] chose it #topic (in prod)\n\nMore prose.\n";

/// Register a domain `eng` holding `alpha` (above) and `beta` (which carries the
/// `subject` tag, so a merge into it has a landing target). Returns the domain
/// directory, the config path and the db path.
fn seed(work: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let domain_dir = work.join("kb");
    let config = work.join("config.yaml");
    let db = work.join("state/index.db");

    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "eng"])
        .assert()
        .success();
    write(&domain_dir, "alpha.md", ALPHA);
    write(
        &domain_dir,
        "beta.md",
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - subject\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBeta body.\n",
    );
    bin()
        .args(["domain", "add", "eng"])
        .arg(&domain_dir)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    (domain_dir, config, db)
}

#[test]
fn rename_rewrites_only_the_tag_tokens() {
    let work = tempfile::tempdir().unwrap();
    let (domain_dir, config, db) = seed(work.path());

    bin()
        .args(["tags", "rename", "topic", "subtopic", "--yes", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicates::str::contains("Rewrote 1 engrams."));

    let after = std::fs::read_to_string(domain_dir.join("alpha.md")).unwrap();
    let expected = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - subtopic\n  - keep\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nProse mentioning #topic stays put.\n\n- [decision] chose it #subtopic (in prod)\n\nMore prose.\n";
    assert_eq!(
        after, expected,
        "only the frontmatter tag and the trailing hashtag change; the prose #topic and every other byte are preserved"
    );
}

#[test]
fn dry_run_writes_nothing_and_lists_the_engram() {
    let work = tempfile::tempdir().unwrap();
    let (domain_dir, config, db) = seed(work.path());

    let out = bin()
        .args([
            "tags",
            "rename",
            "topic",
            "subtopic",
            "--dry-run",
            "--config",
        ])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("alpha.md"),
        "the dry run lists the affected engram path: {stdout}"
    );

    let after = std::fs::read_to_string(domain_dir.join("alpha.md")).unwrap();
    assert_eq!(after, ALPHA, "a dry run rewrites nothing");
}

#[test]
fn non_terminal_without_yes_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let (domain_dir, config, db) = seed(work.path());

    bin()
        .args(["tags", "rename", "topic", "subtopic", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not a terminal; pass --yes"));

    let after = std::fs::read_to_string(domain_dir.join("alpha.md")).unwrap();
    assert_eq!(after, ALPHA, "a refused rename rewrites nothing");
}

#[test]
fn merge_folds_a_tag_into_an_existing_one() {
    let work = tempfile::tempdir().unwrap();
    let (domain_dir, config, db) = seed(work.path());

    // Merge `topic` into the existing `subject` (which Beta carries).
    bin()
        .args(["tags", "merge", "topic", "subject", "--yes", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicates::str::contains("Rewrote 1 engrams."));

    let after = std::fs::read_to_string(domain_dir.join("alpha.md")).unwrap();
    assert!(
        after.contains("- subject"),
        "frontmatter tag folded: {after}"
    );
    assert!(
        after.contains("chose it #subject (in prod)"),
        "trailing hashtag folded: {after}"
    );
    assert!(
        !after.contains("- topic"),
        "the frontmatter tag no longer says topic: {after}"
    );
    assert!(
        after.contains("Prose mentioning #topic stays put."),
        "the prose hashtag is never touched: {after}"
    );
}
