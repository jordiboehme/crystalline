//! Integration tests for `crystalline install` / `crystalline uninstall`,
//! spawning the real `crystalline` binary. Every scenario needs control over
//! the harness paths (`~/.claude`, `~/.codex`, `~/.agents`, `~/.copilot`),
//! reachable only through `HOME` and the XDG base directories, so the same
//! isolation technique `crates/cli/tests/hook.rs` uses applies here: the
//! environment is set per child with `assert_cmd`'s `.env`, never a
//! process-global `std::env::set_var`.
//!
//! The harness CLIs (`claude`, `codex`, `copilot`) are never the real tools.
//! Each test that exercises MCP registration drops a tiny shell shim into a
//! bin folder, makes it executable, points the child's `PATH` at that folder
//! alone and reads back the log of arguments the shim was called with. A shim
//! exits 1 for `mcp get` (so the install always proceeds to `mcp add`) and 0
//! for everything else. The missing-CLI test points `PATH` at an empty folder
//! so no shim is found at all.
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
        // A developer machine's own Copilot home must never leak into a
        // test's path resolution.
        .env_remove("COPILOT_HOME")
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

/// The Copilot-owned hooks file under an isolated `home`.
fn copilot_hooks_file(home: &Path) -> PathBuf {
    home.join(".copilot").join("hooks").join("crystalline.json")
}

/// A managed skill's SKILL.md under the Copilot user skills folder.
fn copilot_skill(home: &Path, name: &str) -> PathBuf {
    home.join(".copilot")
        .join("skills")
        .join(name)
        .join("SKILL.md")
}

/// The exact managed content of the Copilot-owned hooks file: the version
/// marker, PascalCase event names (they select the snake_case payloads
/// `crystalline hook stop` already parses) and flat command entries in
/// Copilot's field spellings.
fn copilot_managed_hooks() -> Value {
    json!({
        "version": 1,
        "hooks": {
            "SessionStart": [
                { "type": "command", "command": "crystalline prompt system --format copilot", "timeoutSec": 10 }
            ],
            "Stop": [
                { "type": "command", "command": "crystalline hook stop", "timeoutSec": 10 }
            ]
        }
    })
}

#[test]
fn copilot_writes_an_owned_hooks_file_and_copilot_skills() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("copilot.log");
    write_shim(&bin_dir, "copilot", &log);

    let out = install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .output()
        .unwrap();
    assert!(out.status.success(), "install must succeed");

    // The owned file carries exactly the managed shape, nothing else.
    let hooks = read_json(&copilot_hooks_file(&home));
    assert_eq!(hooks, copilot_managed_hooks(), "crystalline.json shape");

    // All four skills land under ~/.copilot/skills.
    for name in [
        "crystalline-routing",
        "crystalline-capture",
        "crystalline-schema",
        "crystalline-collaboration",
    ] {
        assert!(
            copilot_skill(&home, name).exists(),
            "skill {name} installed"
        );
    }

    // MCP registration uses the stdio `--` form, at user level.
    let logged = read_log(&log);
    assert!(
        logged.contains("mcp get crystalline"),
        "get first: {logged}"
    );
    assert!(
        logged.contains("mcp add crystalline -- crystalline mcp"),
        "copilot add form: {logged}"
    );
}

#[test]
fn a_second_copilot_install_is_a_byte_identical_no_op() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("copilot.log");
    write_shim(&bin_dir, "copilot", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();
    let after_first = std::fs::read(copilot_hooks_file(&home)).unwrap();

    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();
    let after_second = std::fs::read(copilot_hooks_file(&home)).unwrap();

    assert_eq!(
        after_first, after_second,
        "a re-run must not rewrite the owned hooks file"
    );
}

