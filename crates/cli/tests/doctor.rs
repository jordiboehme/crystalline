//! Integration tests for `crystalline doctor`.
//!
//! Domain-level checks (orphans, unindexed files, config sanity) only need an
//! explicit `--config`/`--db`, matching the other data-command tests. The
//! stale lock/socket check additionally needs control over the state
//! directory, so that scenario isolates `HOME`/`XDG_*` the same way the
//! service integration tests do.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

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

fn engram(title: &str, permalink: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody for {title} with enough content.\n\nSecond line.\n"
    )
}

/// Scaffold and register a fresh domain, returning its root path. `domain add`
/// now indexes on registration, so it needs the same `--db` the test's own
/// later sync/doctor calls use (`work/index.db`), rather than the machine's
/// default state directory.
fn setup_domain(work: &Path, name: &str, config: &Path) -> PathBuf {
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
        .arg("--db")
        .arg(work.join("index.db"))
        .assert()
        .success();
    domain_dir
}

#[test]
fn reports_clean_when_nothing_is_wrong() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "a.md", &engram("A", "a"));

    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["domains"][0]["orphans"], serde_json::json!([]));
    assert_eq!(report["domains"][0]["unindexed"], serde_json::json!([]));
    assert_eq!(report["domains"][0]["path_exists"], serde_json::json!(true));
    assert_eq!(
        report["domains"][0]["manifest_present"],
        serde_json::json!(true)
    );
}

#[test]
fn detects_orphan_and_fix_removes_it() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "a.md", &engram("A", "a"));
    write(&domain_dir, "b.md", &engram("B", "b"));

    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // Plant the orphan: the file vanishes without a resync.
    std::fs::remove_file(domain_dir.join("b.md")).unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["domains"][0]["orphans"], serde_json::json!(["b.md"]));

    // --fix removes the orphan row and the report shows zero problems.
    let fixed_out = bin()
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fixed: Value = serde_json::from_slice(&fixed_out).unwrap();
    assert_eq!(fixed["domains"][0]["orphans_removed"], serde_json::json!(1));

    // A clean re-run confirms the row is really gone.
    let clean = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let clean: Value = serde_json::from_slice(&clean).unwrap();
    assert_eq!(clean["domains"][0]["orphans"], serde_json::json!([]));
}

#[test]
fn detects_unindexed_files_without_fixing_them() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "a.md", &engram("A", "a"));

    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // A new file lands on disk after the last sync.
    write(&domain_dir, "c.md", &engram("C", "c"));

    let out = bin()
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1) // unindexed files are report-only, never auto-fixed.
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["unindexed"],
        serde_json::json!(["c.md"])
    );
}

#[test]
fn detects_a_registered_domain_whose_path_vanished() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    std::fs::remove_dir_all(&domain_dir).unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["path_exists"],
        serde_json::json!(false)
    );
}

#[test]
fn detects_a_domain_path_that_lost_its_manifest() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    std::fs::remove_file(domain_dir.join("MANIFEST.md")).unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["domains"][0]["path_exists"], serde_json::json!(true));
    assert_eq!(
        report["domains"][0]["manifest_present"],
        serde_json::json!(false)
    );
}

