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
/// Point a doctor invocation's `HOME`/`XDG_*` at a scratch directory so its
/// exit-code assertions never couple to the developer's real machine-wide
/// state (harness configs under `~/.claude` and `~/.codex`, service lock and
/// socket). A no-op on Windows, where the base-directory strategy does not
/// honor these variables; Windows CI runs on a fresh profile, so the ambient
/// state is empty there anyway.
#[allow(unused_variables)]
fn shield_ambient_home(cmd: &mut Command, tag: &str) {
    #[cfg(unix)]
    {
        let (home, _state) = isolated_home(tag);
        apply_home(cmd, &home);
    }
}

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

    let mut cmd = bin();
    shield_ambient_home(&mut cmd, "clean-run");
    let out = cmd
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
    let mut fixed_cmd = bin();
    shield_ambient_home(&mut fixed_cmd, "orphan-fix");
    let fixed_out = fixed_cmd
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
    let mut clean_cmd = bin();
    shield_ambient_home(&mut clean_cmd, "orphan-clean");
    let clean = clean_cmd
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

    let mut cmd = bin();
    shield_ambient_home(&mut cmd, "domain-filter");
    let out = cmd
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
/// tests below isolate a short-path temp `HOME` the same way. Unix-only like
/// every test that calls it: the isolation runs through `/tmp` and `HOME`.
#[cfg(unix)]
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

#[cfg(unix)]
fn apply_home(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"));
}

/// A team domain's config entry, with a real MANIFEST.md at `domain_dir` so
/// the ordinary per-domain checks report clean and the assertions below stay
/// focused on the github section.
#[cfg(unix)]
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

#[test]
#[cfg(unix)]
fn github_section_reports_the_environment_token_store_when_the_variable_is_set() {
    let (home, _state_dir) = isolated_home("env-token");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    // No domains and no on-disk `github.enabled`: turning collaboration on
    // and supplying the token both come from the environment alone, proving
    // the overlay drives `check_github` end to end.

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_GITHUB_ENABLED", "true")
        .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
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
    assert_eq!(report["github"]["connected"], serde_json::json!(true));
    assert_eq!(report["github"]["user"], serde_json::Value::Null);
    assert_eq!(
        report["github"]["token_store"],
        serde_json::json!("environment")
    );

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.env("CRYSTALLINE_GITHUB_ENABLED", "true")
            .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
            .args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("connected via CRYSTALLINE_GITHUB_TOKEN (environment token store)"),
        "{human}"
    );
    assert!(!human.contains("SECRET"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

/// A minimal config plus an env-defined domain (with a real `MANIFEST.md` so
/// the per-domain checks report clean and the assertions below stay focused
/// on the environment section), used by the three tests below.
fn setup_env_domain(work: &Path) -> (PathBuf, PathBuf) {
    let config = work.join("config.yaml");
    std::fs::write(&config, "domains: {}\n").unwrap();
    let domain_dir = work.join("kb-team");
    std::fs::create_dir_all(&domain_dir).unwrap();
    std::fs::write(domain_dir.join("MANIFEST.md"), "# Manifest\n").unwrap();
    (config, domain_dir)
}

#[test]
fn environment_section_reports_masked_and_filtered_overrides_domains_and_token() {
    let work = tempfile::tempdir().unwrap();
    let (config, domain_dir) = setup_env_domain(work.path());
    let env_config_path = work.path().join("elsewhere-config.yaml");

    let out = bin()
        .env("CRYSTALLINE_SERVICE_READ_ONLY", "true")
        .env("CRYSTALLINE_DOMAIN_TEAM", &domain_dir)
        .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
        .env("CRYSTALLINE_CONFIG", &env_config_path)
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .output()
        .unwrap()
        .stdout;
    let report: Value = serde_json::from_slice(&out).unwrap();
    let env = &report["environment"];

    // The `--config` flag wins for what actually loads, but the overlay still
    // records the variable itself, independent of which path won.
    assert_eq!(
        env["config_path_var"],
        serde_json::json!(env_config_path.display().to_string())
    );

    let overrides = env["overrides"].as_array().unwrap();
    assert!(
        overrides
            .iter()
            .any(|o| o["var"] == "CRYSTALLINE_SERVICE_READ_ONLY"
                && o["key"] == "service.read_only"
                && o["value"] == "true"),
        "{overrides:?}"
    );
    assert!(
        !overrides
            .iter()
            .any(|o| o["key"].as_str().unwrap_or_default().starts_with("domain.")),
        "domain rows belong in the dedicated `domains` list, not the flat overrides: {overrides:?}"
    );
    assert!(
        !overrides.iter().any(|o| o["key"] == "github.token"),
        "the token belongs in the dedicated `github_token` flag, not the flat overrides: {overrides:?}"
    );

    let domains = env["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(
        domains[0]["var"],
        serde_json::json!("CRYSTALLINE_DOMAIN_TEAM")
    );
    assert_eq!(domains[0]["name"], serde_json::json!("team"));
    assert_eq!(
        domains[0]["path"],
        serde_json::json!(domain_dir.display().to_string())
    );
    assert_eq!(domains[0]["origin"], serde_json::Value::Null);

    assert_eq!(env["github_token"], serde_json::json!(true));
}

#[test]
fn environment_section_renders_for_a_human_without_leaking_the_token() {
    let work = tempfile::tempdir().unwrap();
    let (config, domain_dir) = setup_env_domain(work.path());

    let human = bin()
        .env("CRYSTALLINE_SERVICE_READ_ONLY", "true")
        .env("CRYSTALLINE_DOMAIN_TEAM", &domain_dir)
        .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .output()
        .unwrap()
        .stdout;
    let human = String::from_utf8(human).unwrap();

    assert!(human.contains("environment:"), "{human}");
    assert!(
        human.contains("CRYSTALLINE_SERVICE_READ_ONLY overrides service.read_only = true"),
        "{human}"
    );
    assert!(
        human.contains(&format!(
            "CRYSTALLINE_DOMAIN_TEAM defines domain 'team' at {}",
            domain_dir.display()
        )),
        "{human}"
    );
    assert!(
        human.contains("CRYSTALLINE_GITHUB_TOKEN provides the GitHub token (read-only)"),
        "{human}"
    );
    assert!(!human.contains("SECRET"), "{human}");
}

#[test]
fn environment_section_is_absent_with_no_env_vars_active() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    std::fs::write(&config, "domains: {}\n").unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .output()
        .unwrap()
        .stdout;
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["environment"], serde_json::Value::Null);
}

