//! Smoke tests for `crystalline config show|set|unset` against a temp config,
//! no daemon involved (the in-process path). The daemon/ctl `configure` round
//! trip is covered at the engine level by `crates/service/tests/configure.rs`;
//! there is no existing daemon-lifecycle test for it here (see
//! `crates/cli/tests/service.rs` for that harness), noted as a gap.

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
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
    assert_eq!(settings.len(), 8);
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
