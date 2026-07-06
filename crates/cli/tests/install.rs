//! Integration tests for `crystalline install` / `crystalline uninstall`,
//! spawning the real `crystalline` binary. Every scenario needs control over
//! the harness paths (`~/.claude`, `~/.codex`, `~/.agents`), reachable only
//! through `HOME` and the XDG base directories, so the same isolation
//! technique `crates/cli/tests/hook.rs` uses applies here: the environment is
//! set per child with `assert_cmd`'s `.env`, never a process-global
//! `std::env::set_var`.
//!
//! The harness CLIs (`claude`, `codex`) are never the real tools. Each test
//! that exercises MCP registration drops a tiny shell shim into a bin folder,
//! makes it executable, points the child's `PATH` at that folder alone and
//! reads back the log of arguments the shim was called with. A shim exits 1
//! for `mcp get` (so the install always proceeds to `mcp add`) and 0 for
//! everything else. The missing-CLI test points `PATH` at an empty folder so
//! no shim is found at all.
//!
//! Unix-only: `etcetera`'s base-directory resolution on Windows does not
//! honor these variables the way the XDG strategy the isolation relies on
//! does, and the shims are shell scripts.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Redirect `HOME` and the XDG base directories into `home` and point `PATH`
/// at `bin_dir` alone, so both the harness config the command writes and the
/// harness CLI it shells out to are fully under the test's control.
fn install_cmd(home: &Path, bin_dir: &Path) -> Command {
    let mut cmd = bin();
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("PATH", bin_dir);
    cmd
}

/// Create an executable shim named `name` in `bin_dir` that appends its
/// arguments to `log` and exits 1 for `mcp get`, 0 otherwise. Mirrors what a
/// real `claude`/`codex` would answer for a not-yet-registered server.
fn write_shim(bin_dir: &Path, name: &str, log: &Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = mcp ] && [ \"$2\" = get ]; then\n  exit 1\nfi\nexit 0\n",
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

/// The Claude Code user-scope settings file under an isolated `home`.
fn claude_settings(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

/// A managed skill's SKILL.md under a Claude Code skills folder.
fn claude_skill(home: &Path, name: &str) -> PathBuf {
    home.join(".claude")
        .join("skills")
        .join(name)
        .join("SKILL.md")
}

/// Parse a JSON file into a value.
fn read_json(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn install_into_an_empty_home_writes_the_exact_managed_shape() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    let out = install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .output()
        .unwrap();
    assert!(out.status.success(), "install must succeed");

    let settings = read_json(&claude_settings(&home));
    let expected = json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "startup|clear|compact",
                    "hooks": [ { "type": "command", "command": "crystalline prompt system", "timeout": 10 } ]
                }
            ],
            "Stop": [
                {
                    "hooks": [ { "type": "command", "command": "crystalline hook stop", "timeout": 10 } ]
                }
            ]
        }
    });
    assert_eq!(settings, expected, "settings.json managed shape");

    // The MCP shim was asked to register the server, at user scope.
    let logged = read_log(&log);
    assert!(
        logged.contains("mcp get crystalline"),
        "get first: {logged}"
    );
    assert!(
        logged.contains("mcp add crystalline --scope user crystalline mcp"),
        "add at user scope: {logged}"
    );

    // All four skills land, and only those four.
    for name in [
        "crystalline-routing",
        "crystalline-capture",
        "crystalline-schema",
        "crystalline-collaboration",
    ] {
        assert!(claude_skill(&home, name).exists(), "skill {name} installed");
    }
    assert!(
        !claude_skill(&home, "crystalline-memory").exists(),
        "crystalline-memory is Desktop-only and never installed"
    );
}

#[test]
fn a_second_install_is_a_byte_identical_no_op() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();
    let after_first = std::fs::read(claude_settings(&home)).unwrap();

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();
    let after_second = std::fs::read(claude_settings(&home)).unwrap();

    assert_eq!(
        after_first, after_second,
        "a re-run must not rewrite the settings file"
    );
}