#[test]
fn copilot_uninstall_deletes_an_untouched_owned_file_but_keeps_user_entries() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("copilot.log");
    write_shim(&bin_dir, "copilot", &log);

    // Round one: an untouched owned file disappears entirely, its emptied
    // hooks folder with it.
    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();
    install_cmd(&home, &bin_dir)
        .args(["uninstall", "copilot"])
        .assert()
        .success();
    assert!(
        !copilot_hooks_file(&home).exists(),
        "an untouched owned file is deleted"
    );
    assert!(
        !home.join(".copilot").join("hooks").exists(),
        "the emptied hooks folder is pruned"
    );

    // Round two: a user entry added into our file keeps it alive.
    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();
    let path = copilot_hooks_file(&home);
    let mut root = read_json(&path);
    root["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "command", "command": "my-own-stop" }));
    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap()).unwrap();

    install_cmd(&home, &bin_dir)
        .args(["uninstall", "copilot"])
        .assert()
        .success();
    let after = read_json(&path);
    assert_eq!(
        after,
        json!({
            "version": 1,
            "hooks": { "Stop": [ { "type": "command", "command": "my-own-stop" } ] }
        }),
        "only the user's entry survives"
    );
}

#[test]
fn copilot_project_scope_writes_under_dot_github() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("copilot.log");
    write_shim(&bin_dir, "copilot", &log);
    let project = work.path().join("repo");
    std::fs::create_dir_all(&project).unwrap();

    let out = install_cmd(&home, &bin_dir)
        .current_dir(&project)
        .args(["install", "copilot", "--project"])
        .output()
        .unwrap();
    assert!(out.status.success());

    // Hooks and skills land under the project's .github, nothing in home.
    assert_eq!(
        read_json(
            &project
                .join(".github")
                .join("hooks")
                .join("crystalline.json")
        ),
        copilot_managed_hooks()
    );
    assert!(
        project
            .join(".github")
            .join("skills")
            .join("crystalline-routing")
            .join("SKILL.md")
            .exists(),
        "project skills under .github/skills"
    );
    assert!(
        !copilot_hooks_file(&home).exists(),
        "nothing is written into the user home under --project"
    );

    // Copilot has no project MCP scope and gates repository hooks and
    // skills behind folder trust; both notices are printed.
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("added globally even with --project"),
        "the user-level MCP notice is printed: {stdout}"
    );
    assert!(
        stdout.contains("trusted folder"),
        "the trust notice is printed: {stdout}"
    );
    assert!(
        read_log(&log).contains("mcp add crystalline -- crystalline mcp"),
        "the add form carries no scope: {}",
        read_log(&log)
    );
}

#[test]
fn copilot_home_env_var_relocates_the_user_install() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("copilot.log");
    write_shim(&bin_dir, "copilot", &log);
    let copilot_home = work.path().join("relocated-copilot");

    install_cmd(&home, &bin_dir)
        .env("COPILOT_HOME", &copilot_home)
        .args(["install", "copilot"])
        .assert()
        .success();

    assert_eq!(
        read_json(&copilot_home.join("hooks").join("crystalline.json")),
        copilot_managed_hooks(),
        "hooks land under COPILOT_HOME"
    );
    assert!(
        copilot_home
            .join("skills")
            .join("crystalline-routing")
            .join("SKILL.md")
            .exists(),
        "skills land under COPILOT_HOME"
    );
    assert!(
        !home.join(".copilot").exists(),
        "nothing is written under ~/.copilot when COPILOT_HOME is set"
    );
}