/// The `harnesses` section reads `~/.claude` and `~/.codex`/`~/.agents`,
/// reachable only through `HOME`, never a CLI flag, so every scenario below
/// isolates a fresh `HOME` the same way the lock/socket and github tests
/// above do. A domain-less config plus a fresh `--db` path keeps every
/// assertion focused on the harnesses section alone.
#[cfg(unix)]
fn empty_config(work: &Path) -> (PathBuf, PathBuf) {
    let config = work.join("config.yaml");
    std::fs::write(&config, "domains: {}\n").unwrap();
    (config, work.join("index.db"))
}

/// Find one harness's entry in a `--json doctor` report's `harnesses` array
/// by its `name` (`"claude-code"` or `"codex"`).
#[cfg(unix)]
fn harness_entry<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["harnesses"]
        .as_array()
        .unwrap_or_else(|| panic!("harnesses section is absent: {report}"))
        .iter()
        .find(|h| h["name"] == name)
        .unwrap_or_else(|| panic!("no {name} entry in harnesses: {report}"))
}

#[test]
#[cfg(unix)]
fn harnesses_section_reports_both_hooks_present_after_install() {
    let (home, _state_dir) = isolated_home("harness-installed");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    // Seed a real Claude Code settings file the same way a user would: run
    // the installer itself, skipping MCP (no shim on PATH) and skills
    // (irrelevant to this scenario) so only the two hooks land.
    let mut install_cmd = bin();
    apply_home(&mut install_cmd, &home);
    install_cmd
        .args(["install", "claude-code", "--skip-mcp", "--skip-skills"])
        .assert()
        .success();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
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
    let claude = harness_entry(&report, "claude-code");
    assert_eq!(claude["settings_present"], serde_json::json!(true));
    assert_eq!(claude["settings_parse_error"], serde_json::Value::Null);
    assert_eq!(claude["session_start_hook"], serde_json::json!(true));
    assert_eq!(claude["stop_hook"], serde_json::json!(true));

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_counts_a_corrupt_settings_file_as_a_problem() {
    let (home, _state_dir) = isolated_home("harness-corrupt");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "{ not valid json").unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
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
    let claude = harness_entry(&report, "claude-code");
    assert!(
        claude["settings_parse_error"].is_string(),
        "a corrupt settings file must report a parse error: {report}"
    );

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("not valid JSON"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_hints_partial_setup_when_only_one_hook_is_present() {
    let (home, _state_dir) = isolated_home("harness-partial");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    // Only the hand-written SessionStart recipe from the README, no Stop
    // hook: exactly the "half installed" shape the hint exists for.
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{ "hooks": { "SessionStart": [ { "matcher": "startup", "hooks": [ { "type": "command", "command": "crystalline prompt system" } ] } ] } }"#,
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let human = cmd
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap()
        .stdout;
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("partial setup - run: crystalline install claude-code"),
        "{human}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_is_absent_with_no_trace_on_either_harness() {
    let (home, _state_dir) = isolated_home("harness-none");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
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
    assert_eq!(report["harnesses"], serde_json::Value::Null);

    let _ = std::fs::remove_dir_all(&home);
}

// --- receipt-aware harness diagnostics ---------------------------------------
//
// The scenario below needs a real install receipt to tamper with, which only
// `crystalline install` writes. `receipt_file`, `tamper_receipt` and
// `write_shim` mirror the same-named helpers in `tests/install.rs` - test
// binaries do not share code across files, so they are duplicated here rather
// than factored out.

/// Create an executable shim named `name` in `bin_dir` that appends its
/// arguments to `log` and exits 1 for `mcp get`, 0 otherwise, so the install
/// this scenario seeds proceeds exactly like a real (not-yet-registered)
/// harness CLI would.
#[cfg(unix)]
fn write_shim(bin_dir: &Path, name: &str, log: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = mcp ] && [ \"$2\" = get ]; then\n  exit 1\nfi\nexit 0\n",
        log.display()
    );
    let path = bin_dir.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The receipt path under an isolated home: state_dir honors
/// `XDG_STATE_HOME`, which `apply_home` points at `<home>/state`.
#[cfg(unix)]
fn receipt_file(home: &Path) -> PathBuf {
    home.join("state").join("crystalline").join("installs.json")
}

/// Rewrite the receipt with a mutation applied, for simulating an install
/// last reconciled by a different binary version, or one that still
/// remembers a skill this version no longer ships.
#[cfg(unix)]
fn tamper_receipt(home: &Path, mutate: impl FnOnce(&mut Value)) {
    let path = receipt_file(home);
    let bytes = std::fs::read(&path).unwrap();
    let mut receipt: Value = serde_json::from_slice(&bytes).unwrap();
    mutate(&mut receipt);
    std::fs::write(&path, serde_json::to_string_pretty(&receipt).unwrap()).unwrap();
}

/// A version-skewed install whose receipt still remembers a retired skill:
/// `doctor` must surface both the skew and the leftover, in `--json` and in
/// the human rendering, without failing the exit code - both self-heal, the
/// skew at the next session start and the leftover at the next `crystalline
/// install`.
#[test]
#[cfg(unix)]
fn doctor_reports_a_version_skewed_install_and_retired_leftovers() {
    let (home, _state_dir) = isolated_home("skew-leftover");
    let work = tempfile::tempdir().unwrap();
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    let (config, db) = empty_config(work.path());

    let mut install_cmd = bin();
    apply_home(&mut install_cmd, &home);
    install_cmd
        .env("PATH", &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    // Tamper the receipt: an older version reconciled this install, and it
    // still remembers a skill folder this binary no longer ships.
    tamper_receipt(&home, |receipt| {
        let entry = &mut receipt["installs"][0];
        entry["version"] = serde_json::json!("0.0.1");
        let skills = entry["skills"].as_array_mut().unwrap();
        skills.push(serde_json::json!({
            "name": "crystalline-legacy",
            "sha256": "0".repeat(64),
        }));
    });
    let legacy_dir = home
        .join(".claude")
        .join("skills")
        .join("crystalline-legacy");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("SKILL.md"), "legacy body").unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
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
    let claude = harness_entry(&report, "claude-code");
    assert_eq!(claude["receipt_version"], serde_json::json!("0.0.1"));
    assert_eq!(
        claude["retired_leftovers"],
        serde_json::json!(["crystalline-legacy"])
    );

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("installed by 0.0.1"), "{human}");
    assert!(human.contains("crystalline-legacy"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}
