//! Integration tests for `crystalline provision` / `provision status` /
//! `provision allow` / `provision deny`, spawning the real `crystalline`
//! binary.
//!
//! Mirrors `crates/cli/tests/install.rs`'s isolation technique: `HOME` and
//! the XDG base directories are redirected per child with `assert_cmd`'s
//! `.env`, and a tiny shell shim stands in for the `claude` CLI, logging the
//! arguments it was called with. Provisioning targets a harness's config
//! directory directly (never the index or a daemon), so every test here
//! seeds a fake `installs.json` marking claude-code onboarded rather than
//! running a real `crystalline install`.
//!
//! Unix-only: the isolation and the shim are the same as `install.rs`'s.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Redirect `HOME` and the XDG base directories into `home` and point `PATH`
/// at `bin_dir` alone, the same isolation `install.rs`'s `install_cmd` uses.
fn provision_cmd(home: &Path, bin_dir: &Path) -> Command {
    let mut cmd = bin();
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("COPILOT_HOME")
        .env("PATH", bin_dir);
    cmd
}

/// Create an executable `claude` shim in `bin_dir` that appends its
/// arguments to `log` and always exits 0 - provisioning's `SystemMcpRunner`
/// never probes with `mcp get` first, unlike `install`.
fn write_shim(bin_dir: &Path, name: &str, log: &Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
        log.display()
    );
    let path = bin_dir.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Read the shim's argument log, or an empty string when it was never called.