/// A `gh` shim standing in for the GitHub CLI's forwarding form: crystalline
/// invokes it as `gh copilot -- <verb args>`, so the MCP verbs arrive two
/// positions later than on the plain shim ($3/$4 instead of $1/$2). Exits 1
/// for `mcp get` like [`write_shim`], 0 otherwise.
fn write_gh_shim(bin_dir: &Path, log: &Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$3\" = mcp ] && [ \"$4\" = get ]; then\n  exit 1\nfi\nexit 0\n",
        log.display()
    );
    let path = bin_dir.join("gh");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn copilot_falls_back_to_gh_when_copilot_is_missing() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("gh.log");
    // Only `gh` is on the PATH, the machine the GitHub CLI installed
    // Copilot on.
    write_gh_shim(&bin_dir, &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();

    let logged = read_log(&log);
    assert!(
        logged.contains("copilot -- mcp get crystalline"),
        "the get went through gh's forwarding form: {logged}"
    );
    assert!(
        logged.contains("copilot -- mcp add crystalline -- crystalline mcp"),
        "the add went through gh's forwarding form: {logged}"
    );
    assert!(
        copilot_hooks_file(&home).exists(),
        "hooks still installed through the fallback"
    );
    assert!(
        copilot_skill(&home, "crystalline-routing").exists(),
        "skills still installed through the fallback"
    );
}

#[test]
fn a_missing_copilot_and_gh_print_the_plain_manual_command() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let empty_bin = work.path().join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();

    let out = install_cmd(&home, &empty_bin)
        .args(["install", "copilot"])
        .output()
        .unwrap();
    assert!(out.status.success(), "a missing harness CLI is never fatal");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("copilot mcp add crystalline -- crystalline mcp"),
        "the manual command shows the plain copilot form: {stdout}"
    );
    assert!(
        !stdout.contains("gh copilot"),
        "the manual command never shows the gh spelling: {stdout}"
    );
    assert!(copilot_hooks_file(&home).exists(), "hooks still written");
}

#[test]
fn copilot_prefers_the_copilot_binary_over_gh() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let copilot_log = work.path().join("copilot.log");
    let gh_log = work.path().join("gh.log");
    write_shim(&bin_dir, "copilot", &copilot_log);
    write_gh_shim(&bin_dir, &gh_log);

    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();

    assert!(
        read_log(&copilot_log).contains("mcp add crystalline -- crystalline mcp"),
        "the standalone binary handled the registration: {}",
        read_log(&copilot_log)
    );
    assert!(
        read_log(&gh_log).is_empty(),
        "gh is never consulted while copilot exists: {}",
        read_log(&gh_log)
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

/// A `crystalline` shim answering `--version` with the given version string,
/// for the PATH version-skew notice.
fn write_version_shim(bin_dir: &Path, version: &str) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = format!("#!/bin/sh\necho 'crystalline {version}'\nexit 0\n");
    let path = bin_dir.join("crystalline");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_older_crystalline_on_the_path_earns_a_version_notice() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    write_version_shim(&bin_dir, "0.0.1");

    let out = install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("version 0.0.1, not this binary's"),
        "an older PATH binary must earn the skew notice: {stdout}"
    );
}

#[test]
fn a_matching_crystalline_on_the_path_earns_no_version_notice() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    write_version_shim(&bin_dir, env!("CARGO_PKG_VERSION"));

    let out = install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains("from your PATH"),
        "a matching PATH binary must stay quiet: {stdout}"
    );
}

