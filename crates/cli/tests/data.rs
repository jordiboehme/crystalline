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

    bin()
        .args(["domain", "add", "eng"])
        .arg(&domain_dir)
        .args(["--config"])
        .arg(&config)
        .assert()
        .success();
    assert!(config.exists(), "config written");

    // sync indexes the manifest and the engram.
    let out = bin()
        .args(["--json", "sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let reports: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let added = reports[0]["added"].as_u64().unwrap();
    assert_eq!(added, 2, "manifest and engram indexed");

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
    assert_eq!(
        status["store"]["fts_mode"],
        serde_json::json!("candidate-scan")
    );
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