#[test]
fn domain_filter_restricts_checks_to_one_domain() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    setup_domain(work.path(), "eng", &config);
    setup_domain(work.path(), "product", &config);

    let out = bin()
        .args(["--json", "doctor", "--domain", "eng", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let domains = report["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["name"], serde_json::json!("eng"));
}

/// Stale lock/socket detection needs control over the state directory, which
/// is only reachable through `HOME`/`XDG_*`, not a CLI flag. Isolated with a
/// short-path temp `HOME`, the same technique the service integration tests
/// use for their stale-lock scenario.
#[test]
#[cfg(unix)]
fn detects_and_fixes_a_stale_lock_and_orphaned_socket() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let home = PathBuf::from("/tmp").join(format!("cq-doctor-{nanos}"));
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::create_dir_all(home.join("state")).unwrap();
    std::fs::create_dir_all(home.join("cache")).unwrap();
    let state_dir = home.join("state/crystalline");
    std::fs::create_dir_all(&state_dir).unwrap();

    let apply = |cmd: &mut Command| {
        cmd.env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CACHE_HOME", home.join("cache"));
    };

    // A lock file naming a pid that cannot possibly be alive, plus an
    // orphaned socket file, simulating a `kill -9`'d daemon.
    std::fs::write(
        state_dir.join("service.lock"),
        r#"{"pid":2147483647,"socket_path":"x","version":"0.0.0","started_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    std::fs::write(state_dir.join("service.sock"), b"").unwrap();

    let mut cmd = bin();
    apply(&mut cmd);
    let out = cmd
        .args(["--json", "doctor"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["service"]["lock_stale"], serde_json::json!(true));
    assert_eq!(
        report["service"]["socket_orphaned"],
        serde_json::json!(true)
    );

    let mut fix_cmd = bin();
    apply(&mut fix_cmd);
    let fixed_out = fix_cmd
        .args(["--json", "doctor", "--fix"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fixed: Value = serde_json::from_slice(&fixed_out).unwrap();
    assert_eq!(fixed["service"]["lock_removed"], serde_json::json!(true));
    assert_eq!(fixed["service"]["socket_removed"], serde_json::json!(true));
    assert!(!state_dir.join("service.lock").exists());
    assert!(!state_dir.join("service.sock").exists());

    let _ = std::fs::remove_dir_all(&home);
}

/// Origin state (like the service lock/socket above) lives under the state
/// directory, reachable only through `HOME`/`XDG_*`, never a CLI flag, so the
/// three tests below isolate a short-path temp `HOME` the same way.
fn isolated_home(tag: &str) -> (PathBuf, PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let home = PathBuf::from("/tmp").join(format!("cq-doctor-{tag}-{nanos}"));
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::create_dir_all(home.join("state")).unwrap();
    std::fs::create_dir_all(home.join("cache")).unwrap();
    let state_dir = home.join("state/crystalline");
    std::fs::create_dir_all(&state_dir).unwrap();
    (home, state_dir)
}

fn apply_home(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"));
}

/// A team domain's config entry, with a real MANIFEST.md at `domain_dir` so
/// the ordinary per-domain checks report clean and the assertions below stay
/// focused on the github section.
fn write_team_domain_config(config: &Path, domain_dir: &Path) {
    std::fs::create_dir_all(domain_dir).unwrap();
    std::fs::write(domain_dir.join("MANIFEST.md"), "# Manifest\n").unwrap();
    std::fs::write(
        config,
        format!(
            "domains:\n  brand:\n    path: {}\n    origin:\n      repo: acme/brand-knowledge\n      branch: main\ngithub:\n  enabled: true\n",
            domain_dir.display()
        ),
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn github_section_reports_disconnected_and_an_intact_origin_as_clean() {
    let (home, state_dir) = isolated_home("intact");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    write_team_domain_config(&config, &work.path().join("kb-brand"));

    // A base snapshot matching its recorded stamp exactly (sha256 and size of
    // the literal bytes written below).
    let origin_dir = state_dir.join("origins/brand");
    std::fs::create_dir_all(origin_dir.join("base")).unwrap();
    std::fs::write(origin_dir.join("base/a.md"), "# Team\n\nHello.\n").unwrap();
    std::fs::write(
        origin_dir.join("state.json"),
        r#"{"version":1,"repo":"acme/brand-knowledge","branch":"main","base_commit":"abc123","ref_etag":null,"last_checked":null,"files":{"a.md":{"sha256":"c3c11220a2499569be3fefd408a950e49125ad33d587a26dadbcb210127098fc","size":15}},"proposals":[],"history":[],"conflicts":[]}"#,
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["github"]["connected"], serde_json::json!(false));
    assert_eq!(
        report["github"]["origins"][0]["name"],
        serde_json::json!("brand")
    );
    assert_eq!(
        report["github"]["origins"][0]["repo"],
        serde_json::json!("acme/brand-knowledge")
    );
    assert_eq!(
        report["github"]["origins"][0]["state_present"],
        serde_json::json!(true)
    );
    assert_eq!(
        report["github"]["origins"][0]["base_mismatches"],
        serde_json::json!([])
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn github_section_reports_missing_origin_state_as_a_problem() {
    let (home, _state_dir) = isolated_home("missing-state");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    write_team_domain_config(&config, &work.path().join("kb-brand"));
    // No `origins/brand/state.json` is ever written: the state directory was
    // lost or the domain never fully connected.

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["github"]["origins"][0]["state_present"],
        serde_json::json!(false)
    );

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("no origin state on disk"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn github_section_reports_a_base_snapshot_mismatch_as_a_problem() {
    let (home, state_dir) = isolated_home("base-mismatch");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    write_team_domain_config(&config, &work.path().join("kb-brand"));

    // State records `a.md`, but its base snapshot copy was never written (or
    // was lost), so `verify_base` finds it missing.
    let origin_dir = state_dir.join("origins/brand");
    std::fs::create_dir_all(&origin_dir).unwrap();
    std::fs::write(
        origin_dir.join("state.json"),
        r#"{"version":1,"repo":"acme/brand-knowledge","branch":"main","base_commit":"abc123","ref_etag":null,"last_checked":null,"files":{"a.md":{"sha256":"c3c11220a2499569be3fefd408a950e49125ad33d587a26dadbcb210127098fc","size":15}},"proposals":[],"history":[],"conflicts":[]}"#,
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["github"]["origins"][0]["state_present"],
        serde_json::json!(true)
    );
    assert_eq!(
        report["github"]["origins"][0]["base_mismatches"],
        serde_json::json!(["a.md"])
    );

    let _ = std::fs::remove_dir_all(&home);
}
