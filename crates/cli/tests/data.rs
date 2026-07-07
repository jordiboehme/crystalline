//! End-to-end smoke test of the domain and data subcommands against a temp
//! config and a temp index. Search itself is covered by the index crate's
//! Store-API tests; the CLI search command lands in M5 with the data commands.

use std::path::Path;

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

#[test]
fn init_add_sync_status_end_to_end() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb");
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    // domain init scaffolds a MANIFEST.md.
    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "eng"])
        .assert()
        .success();
    assert!(
        domain_dir.join("MANIFEST.md").exists(),
        "manifest scaffolded"
    );

    // Add an engram to index alongside the manifest.
    write(
        &domain_dir,
        "alpha.md",
        "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nAlpha body with a searchable token.\n",
    );

    // domain add refuses without a manifest, then registers the domain.
    let no_manifest = work.path().join("empty");
    std::fs::create_dir_all(&no_manifest).unwrap();
    bin()
        .args(["domain", "add", "bad"])
        .arg(&no_manifest)
        .args(["--config"])
        .arg(&config)
        .assert()
        .failure();

    // domain add registers the domain and indexes its existing files immediately.
    let out = bin()
        .args(["--json", "domain", "add", "eng"])
        .arg(&domain_dir)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(config.exists(), "config written");
    let add_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        add_report["sync"]["added"],
        serde_json::json!(2),
        "manifest and engram indexed on add: {add_report}"
    );

    // A search finds the engram right away, with no explicit sync in between.
    let out = bin()
        .args(["--json", "search", "searchable token", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        search["total"].as_u64().unwrap() >= 1,
        "search finds the engram indexed on add: {search}"
    );

    // An explicit sync afterward is a no-op: both files are already indexed.
    let out = bin()
        .args(["--json", "sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let reports: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reports[0]["unchanged"].as_u64().unwrap(),
        2,
        "domain add already indexed both files: {reports}"
    );

    // status reports the counts and the active fts path.
    let out = bin()
        .args(["--json", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["indexed"], serde_json::json!(true));
    assert_eq!(status["fts_mode"], serde_json::json!("candidate-scan"));
    let engrams = status["domains"][0]["engrams"].as_i64().unwrap();
    assert_eq!(engrams, 2);

    // domain list shows the engram count.
    let out = bin()
        .args(["--json", "domain", "list", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let list: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(list["domains"][0]["name"], serde_json::json!("eng"));
    assert_eq!(list["domains"][0]["engrams"], serde_json::json!(2));

    // reindex --full rebuilds and still reports two engrams.
    let out = bin()
        .args(["--json", "reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let reindex: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reindex["reports"][0]["added"], serde_json::json!(2));

    // domain remove drops it from the config but leaves files.
    bin()
        .args(["domain", "remove", "eng", "--config"])
        .arg(&config)
        .assert()
        .success();
    assert!(domain_dir.join("alpha.md").exists(), "files untouched");
}

#[test]
fn domain_add_indexes_pre_existing_files_without_an_explicit_sync() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb-docs");
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "docs"])
        .assert()
        .success();
    write(
        &domain_dir,
        "beta.md",
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBeta body with a distinct findable marker.\n",
    );

    bin()
        .args(["domain", "add", "docs"])
        .arg(&domain_dir)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let out = bin()
        .args(["--json", "search", "distinct findable marker", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        search["total"].as_u64().unwrap() >= 1,
        "search finds the pre-existing file without a sync command: {search}"
    );
}

#[test]
fn read_only_config_refuses_a_cli_write() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb");
    let db = work.path().join("state/index.db");
    let config = work.path().join("config.yaml");

    // A registered domain with a manifest, and a config that serves read-only.
    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "eng"])
        .assert()
        .success();
    std::fs::write(
        &config,
        format!(
            "domains:\n  eng:\n    path: {}\nservice:\n  read_only: true\n",
            domain_dir.display()
        ),
    )
    .unwrap();

    // A standalone `crystalline write` (no daemon) refuses over the engine
    // guard, with the friendly read-only message on stderr, and writes nothing.
    bin()
        .args(["write", "eng", "Blocked", "--content", "- [fact] nope #eng"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("read-only"));
    assert!(
        !domain_dir.join("blocked.md").exists(),
        "the refused write left no file"
    );
}

#[test]
fn domain_add_no_sync_registers_without_indexing() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb-later");
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "later"])
        .assert()
        .success();
    write(
        &domain_dir,
        "gamma.md",
        "---\ntype: engram\ntitle: Gamma\npermalink: gamma\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nGamma body with an unindexed marker.\n",
    );

    let out = bin()
        .args(["--json", "domain", "add", "later"])
        .arg(&domain_dir)
        .arg("--no-sync")
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let add_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(add_report["synced"], serde_json::json!(false));

    let out = bin()
        .args(["--json", "search", "unindexed marker", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        search["total"].as_u64().unwrap(),
        0,
        "--no-sync leaves the domain unindexed: {search}"
    );
}
