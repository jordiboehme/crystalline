//! `crystalline install kiro` and `crystalline uninstall kiro`.

use super::support::*;
use serde_json::json;

const MCP: &str = include_str!("../../../../core/tests/fixtures/harness/kiro-mcp.json");

fn abs(b: &Sandbox) -> String {
    b.bin.join("crystalline").display().to_string()
}

#[test]
fn a_fresh_install_writes_mcp_hook_steering_and_skills() {
    let b = sandbox();
    cmd(&b).args(["install", "kiro"]).assert().success();
    let abs = abs(&b);
    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(b.home.join(".kiro/settings/mcp.json")).unwrap())
            .unwrap();
    assert_eq!(
        mcp,
        json!({"mcpServers": {"crystalline": {"command": abs, "args": ["mcp", "--harness", "kiro"]}}})
    );
    let hook: serde_json::Value = serde_json::from_slice(
        &std::fs::read(b.home.join(".kiro/hooks/crystalline.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        hook["hooks"][0]["action"]["command"],
        format!("{abs} prompt system --harness kiro")
    );
    let steering = std::fs::read_to_string(b.home.join(".kiro/steering/crystalline.md")).unwrap();
    assert_eq!(
        steering,
        crystalline_core::harness::pointer::steering_file_text()
    );
    assert!(
        b.home
            .join(".kiro/skills/crystalline-routing/SKILL.md")
            .is_file()
    );
    let r = receipt(&b);
    let row = r["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "kiro")
        .unwrap();
    assert_eq!(
        row["parts"],
        json!({"mcp": true, "hooks": true, "skills": true})
    );
}

#[test]
fn the_install_says_kiro_may_not_run_the_hooks() {
    let b = sandbox();
    let out = cmd(&b)
        .args(["install", "kiro"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(
        out.contains("Kiro CLI 2.x and custom agents may not run hooks from ~/.kiro/hooks. The steering file ~/.kiro/steering/crystalline.md points the agent at the routing block either way."),
        "{out}"
    );
}

#[test]
fn a_second_install_is_byte_identical() {
    let b = sandbox();
    cmd(&b).args(["install", "kiro"]).assert().success();
    let files = [
        ".kiro/settings/mcp.json",
        ".kiro/hooks/crystalline.json",
        ".kiro/steering/crystalline.md",
    ];
    let snap = || -> Vec<Vec<u8>> {
        files
            .iter()
            .map(|f| std::fs::read(b.home.join(f)).unwrap())
            .collect()
    };
    let before = snap();
    cmd(&b).args(["install", "kiro"]).assert().success();
    assert_eq!(before, snap());
}

#[test]
fn uninstall_deletes_the_owned_files_and_gives_mcp_json_back() {
    let b = sandbox();
    write(&b.home.join(".kiro/settings/mcp.json"), MCP.as_bytes());
    cmd(&b).args(["install", "kiro"]).assert().success();
    cmd(&b).args(["install", "kiro"]).assert().success();
    cmd(&b).args(["uninstall", "kiro"]).assert().success();
    assert_eq!(
        std::fs::read_to_string(b.home.join(".kiro/settings/mcp.json")).unwrap(),
        MCP
    );
    assert!(!b.home.join(".kiro/hooks").exists());
    assert!(!b.home.join(".kiro/steering/crystalline.md").exists());
    assert!(!b.home.join(".kiro/skills/crystalline-routing").exists());
}

#[test]
fn a_hook_a_person_added_to_our_file_keeps_it_alive() {
    let b = sandbox();
    cmd(&b).args(["install", "kiro"]).assert().success();
    let p = b.home.join(".kiro/hooks/crystalline.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    v["hooks"].as_array_mut().unwrap().push(
        json!({"name": "mine", "trigger": "AgentStop", "action": {"type": "command", "command": "echo hi"}}),
    );
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    cmd(&b).args(["uninstall", "kiro"]).assert().success();
    let left: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!(left["hooks"].as_array().unwrap().len(), 1);
    assert_eq!(left["hooks"][0]["name"], "mine");
}

#[test]
fn an_edited_steering_file_without_our_marker_is_kept_unless_forced() {
    let b = sandbox();
    cmd(&b).args(["install", "kiro"]).assert().success();
    let p = b.home.join(".kiro/steering/crystalline.md");
    std::fs::write(&p, "my own notes\n").unwrap();
    cmd(&b).args(["uninstall", "kiro"]).assert().success();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "my own notes\n");
    cmd(&b).args(["install", "kiro"]).assert().success();
    std::fs::write(&p, "my own notes\n").unwrap();
    cmd(&b)
        .args(["uninstall", "kiro", "--force"])
        .assert()
        .success();
    assert!(!p.exists());
}

#[test]
fn a_malformed_mcp_file_is_refused_and_the_other_parts_still_install() {
    let b = sandbox();
    write(
        &b.home.join(".kiro/settings/mcp.json"),
        b"{ \"mcpServers\": ",
    );
    let out = cmd(&b)
        .args(["--json", "install", "kiro"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["mcp"]["status"], "failed");
    let error = report["mcp"]["error"].as_str().unwrap();
    assert!(
        error.contains(".kiro/settings/mcp.json") && error.contains("line 1, column"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(b.home.join(".kiro/settings/mcp.json")).unwrap(),
        b"{ \"mcpServers\": "
    );
    assert!(b.home.join(".kiro/hooks/crystalline.json").is_file());
    let r = receipt(&b);
    let row = r["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "kiro")
        .unwrap();
    assert_eq!(
        row["parts"],
        json!({"mcp": false, "hooks": true, "skills": true})
    );
}

#[test]
fn project_scope_is_refused_for_install_and_uninstall() {
    let b = sandbox();
    for verb in ["install", "uninstall"] {
        let out = cmd(&b)
            .args([verb, "kiro", "--project"])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        assert!(
            String::from_utf8_lossy(&out)
                .contains("project scope is not supported for Kiro yet; run without --project")
        );
    }
    assert!(!b.home.join(".kiro").exists());
}

#[test]
fn the_skip_flags_leave_their_part_alone() {
    let b = sandbox();
    cmd(&b)
        .args(["install", "kiro", "--skip-mcp", "--skip-skills"])
        .assert()
        .success();
    assert!(!b.home.join(".kiro/settings/mcp.json").exists());
    assert!(!b.home.join(".kiro/skills").exists());
    assert!(b.home.join(".kiro/hooks/crystalline.json").is_file());
    let b = sandbox();
    cmd(&b)
        .args(["install", "kiro", "--skip-hooks"])
        .assert()
        .success();
    assert!(!b.home.join(".kiro/hooks").exists());
}

/// Nothing of ours is left under `.kiro` after an uninstall of an untouched
/// install. The shared `mcp.json` is never deleted (spec decision 10), so it
/// stays, without our entry.
#[test]
fn an_untouched_install_leaves_nothing_of_ours_behind() {
    fn walk(p: &std::path::Path, left: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), left);
                } else {
                    left.push(e.path().display().to_string());
                }
            }
        }
    }
    let b = sandbox();
    cmd(&b).args(["install", "kiro"]).assert().success();
    cmd(&b).args(["uninstall", "kiro"]).assert().success();
    let mut left = Vec::new();
    walk(&b.home.join(".kiro"), &mut left);
    let mcp = b.home.join(".kiro/settings/mcp.json").display().to_string();
    assert_eq!(left, vec![mcp.clone()], "only the shared file stays");
    assert!(
        !std::fs::read_to_string(&mcp)
            .unwrap()
            .contains("crystalline")
    );
}
