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
    assert!(
        String::from_utf8_lossy(&out).contains("covered by"),
        "{}",
        String::from_utf8_lossy(&out)
    );
    assert_eq!(skills_in(&b, ".agents/skills"), 0);
    assert_eq!(skills_in(&b, ".claude/skills"), 4);
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
