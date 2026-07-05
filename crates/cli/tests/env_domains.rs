//! Integration tests for env-defined domains: `CRYSTALLINE_DOMAIN_<NAME>`
//! registers a file domain from the environment, immune to `domain remove` and
//! `domain add`, marked in `domain list` and routed by `prompt system`. Every
//! test injects the variable per-child with `assert_cmd`'s `.env`, never a
//! process-global `set_var`, and points every path at a tempdir.

use std::path::Path;

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Redirect `HOME` and the XDG base directories into `home` so a child never
/// reaches a real daemon socket, the developer's own config or a real index.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"));
}

/// A minimal domain directory with a MANIFEST.md so the routing prompt has
/// something to read.
fn domain_dir(parent: &Path, name: &str, scope: &str) -> std::path::PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- {scope}\n\n## When to Use\n\n- {scope}\n"
        ),
    )
    .unwrap();
    dir
}

#[test]
fn domain_list_marks_an_env_domain() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    let team = domain_dir(work.path(), "team", "shared team knowledge");

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_DOMAIN_TEAM", &team)
        .args(["--json", "domain", "list", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);

    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let domains = listed["domains"].as_array().unwrap();
    let team_row = domains
        .iter()
        .find(|d| d["name"] == "team")
        .expect("the env domain is listed");
    assert_eq!(team_row["source"], "env");
    assert_eq!(team_row["kind"], "file");

    // The human render marks the row with `(env)`.
    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let human = cmd
        .env("CRYSTALLINE_DOMAIN_TEAM", &team)
        .args(["domain", "list", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("team"), "{human}");
    assert!(human.contains("(env)"), "{human}");
}

#[test]
fn domain_remove_refuses_an_env_domain_naming_the_variable() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    let team = domain_dir(work.path(), "team", "shared team knowledge");

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    cmd.env("CRYSTALLINE_DOMAIN_TEAM", &team)
        .args(["domain", "remove", "team", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("CRYSTALLINE_DOMAIN_TEAM"));
}

#[test]
fn domain_add_refuses_an_env_domain_name_naming_the_variable() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    let team = domain_dir(work.path(), "team", "shared team knowledge");
    // A second, unrelated folder the user tries to register under the same name.
    let other = domain_dir(work.path(), "other", "unrelated");

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    cmd.env("CRYSTALLINE_DOMAIN_TEAM", &team)
        .args(["domain", "add", "team"])
        .arg(&other)
        .args(["--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("CRYSTALLINE_DOMAIN_TEAM"));
}

#[test]
fn prompt_system_routes_an_env_domain() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    let workspace = work.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let team = domain_dir(work.path(), "turbine-team", "facts about wind turbines");

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_DOMAIN_TURBINE_TEAM", &team)
        .args(["prompt", "system", "--workspace"])
        .arg(&workspace)
        .args(["--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("turbine-team"),
        "the routing prompt names the env domain: {text}"
    );
    assert!(
        text.contains("facts about wind turbines"),
        "the routing prompt carries the env domain's scope: {text}"
    );
}