#[test]
fn foreign_hooks_survive_install_and_uninstall() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    // Seed a settings file carrying a foreign hook and a foreign top-level key.
    let settings_path = claude_settings(&home);
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    let seeded = json!({
        "model": "opus",
        "hooks": {
            "PreToolUse": [
                { "matcher": "Bash", "hooks": [ { "type": "command", "command": "echo hi" } ] }
            ]
        }
    });
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&seeded).unwrap(),
    )
    .unwrap();

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    let after_install = read_json(&settings_path);
    // Foreign data intact.
    assert_eq!(after_install["model"], "opus");
    assert_eq!(
        after_install["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "echo hi"
    );
    // Ours added.
    assert_eq!(
        after_install["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "crystalline prompt system"
    );
    assert_eq!(
        after_install["hooks"]["Stop"][0]["hooks"][0]["command"],
        "crystalline hook stop"
    );

    install_cmd(&home, &bin_dir)
        .args(["uninstall", "claude-code"])
        .assert()
        .success();

    let after_uninstall = read_json(&settings_path);
    // Foreign data still intact.
    assert_eq!(after_uninstall["model"], "opus");
    assert_eq!(
        after_uninstall["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "echo hi"
    );
    // Ours gone, foreign event kept.
    assert!(
        after_uninstall["hooks"]
            .as_object()
            .unwrap()
            .get("SessionStart")
            .is_none()
    );
    assert!(
        after_uninstall["hooks"]
            .as_object()
            .unwrap()
            .get("Stop")
            .is_none()
    );
    assert!(
        after_uninstall["hooks"]
            .as_object()
            .unwrap()
            .contains_key("PreToolUse")
    );
}

#[test]
fn an_unparseable_settings_file_is_a_hard_error_and_is_left_untouched() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    let settings_path = claude_settings(&home);
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    std::fs::write(&settings_path, "{ this is not json").unwrap();

    let out = install_cmd(&home, &bin_dir)
        .args(["install", "claude-code", "--skip-mcp", "--skip-skills"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "an unparseable file aborts");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("settings.json"),
        "the error names the path: {stderr}"
    );
    // Never overwritten.
    assert_eq!(
        std::fs::read_to_string(&settings_path).unwrap(),
        "{ this is not json"
    );
}

#[test]
fn a_locally_modified_skill_is_kept_then_force_removes_it() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    // A person edits one skill by hand.
    let edited = claude_skill(&home, "crystalline-routing");
    std::fs::write(&edited, "locally changed").unwrap();

    // Plain uninstall keeps the edited skill, removes the untouched ones.
    install_cmd(&home, &bin_dir)
        .args(["uninstall", "claude-code"])
        .assert()
        .success();
    assert!(edited.exists(), "a locally modified skill is kept");
    assert!(
        !claude_skill(&home, "crystalline-capture").exists(),
        "an untouched skill is removed"
    );

    // --force removes the edited one too.
    install_cmd(&home, &bin_dir)
        .args(["uninstall", "claude-code", "--force"])
        .assert()
        .success();
    assert!(!edited.exists(), "--force removes even a modified skill");
}

#[test]
fn skip_flags_leave_each_part_untouched() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args([
            "install",
            "claude-code",
            "--skip-mcp",
            "--skip-hooks",
            "--skip-skills",
        ])
        .assert()
        .success();

    assert!(
        !claude_settings(&home).exists(),
        "--skip-hooks writes no settings file"
    );
    assert!(
        !claude_skill(&home, "crystalline-routing").exists(),
        "--skip-skills copies no skill"
    );
    assert!(
        read_log(&log).is_empty(),
        "--skip-mcp shells out to nothing: {}",
        read_log(&log)
    );
}

#[test]
fn codex_writes_hooks_json_and_agents_skills_with_a_trust_notice() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("codex.log");
    write_shim(&bin_dir, "codex", &log);

    let out = install_cmd(&home, &bin_dir)
        .args(["install", "codex"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("/hooks"),
        "codex output carries the trust notice: {stdout}"
    );

    // Hooks land in ~/.codex/hooks.json.
    let hooks_json = home.join(".codex").join("hooks.json");
    let settings = read_json(&hooks_json);
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "crystalline prompt system"
    );
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        "crystalline hook stop"
    );

    // Skills land under ~/.agents/skills.
    assert!(
        home.join(".agents")
            .join("skills")
            .join("crystalline-routing")
            .join("SKILL.md")
            .exists(),
        "codex skills go to ~/.agents/skills"
    );

    // MCP registration uses codex's `--` argument separator.
    let logged = read_log(&log);
    assert!(
        logged.contains("mcp add crystalline -- crystalline mcp"),
        "codex add form: {logged}"
    );
}

#[test]
fn project_scope_writes_relative_to_the_working_directory() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    let project = work.path().join("repo");
    std::fs::create_dir_all(&project).unwrap();

    install_cmd(&home, &bin_dir)
        .current_dir(&project)
        .args(["install", "claude-code", "--project"])
        .assert()
        .success();

    // Settings and skills land under the project, not the home directory.
    assert!(
        project.join(".claude").join("settings.json").exists(),
        "project settings under the cwd"
    );
    assert!(
        project
            .join(".claude")
            .join("skills")
            .join("crystalline-routing")
            .join("SKILL.md")
            .exists(),
        "project skills under the cwd"
    );
    assert!(
        !claude_settings(&home).exists(),
        "nothing is written into the user home under --project"
    );
    // Claude Code project MCP scope is requested explicitly.
    assert!(
        read_log(&log).contains("mcp add crystalline --scope project crystalline mcp"),
        "project scope requested: {}",
        read_log(&log)
    );
}

#[test]
fn a_missing_harness_cli_prints_a_manual_command_and_still_succeeds() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    // An empty bin folder: no `claude` shim on PATH at all.
    let empty_bin = work.path().join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();

    let out = install_cmd(&home, &empty_bin)
        .args(["install", "claude-code"])
        .output()
        .unwrap();
    assert!(out.status.success(), "a missing harness CLI is never fatal");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("claude mcp add crystalline --scope user crystalline mcp"),
        "the manual MCP command is printed: {stdout}"
    );
    // The hooks and skills still installed despite the missing CLI.
    assert!(claude_settings(&home).exists(), "hooks still written");
    assert!(
        claude_skill(&home, "crystalline-routing").exists(),
        "skills still copied"
    );
}