fn read_log(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

/// Write a claude-code install receipt at the isolated home's state
/// directory, marking it onboarded at user scope, so `provision` treats it
/// as an installed harness without a real `crystalline install` run.
fn write_install_receipt(home: &Path) {
    let path = home.join("state").join("crystalline").join("installs.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "format": 1,
            "installs": [
                {
                    "harness": "claude-code",
                    "scope": "user",
                    "version": "0.0.0",
                    "parts": { "mcp": true, "hooks": true, "skills": true },
                    "skills": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Write a harbor-shaped MANIFEST declaring a `## Provisioning` section from
/// `bullets` (already `- `-prefixed lines, one per artifact type) - the same
/// shape `crates/core/tests/provision.rs` uses, with the temporal metadata a
/// full `domain add` (not just a bare parse) expects.
fn write_manifest(dir: &Path, bullets: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let source = format!(
        "---\ntype: manifest\ntitle: harbor\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n\
         # harbor\n\n\
         ## Scope\n\n- Coastal navigation knowledge\n\n\
         ## When to Use\n\n- When docking\n\n\
         ## Provisioning\n\n{bullets}\n"
    );
    std::fs::write(dir.join("MANIFEST.md"), source).unwrap();
}

/// Build the harbor domain fixture from `crates/core/tests/provision.rs`'s
/// M4 brief: a `tide-tables` skill (`SKILL.md` plus `scripts/chart.sh`), a
/// `charts/plot-route.md` command, a `quartermaster.md` agent and a
/// `lighthouse.json` mcp.
fn write_harbor(dir: &Path) {
    write_manifest(
        dir,
        "- skills: skills\n- commands: commands\n- agents: agents\n- mcps: mcps\n",
    );
    write(
        dir,
        "skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nReads the harbor's tide tables.\n",
    );
    write(
        dir,
        "skills/tide-tables/scripts/chart.sh",
        "#!/bin/sh\necho charting\n",
    );
    write(
        dir,
        "commands/charts/plot-route.md",
        "Plot a route between two buoys.\n",
    );
    write(
        dir,
        "agents/quartermaster.md",
        "# Quartermaster\n\nKeeps the manifest of stores.\n",
    );
    write(
        dir,
        "mcps/lighthouse.json",
        r#"{"name": "lighthouse", "server": {"type": "http", "url": "https://example.test/mcp"}}"#,
    );
}

/// Register the harbor domain (already written at `harbor_dir`) without
/// indexing it: provisioning only ever reads a domain's `MANIFEST.md` and
/// declared folders straight off disk, never the index.
fn register_harbor(home: &Path, bin_dir: &Path, harbor_dir: &Path) {
    provision_cmd(home, bin_dir)
        .args(["domain", "add", "harbor"])
        .arg(harbor_dir)
        .arg("--no-sync")
        .assert()
        .success();
}

/// Register and immediately opt harbor in, the common setup for every test
/// past the bare-allow one.
fn register_and_allow(home: &Path, bin_dir: &Path, harbor_dir: &Path) {
    register_harbor(home, bin_dir, harbor_dir);
    provision_cmd(home, bin_dir)
        .args(["provision", "allow", "harbor"])
        .assert()
        .success();
}

/// A fresh work/home/bin-dir/harbor-dir tuple with the install receipt and
/// harbor fixture already in place, but the domain not yet registered.
fn setup(tag: &str) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join(format!("home-{tag}"));
    let bin_dir = work.path().join("bin");
    let harbor_dir = work.path().join("kb-harbor");
    write_install_receipt(&home);
    write_harbor(&harbor_dir);
    (work, home, bin_dir, harbor_dir)
}

#[test]
fn allow_installs_all_four_artifact_types_and_records_receipt() {
    let (work, home, bin_dir, harbor_dir) = setup("allow");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    register_harbor(&home, &bin_dir, &harbor_dir);

    let out = provision_cmd(&home, &bin_dir)
        .args(["provision", "allow", "harbor"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "provision allow must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // All four artifact types land under the temp HOME's .claude.
    assert!(
        home.join(".claude/skills/tide-tables/SKILL.md").exists(),
        "skill file installed"
    );
    assert!(
        home.join(".claude/skills/tide-tables/scripts/chart.sh")
            .exists(),
        "nested skill file installed"
    );
    assert!(
        home.join(".claude/commands/charts/plot-route.md").exists(),
        "command installed"
    );
    assert!(
        home.join(".claude/agents/quartermaster.md").exists(),
        "agent installed"
    );

    // provisions.json carries the harness state and source stamps.
    let receipt_path = home.join("state/crystalline/provisions.json");
    let receipt: Value = serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["format"], 1);
    let claude_files = &receipt["harnesses"]["claude-code"]["files"];
    assert!(
        claude_files["skills/tide-tables/SKILL.md"].is_object(),
        "{receipt}"
    );
    assert!(
        claude_files["commands/charts/plot-route.md"].is_object(),
        "{receipt}"
    );
    assert!(
        claude_files["agents/quartermaster.md"].is_object(),
        "{receipt}"
    );
    assert!(
        receipt["harnesses"]["claude-code"]["mcps"]["lighthouse"].is_object(),
        "{receipt}"
    );
    assert!(
        receipt["sources"]["harbor"]["files"]
            .as_object()
            .is_some_and(|m| !m.is_empty()),
        "source stamps recorded: {receipt}"
    );

    // The shim saw the mcp registration at user scope.
    let logged = read_log(&log);
    assert!(
        logged.contains("mcp add-json lighthouse"),
        "mcp add-json: {logged}"
    );
    assert!(logged.contains("--scope user"), "user scope: {logged}");
}

#[test]
fn edit_then_source_update_writes_bak() {
    let (work, home, bin_dir, harbor_dir) = setup("edit-source");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    register_and_allow(&home, &bin_dir, &harbor_dir);

    let agent = home.join(".claude/agents/quartermaster.md");
    std::fs::write(&agent, "my local edit").unwrap();

    // The source changes at the domain.
    write(
        &harbor_dir,
        "agents/quartermaster.md",
        "# Quartermaster\n\nUpdated stores manifest.\n",
    );

    provision_cmd(&home, &bin_dir)
        .args(["provision"])
        .assert()
        .success();

    assert_eq!(
        std::fs::read_to_string(&agent).unwrap(),
        "# Quartermaster\n\nUpdated stores manifest.\n",
        "the file is updated to the new source"
    );
    let bak = agent.with_file_name("quartermaster.md.bak");
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "my local edit",
        "the edit survives as a .bak"
    );
}

#[test]
fn rename_at_source_removes_old_target() {
    let (work, home, bin_dir, harbor_dir) = setup("rename-source");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    register_and_allow(&home, &bin_dir, &harbor_dir);

    let old_target = home.join(".claude/commands/charts/plot-route.md");
    assert!(old_target.exists(), "old target present before the rename");

    // Rename the command at its source.
    std::fs::remove_file(harbor_dir.join("commands/charts/plot-route.md")).unwrap();
    write(
        &harbor_dir,
        "commands/charts/plot-course.md",
        "Plot a course between two buoys.\n",
    );

    provision_cmd(&home, &bin_dir)
        .args(["provision"])
        .assert()
        .success();

    assert!(!old_target.exists(), "the old target is gone");
    assert!(
        home.join(".claude/commands/charts/plot-course.md").exists(),
        "the new target is present"
    );
}

#[test]
fn deny_cleans_up_and_keeps_edited_as_bak() {
    let (work, home, bin_dir, harbor_dir) = setup("deny");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    register_and_allow(&home, &bin_dir, &harbor_dir);

    // A person edits one installed command by hand before the domain is
    // denied.
    let command = home.join(".claude/commands/charts/plot-route.md");
    std::fs::write(&command, "my own notes").unwrap();

    let out = provision_cmd(&home, &bin_dir)
        .args(["provision", "deny", "harbor"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "provision deny must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Clean files are removed outright.
    assert!(
        !home.join(".claude/skills/tide-tables/SKILL.md").exists(),
        "clean skill removed"
    );
    assert!(
        !home.join(".claude/agents/quartermaster.md").exists(),
        "clean agent removed"
    );
    // The edited command is retired to a .bak, never destroyed.
    assert!(!command.exists(), "the edited command's plain path is gone");
    let bak = command.with_file_name("plot-route.md.bak");
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), "my own notes");

    // The mcp shim saw a remove for the server it added on allow.
    let logged = read_log(&log);
    assert!(
        logged.contains("mcp remove lighthouse"),
        "mcp remove: {logged}"
    );
}

#[test]
fn foreign_file_is_never_overwritten() {
    let (work, home, bin_dir, harbor_dir) = setup("foreign");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    register_harbor(&home, &bin_dir, &harbor_dir);

    // A file already sits at the target path before harbor is ever allowed,
    // differing from what harbor would install.
    let target = home.join(".claude/agents/quartermaster.md");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "not crystalline's file").unwrap();

    let out = provision_cmd(&home, &bin_dir)
        .args(["provision", "allow", "harbor"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Survives byte for byte.
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "not crystalline's file",
        "a foreign file is never overwritten"
    );

    // The receipt never claims ownership of it.
    let receipt_path = home.join("state/crystalline/provisions.json");
    let receipt: Value = serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    assert!(
        receipt["harnesses"]["claude-code"]["files"]["agents/quartermaster.md"].is_null(),
        "the foreign file is not owned: {receipt}"
    );

    let _ = log; // the shim is still exercised for the mcp artifact
}

#[test]
fn status_reports_pending_and_counts() {
    let (work, home, bin_dir, harbor_dir) = setup("status");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    register_harbor(&home, &bin_dir, &harbor_dir);

    // Before a decision, harbor shows up as pending with its counts.
    let out = provision_cmd(&home, &bin_dir)
        .args(["--json", "provision", "status"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: Value = serde_json::from_slice(&out.stdout).unwrap();
    let pending = data["pending"].as_array().unwrap();
    let harbor_pending = pending
        .iter()
        .find(|p| p["domain"] == "harbor")
        .unwrap_or_else(|| panic!("harbor pending: {data}"));
    assert_eq!(harbor_pending["counts"]["skills"], 2, "{data}");
    assert_eq!(harbor_pending["counts"]["commands"], 1, "{data}");
    assert_eq!(harbor_pending["counts"]["agents"], 1, "{data}");
    assert_eq!(harbor_pending["counts"]["mcps"], 1, "{data}");

    // Human rendering names the domain and hints at the allow command.
    let out = provision_cmd(&home, &bin_dir)
        .args(["provision", "status"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("provision allow harbor"),
        "the hint is printed: {stdout}"
    );

    // After allow, harbor is no longer pending and the harness counts show
    // what is installed.
    provision_cmd(&home, &bin_dir)
        .args(["provision", "allow", "harbor"])
        .assert()
        .success();

    let out = provision_cmd(&home, &bin_dir)
        .args(["--json", "provision", "status"])
        .output()
        .unwrap();
    let data: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        data["pending"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["domain"] != "harbor"),
        "harbor is no longer pending: {data}"
    );
    let claude_status = data["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["harness"] == "claude-code")
        .unwrap_or_else(|| panic!("claude-code status: {data}"));
    assert_eq!(claude_status["installed_files"], 4, "{data}");
    assert_eq!(claude_status["installed_mcps"], 1, "{data}");
    assert_eq!(claude_status["edited"], 0, "{data}");
    assert_eq!(claude_status["missing"], 0, "{data}");
}

#[test]
fn read_only_fallback_refuses_allow_and_apply_without_writing() {
    let (_work, home, bin_dir, harbor_dir) = setup("read-only");
    register_harbor(&home, &bin_dir, &harbor_dir);

    provision_cmd(&home, &bin_dir)
        .args(["config", "set", "service.read_only", "true"])
        .assert()
        .success();
    let config_path = home.join("config/crystalline/config.yaml");
    let before = std::fs::read_to_string(&config_path).unwrap();

    // A decision is refused with the standard read-only error, before
    // anything touches the config file or a harness directory.
    let out = provision_cmd(&home, &bin_dir)
        .args(["provision", "allow", "harbor"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "read-only must refuse allow");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("read-only"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        before,
        "the config file is untouched"
    );
    assert!(
        !home.join(".claude").exists(),
        "nothing lands in a harness directory"
    );

    // Bare apply is refused the same way.
    let out = provision_cmd(&home, &bin_dir)
        .args(["provision"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "read-only must refuse apply");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("read-only"), "{stderr}");

    // status is still answered, the same carve-out `config show` has.
    provision_cmd(&home, &bin_dir)
        .args(["provision", "status"])
        .assert()
        .success();
}

#[test]
fn env_defined_domain_decisions_get_the_env_message() {
    let (work, home, bin_dir, harbor_dir) = setup("env-domain");
    register_harbor(&home, &bin_dir, &harbor_dir);
    let config_path = home.join("config/crystalline/config.yaml");
    let before = std::fs::read_to_string(&config_path).unwrap();

    // Shadowed: harbor is in the file config AND defined by the environment.
    // The variable is the source of truth (the overlay re-inserts a fresh
    // entry on every read, discarding any provision decision written to the
    // file), so the decision is refused naming the variable.
    let out = provision_cmd(&home, &bin_dir)
        .env("CRYSTALLINE_DOMAIN_HARBOR", &harbor_dir)
        .args(["provision", "allow", "harbor"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a shadowed domain must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("CRYSTALLINE_DOMAIN_HARBOR"), "{stderr}");
    assert!(stderr.contains("unset it to manage"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        before,
        "the config file is untouched"
    );

    // Env-only: a domain no config file registers still gets the env
    // message, never the unknown-domain error - status lists it, so "not
    // registered" would be a lie.
    let cove_dir = work.path().join("kb-cove");
    std::fs::create_dir_all(&cove_dir).unwrap();
    let out = provision_cmd(&home, &bin_dir)
        .env("CRYSTALLINE_DOMAIN_COVE", &cove_dir)
        .args(["provision", "allow", "cove"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "an env-only domain must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("CRYSTALLINE_DOMAIN_COVE"), "{stderr}");
    assert!(!stderr.contains("not registered"), "{stderr}");
}

#[test]
fn unknown_and_virtual_domain_decisions_error() {
    let (_work, home, bin_dir, harbor_dir) = setup("unknown-virtual");
    register_harbor(&home, &bin_dir, &harbor_dir);

    let out = provision_cmd(&home, &bin_dir)
        .args(["provision", "allow", "does-not-exist"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "an unregistered domain must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does-not-exist"), "{stderr}");
    assert!(stderr.contains("not registered"), "{stderr}");

    provision_cmd(&home, &bin_dir)
        .args(["domain", "add", "notes", "--virtual"])
        .assert()
        .success();

    let out = provision_cmd(&home, &bin_dir)
        .args(["provision", "deny", "notes"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a virtual domain must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("notes"), "{stderr}");
    assert!(stderr.contains("virtual"), "{stderr}");
}