/// The receipt path under an isolated home: state_dir honors
/// XDG_STATE_HOME, which install_cmd points at <home>/state.
fn receipt_file(home: &Path) -> PathBuf {
    home.join("state").join("crystalline").join("installs.json")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[test]
fn install_writes_a_receipt_and_uninstall_removes_its_entry() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    let receipt = read_json(&receipt_file(&home));
    assert_eq!(receipt["format"], 1);
    let entry = &receipt["installs"][0];
    assert_eq!(entry["harness"], "claude-code");
    assert_eq!(entry["scope"], "user");
    assert!(
        entry.get("project_path").is_none(),
        "user scope records no path"
    );
    assert_eq!(entry["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        entry["parts"],
        json!({ "mcp": true, "hooks": true, "skills": true })
    );
    let skills = entry["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 4, "all four managed skills recorded");
    for s in skills {
        let hash = s["sha256"].as_str().unwrap();
        assert_eq!(hash.len(), 64, "a full sha256 hex digest per skill");
    }

    install_cmd(&home, &bin_dir)
        .args(["uninstall", "claude-code"])
        .assert()
        .success();
    let receipt = read_json(&receipt_file(&home));
    assert!(
        receipt["installs"].as_array().unwrap().is_empty(),
        "uninstall prunes the entry"
    );
}

#[test]
fn a_skip_run_after_a_full_install_keeps_the_recorded_knowledge() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();
    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code", "--skip-skills", "--skip-mcp"])
        .assert()
        .success();

    let receipt = read_json(&receipt_file(&home));
    let entry = &receipt["installs"][0];
    // Skip means "do not touch", not "undo": parts stay true and the skill
    // records survive for the next reconcile.
    assert_eq!(
        entry["parts"],
        json!({ "mcp": true, "hooks": true, "skills": true })
    );
    assert_eq!(entry["skills"].as_array().unwrap().len(), 4);
}

#[test]
fn a_skip_run_after_a_version_bump_keeps_the_prior_version_stamped() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    // Simulate the binary having since been upgraded: the receipt still
    // says an older version last reconciled this install.
    tamper_receipt(&home, |receipt| {
        receipt["installs"][0]["version"] = json!("0.0.1");
    });

    // A run that skips skills must not stamp the current version: the
    // skills on disk are still whatever the 0.0.1 install left there, so
    // stamping the current version here would tell the session-start
    // auto-reconcile there is nothing left to refresh.
    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code", "--skip-skills", "--skip-mcp"])
        .assert()
        .success();
    let receipt = read_json(&receipt_file(&home));
    assert_eq!(
        receipt["installs"][0]["version"],
        json!("0.0.1"),
        "a run that skips a recorded part must not advance the stamped version"
    );

    // A full install with nothing skipped reconciles skills for real, so
    // the version is free to advance again.
    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();
    let receipt = read_json(&receipt_file(&home));
    assert_eq!(
        receipt["installs"][0]["version"],
        json!(env!("CARGO_PKG_VERSION")),
        "a run that skips nothing stamps the current version"
    );
}

#[test]
fn uninstall_removes_an_old_but_clean_skill_via_its_receipt_hash() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    // Simulate a skill written by an older binary: the file and its receipt
    // hash agree with each other but not with this binary's embedded copy.
    let old_body = "routing skill as an older version shipped it";
    std::fs::write(claude_skill(&home, "crystalline-routing"), old_body).unwrap();
    let receipt_path = receipt_file(&home);
    let mut receipt = read_json(&receipt_path);
    for s in receipt["installs"][0]["skills"].as_array_mut().unwrap() {
        if s["name"] == "crystalline-routing" {
            s["sha256"] = json!(sha256_hex(old_body.as_bytes()));
        }
    }
    std::fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&receipt).unwrap(),
    )
    .unwrap();

    install_cmd(&home, &bin_dir)
        .args(["uninstall", "claude-code"])
        .assert()
        .success();
    assert!(
        !claude_skill(&home, "crystalline-routing").exists(),
        "an old but untouched skill is recognized by its receipt hash and removed"
    );
}

#[test]
fn a_corrupt_receipt_never_blocks_install() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    let receipt_path = receipt_file(&home);
    std::fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
    std::fs::write(&receipt_path, "{ not a receipt").unwrap();

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();
    // Regenerated fresh: valid again and carrying this install.
    let receipt = read_json(&receipt_path);
    assert_eq!(receipt["installs"][0]["harness"], "claude-code");
}

/// Rewrite the receipt with a mutation applied, for simulating an install
/// performed by a different binary version.
fn tamper_receipt(home: &Path, mutate: impl FnOnce(&mut Value)) {
    let path = receipt_file(home);
    let mut receipt = read_json(&path);
    mutate(&mut receipt);
    std::fs::write(&path, serde_json::to_string_pretty(&receipt).unwrap()).unwrap();
}

/// The embedded routing skill, byte-identical to what install writes.
const ROUTING_SKILL: &str = include_str!("../../../skills/crystalline-routing/SKILL.md");
const CAPTURE_SKILL: &str = include_str!("../../../skills/crystalline-capture/SKILL.md");

