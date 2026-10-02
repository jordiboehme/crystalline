//! Install and uninstall cases that cross two harnesses: shared skills
//! folders, the skills rebalance and the imported Claude Code hook.

use super::support::*;

fn skills_in(b: &Sandbox, rel: &str) -> usize {
    std::fs::read_dir(b.home.join(rel))
        .map(|d| d.count())
        .unwrap_or(0)
}

#[test]
fn claude_code_then_cursor_covers_cursor_and_writes_nothing_twice() {
    let b = sandbox();
    // Claude Code's MCP part needs its CLI; skip it, it is not under test here.
    cmd(&b)
        .args(["install", "claude-code", "--skip-mcp"])
        .assert()
        .success();
    let out = cmd(&b)
        .args(["install", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("covered by"), "{out}");
    assert!(
        out.contains("The Claude Code routing hook stays silent inside Cursor"),
        "{out}"
    );
    assert!(
        out.contains(
            "Cursor reads ~/.cursor/hooks.json when it starts. Restart Cursor to load the hook."
        ),
        "{out}"
    );
    assert_eq!(skills_in(&b, ".agents/skills"), 0);
    assert_eq!(skills_in(&b, ".claude/skills"), 4);
    let out = cmd(&b)
        .args(["--json", "uninstall", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["skills"]["covered_by"], "claude-code",
        "the report says why nothing was removed: {report}"
    );
    assert_eq!(
        skills_in(&b, ".claude/skills"),
        4,
        "the cover keeps its files"
    );
}

#[test]
fn uninstalling_claude_code_hands_cursors_skills_to_agents_skills() {
    let b = sandbox();
    cmd(&b)
        .args(["install", "claude-code", "--skip-mcp"])
        .assert()
        .success();
    cmd(&b).args(["install", "cursor"]).assert().success();
    cmd(&b)
        .args(["uninstall", "claude-code"])
        .assert()
        .success();
    assert_eq!(skills_in(&b, ".claude/skills"), 0);
    assert_eq!(
        skills_in(&b, ".agents/skills"),
        4,
        "handed over in the same run"
    );
}

/// Without the Cursor hook nothing silences the Claude Code hook inside
/// Cursor, so neither note may claim it; with no hook written there is no
/// hook to restart for either.
#[test]
fn the_cursor_notes_follow_the_hooks_that_are_really_there() {
    let b = sandbox();
    cmd(&b)
        .args(["install", "claude-code", "--skip-mcp"])
        .assert()
        .success();
    let out = cmd(&b)
        .args(["install", "cursor", "--skip-hooks"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(!out.contains("stays silent inside Cursor"), "{out}");
    assert!(!out.contains("Restart Cursor"), "{out}");
}

/// The session-start refresh of a covered Cursor row keeps its marker, so a
/// later uninstall of Claude Code still hands the skills over.
#[test]
fn a_session_start_refresh_keeps_cursors_cover_for_the_hand_over() {
    let b = sandbox();
    cmd(&b)
        .args(["install", "claude-code", "--skip-mcp"])
        .assert()
        .success();
    cmd(&b).args(["install", "cursor"]).assert().success();
    let p = crate::common::isolated_state_dir(&b.home).join("installs.json");
    let text = std::fs::read_to_string(&p)
        .unwrap()
        .replace(env!("CARGO_PKG_VERSION"), "0.21.9");
    std::fs::write(&p, text).unwrap();
    cmd(&b)
        .args([
            "prompt",
            "system",
            "--format",
            "cursor",
            "--harness",
            "cursor",
        ])
        .write_stdin("{\"source\":\"startup\"}")
        .assert()
        .success();
    let r = receipt(&b);
    let row = r["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "cursor")
        .unwrap();
    assert_eq!(row["version"], env!("CARGO_PKG_VERSION"), "refreshed");
    assert_eq!(row["skills"][0]["name"], "covered:claude-code", "{row}");
    cmd(&b)
        .args(["uninstall", "claude-code"])
        .assert()
        .success();
    assert_eq!(skills_in(&b, ".agents/skills"), 4);
}

/// Decision 3 and review focus 3.
#[test]
fn cursor_first_then_claude_code_leaves_one_copy_of_each_skill() {
    let b = sandbox();
    cmd(&b).args(["install", "cursor"]).assert().success();
    assert_eq!(skills_in(&b, ".agents/skills"), 4);
    cmd(&b)
        .args(["install", "claude-code", "--skip-mcp"])
        .assert()
        .success();
    assert_eq!(
        skills_in(&b, ".agents/skills"),
        0,
        "Cursor's copies dropped: it reads ~/.claude/skills now"
    );
    assert_eq!(skills_in(&b, ".claude/skills"), 4);
}

#[test]
fn inside_cursor_only_cursors_own_hook_prints() {
    let b = sandbox();
    cmd(&b)
        .args(["install", "claude-code", "--skip-mcp"])
        .assert()
        .success();
    cmd(&b)
        .args(["install", "cursor", "--skip-mcp"])
        .assert()
        .success();
    let payload = "{\"source\":\"startup\",\"cursor_version\":\"1.9.0\"}";
    let claude = cmd(&b)
        .args(["prompt", "system", "--harness", "claude-code"])
        .write_stdin(payload)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(claude.is_empty());
    let cursor = cmd(&b)
        .args([
            "prompt",
            "system",
            "--format",
            "cursor",
            "--harness",
            "cursor",
        ])
        .write_stdin(payload)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8_lossy(&cursor).contains("additional_context"));
}
