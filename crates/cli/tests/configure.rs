//! Smoke tests for `crystalline config show|set|unset` against a temp config,
//! no daemon involved (the in-process path). The daemon/ctl `configure` round
//! trip is covered at the engine level by `crates/service/tests/configure.rs`;
//! there is no existing daemon-lifecycle test for it here (see
//! `crates/cli/tests/service.rs` for that harness), noted as a gap.

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Redirect `HOME` and the XDG base directories into `home` so a child that
/// reads the environment never reaches a real daemon socket or the developer's
/// own config, and the default config path stays inside the tempdir. The
/// environment-driven tests below combine this with a per-child `.env` for the
/// `CRYSTALLINE_*` variable under test, never a process-global `set_var`.
fn isolate(cmd: &mut Command, home: &std::path::Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"));
}

#[test]
fn config_show_set_unset_round_trip_against_a_temp_config() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");

    // No config file yet: every setting shows its default.
    let out = bin()
        .args(["--json", "config", "show", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(out.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let settings = shown["settings"].as_array().unwrap();
    assert_eq!(settings.len(), 10);
    assert!(settings.iter().all(|s| s["source"] == "default"));

    // Set writes the config file and returns the new effective value.
    let out = bin()
        .args([
            "--json",
            "config",
            "set",
            "github.enabled",
            "true",
            "--config",
        ])
        .arg(&config)
        .output()
        .unwrap();
    assert!(out.status.success());
    let set: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(set["key"], "github.enabled");
    assert_eq!(set["value"], "true");
    assert_eq!(set["source"], "config");
    assert!(config.exists(), "config written");

    // The human-readable render is aligned and marks the default entries.
    let human = bin()
        .args(["config", "show", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("github.enabled"));
    assert!(human.contains("true"));
    assert!(human.contains("(default)"), "unset keys marked: {human}");

    // An invalid value is refused with an actionable message and no write.
    bin()
        .args(["config", "set", "github.poll_secs", "59", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("60"));

    // Unset returns to the default and drops the now-empty github block.
    let out = bin()
        .args(["--json", "config", "unset", "github.enabled", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(out.status.success());
    let unset: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(unset["value"], "false");
    assert_eq!(unset["source"], "default");
    let raw = std::fs::read_to_string(&config).unwrap();
    assert!(!raw.contains("github"), "emptied block dropped: {raw}");
}

#[test]
fn config_set_unknown_key_lists_known_keys() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    bin()
        .args(["config", "set", "github.bogus", "x", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));
}

#[test]
fn config_set_a_startup_effective_key_prints_the_restart_note() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");

    let out = bin()
        .args(["config", "set", "database.backend", "postgres", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(out.status.success());
    let human = String::from_utf8(out.stdout).unwrap();
    assert!(human.contains("database.backend = postgres"), "{human}");
    assert!(
        human.contains("applies the next time the daemon starts"),
        "{human}"
    );
}

#[test]
fn config_show_marks_an_env_overridden_setting() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");

    let mut cmd = bin();
    isolate(&mut cmd, work.path());
    let out = cmd
        .env("CRYSTALLINE_GITHUB_ENABLED", "true")
        .args(["--json", "config", "show", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let settings = shown["settings"].as_array().unwrap();
    let enabled = settings
        .iter()
        .find(|s| s["key"] == "github.enabled")
        .unwrap();
    assert_eq!(enabled["value"], "true");
    assert_eq!(enabled["source"], "env");

    // The human render marks the env-overridden entry.
    let mut cmd = bin();
    isolate(&mut cmd, work.path());
    let human = cmd
        .env("CRYSTALLINE_GITHUB_ENABLED", "true")
        .args(["config", "show", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("github.enabled"));
    assert!(human.contains("(env)"), "env override marked: {human}");
}

#[test]
fn an_invalid_env_value_exits_nonzero_naming_the_variable() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");

    let mut cmd = bin();
    isolate(&mut cmd, work.path());
    cmd.env("CRYSTALLINE_GITHUB_POLL_SECS", "10")
        .args(["config", "show", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("CRYSTALLINE_GITHUB_POLL_SECS"));
}

#[test]
fn crystalline_config_selects_the_file_and_the_config_flag_beats_it() {
    let work = tempfile::tempdir().unwrap();
    let env_config = work.path().join("env-config.yaml");
    let flag_config = work.path().join("flag-config.yaml");

    // With no --config flag, CRYSTALLINE_CONFIG selects the file that a set
    // writes to.
    let mut cmd = bin();
    isolate(&mut cmd, work.path());
    cmd.env("CRYSTALLINE_CONFIG", &env_config)
        .args(["config", "set", "github.poll_secs", "120"])
        .assert()
        .success();
    assert!(env_config.exists(), "CRYSTALLINE_CONFIG file written");

    // Show through the same CRYSTALLINE_CONFIG reads it back.
    let mut cmd = bin();
    isolate(&mut cmd, work.path());
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &env_config)
        .args(["--json", "config", "show"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let poll = shown["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "github.poll_secs")
        .unwrap()
        .clone();
    assert_eq!(poll["value"], "120");
    assert_eq!(poll["source"], "config");

    // A --config flag wins over CRYSTALLINE_CONFIG: the flag file is empty, so
    // the value set in the env-selected file is not seen.
    let mut cmd = bin();
    isolate(&mut cmd, work.path());
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &env_config)
        .args(["--json", "config", "show", "--config"])
        .arg(&flag_config)
        .output()
        .unwrap();
    assert!(out.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let poll = shown["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "github.poll_secs")
        .unwrap()
        .clone();
    assert_eq!(
        poll["value"], "300",
        "the flag file is empty, so the default shows"
    );
    assert_eq!(poll["source"], "default");
}