#[test]
fn prompt_system_reconciles_an_install_from_another_version() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    // Simulate the aftermath of an upgrade: the receipt says 0.0.1 wrote
    // this install, one skill is an old clean copy (file and receipt hash
    // agree), one was edited by a person, one was deleted and one retired
    // name from a past version is still on disk.
    let old_body = "routing as an older release shipped it";
    std::fs::write(claude_skill(&home, "crystalline-routing"), old_body).unwrap();
    std::fs::write(claude_skill(&home, "crystalline-capture"), "my edits").unwrap();
    std::fs::remove_dir_all(claude_skill(&home, "crystalline-schema").parent().unwrap()).unwrap();
    let legacy_dir = home
        .join(".claude")
        .join("skills")
        .join("crystalline-legacy");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("SKILL.md"), "legacy body").unwrap();
    tamper_receipt(&home, |receipt| {
        let entry = &mut receipt["installs"][0];
        entry["version"] = json!("0.0.1");
        let skills = entry["skills"].as_array_mut().unwrap();
        for s in skills.iter_mut() {
            if s["name"] == "crystalline-routing" {
                s["sha256"] = json!(sha256_hex(old_body.as_bytes()));
            }
        }
        skills.push(json!({
            "name": "crystalline-legacy",
            "sha256": sha256_hex(b"legacy body")
        }));
    });

    let out = install_cmd(&home, &bin_dir)
        .args(["prompt", "system"])
        .output()
        .unwrap();
    assert!(out.status.success(), "the hook path must succeed");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("[crystalline]"),
        "a reconcile leaves a notice line: {stdout}"
    );

    // Old clean copy: updated in place, no backup.
    assert_eq!(
        std::fs::read_to_string(claude_skill(&home, "crystalline-routing")).unwrap(),
        ROUTING_SKILL
    );
    assert!(
        !claude_skill(&home, "crystalline-routing")
            .with_file_name("SKILL.md.bak")
            .exists(),
        "a clean old copy earns no backup"
    );
    // Edited copy: updated, edits preserved beside it.
    assert_eq!(
        std::fs::read_to_string(claude_skill(&home, "crystalline-capture")).unwrap(),
        CAPTURE_SKILL
    );
    assert_eq!(
        std::fs::read_to_string(
            claude_skill(&home, "crystalline-capture").with_file_name("SKILL.md.bak")
        )
        .unwrap(),
        "my edits"
    );
    // Deleted skill: the deletion is respected.
    assert!(
        !claude_skill(&home, "crystalline-schema").exists(),
        "auto-reconcile never resurrects a deleted skill"
    );
    // Retired leftover with a matching hash: removed, folder and all.
    assert!(!legacy_dir.exists(), "the retired leftover is removed");

    // Receipt: version current, schema and legacy dropped from the records.
    let receipt = read_json(&receipt_file(&home));
    let entry = &receipt["installs"][0];
    assert_eq!(entry["version"], env!("CARGO_PKG_VERSION"));
    let names: Vec<&str> = entry["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"crystalline-routing"));
    assert!(names.contains(&"crystalline-capture"));
    assert!(names.contains(&"crystalline-collaboration"));
    assert!(
        !names.contains(&"crystalline-schema"),
        "deleted skill left the receipt"
    );
    assert!(
        !names.contains(&"crystalline-legacy"),
        "retired skill left the receipt"
    );

    // A second run is quiet: versions match, nothing to do.
    let out = install_cmd(&home, &bin_dir)
        .args(["prompt", "system"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        !stdout.contains("[crystalline]"),
        "a matching version reconciles nothing: {stdout}"
    );
}

#[test]
fn copilot_receipt_entry_records_harness_copilot() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("copilot.log");
    write_shim(&bin_dir, "copilot", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();

    let receipt = read_json(&receipt_file(&home));
    let entry = &receipt["installs"][0];
    assert_eq!(entry["harness"], "copilot");
    assert_eq!(entry["scope"], "user");
    assert_eq!(
        entry["parts"],
        json!({ "mcp": true, "hooks": true, "skills": true })
    );
    assert_eq!(
        entry["skills"].as_array().unwrap().len(),
        4,
        "all four managed skills recorded"
    );

    install_cmd(&home, &bin_dir)
        .args(["uninstall", "copilot"])
        .assert()
        .success();
    let receipt = read_json(&receipt_file(&home));
    assert!(
        receipt["installs"].as_array().unwrap().is_empty(),
        "uninstall prunes the entry"
    );
}

/// The copilot flavor of the reconcile test doubles as the notice-flow test
/// for the copilot prompt format: an upgrade replays the hooks part (the
/// deleted owned file comes back in the managed shape) and the reconcile
/// notice rides inside the single JSON document, never beside it.
#[test]
fn prompt_system_reconciles_a_copilot_install_from_another_version() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let log = work.path().join("copilot.log");
    write_shim(&bin_dir, "copilot", &log);

    install_cmd(&home, &bin_dir)
        .args(["install", "copilot"])
        .assert()
        .success();

    // Simulate the aftermath of an upgrade: an older binary wrote this
    // install and its hooks file has since gone missing.
    std::fs::remove_file(copilot_hooks_file(&home)).unwrap();
    tamper_receipt(&home, |receipt| {
        receipt["installs"][0]["version"] = json!("0.0.1");
    });

    let out = install_cmd(&home, &bin_dir)
        .args(["prompt", "system", "--format", "copilot"])
        .write_stdin(r#"{"source":"startup"}"#)
        .output()
        .unwrap();
    assert!(out.status.success(), "the hook path must succeed");

    // The whole stdout is one JSON document; the reconcile notice sits
    // inside the envelope.
    let stdout = String::from_utf8(out.stdout).unwrap();
    let parsed: Value = serde_json::from_str(stdout.trim())
        .expect("copilot hook stdout must be a single JSON document");
    let context = parsed["additionalContext"].as_str().unwrap();
    assert!(
        context.contains("[crystalline]"),
        "the reconcile notice rides inside the envelope: {context}"
    );

    // The owned hooks file is back in the managed shape and the receipt is
    // stamped current.
    assert_eq!(
        read_json(&copilot_hooks_file(&home)),
        copilot_managed_hooks(),
        "the hooks part was replayed"
    );
    let receipt = read_json(&receipt_file(&home));
    assert_eq!(receipt["installs"][0]["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn prompt_system_survives_a_corrupt_receipt_untouched() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let bin_dir = work.path().join("bin");
    let receipt_path = receipt_file(&home);
    std::fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
    std::fs::write(&receipt_path, "{ not a receipt").unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();

    let out = install_cmd(&home, &bin_dir)
        .args(["prompt", "system"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "a corrupt receipt never breaks the hook"
    );
    assert_eq!(
        std::fs::read_to_string(&receipt_path).unwrap(),
        "{ not a receipt",
        "the hook never rewrites a file it does not understand"
    );
}

#[test]
fn prompt_system_reconciles_a_project_install_only_from_its_directory() {
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
    let skill = project
        .join(".claude")
        .join("skills")
        .join("crystalline-routing")
        .join("SKILL.md");
    std::fs::write(&skill, "stale body").unwrap();
    tamper_receipt(&home, |receipt| {
        receipt["installs"][0]["version"] = json!("0.0.1");
    });

    // From an unrelated directory the project entry does not apply.
    install_cmd(&home, &bin_dir)
        .current_dir(work.path())
        .args(["prompt", "system"])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        "stale body",
        "a project install is never touched from outside its directory"
    );

    // From the project directory it reconciles.
    install_cmd(&home, &bin_dir)
        .current_dir(&project)
        .args(["prompt", "system"])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        ROUTING_SKILL,
        "the project install reconciles from its own directory"
    );
}
