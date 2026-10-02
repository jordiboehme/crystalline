//! `crystalline install cursor` and `crystalline uninstall cursor`.

use super::support::*;
use serde_json::json;

const HOOKS: &str = include_str!("../../../../core/tests/fixtures/harness/cursor-hooks.json");
const HOOKS_COMMENTED: &str =
    include_str!("../../../../core/tests/fixtures/harness/cursor-hooks-commented.jsonc");
const MCP: &str = include_str!("../../../../core/tests/fixtures/harness/cursor-mcp.json");

fn abs(b: &Sandbox) -> String {
    b.bin.join("crystalline").display().to_string()
}

#[test]
fn a_fresh_install_writes_the_mcp_entry_the_hook_and_the_skills() {
    let b = sandbox();
    cmd(&b).args(["install", "cursor"]).assert().success();
    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(b.home.join(".cursor/mcp.json")).unwrap()).unwrap();
    assert_eq!(
        mcp,
        json!({"mcpServers": {"crystalline": {"type": "stdio", "command": abs(&b), "args": ["mcp", "--harness", "cursor"]}}}),
        "the absolute PATH entry, the symlink and not its target"
    );
    let hooks: serde_json::Value =
        serde_json::from_slice(&std::fs::read(b.home.join(".cursor/hooks.json")).unwrap()).unwrap();
    assert_eq!(
        hooks,
        json!({"version": 1, "hooks": {"sessionStart": [{"command": format!("{} prompt system --format cursor --harness cursor", abs(&b))}]}})
    );
    for name in [
        "crystalline-routing",
        "crystalline-capture",
        "crystalline-schema",
        "crystalline-collaboration",
    ] {
        assert!(
            b.home
                .join(".agents/skills")
                .join(name)
                .join("SKILL.md")
                .is_file(),
            "{name}"
        );
    }
    let r = receipt(&b);
    let row = r["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "cursor")
        .unwrap();
    assert_eq!(
        row["parts"],
        json!({"mcp": true, "hooks": true, "skills": true})
    );
}

#[test]
fn a_second_install_is_byte_identical() {
    let b = sandbox();
    cmd(&b).args(["install", "cursor"]).assert().success();
    let snap = |p: &str| std::fs::read(b.home.join(p)).unwrap();
    let (m, h) = (snap(".cursor/mcp.json"), snap(".cursor/hooks.json"));
    cmd(&b).args(["install", "cursor"]).assert().success();
    assert_eq!(snap(".cursor/mcp.json"), m);
    assert_eq!(snap(".cursor/hooks.json"), h);
}

#[test]
fn the_users_real_files_come_back_byte_for_byte() {
    for (hooks, mcp) in [(HOOKS, MCP), (HOOKS_COMMENTED, MCP)] {
        let b = sandbox();
        write(&b.home.join(".cursor/hooks.json"), hooks.as_bytes());
        write(&b.home.join(".cursor/mcp.json"), mcp.as_bytes());
        let out = cmd(&b)
            .args(["--json", "install", "cursor"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            report["backups"].as_array().unwrap().len(),
            2,
            "both files backed up once: {report}"
        );
        cmd(&b).args(["uninstall", "cursor"]).assert().success();
        assert_eq!(
            std::fs::read_to_string(b.home.join(".cursor/hooks.json")).unwrap(),
            hooks
        );
        assert_eq!(
            std::fs::read_to_string(b.home.join(".cursor/mcp.json")).unwrap(),
            mcp
        );
        assert!(!b.home.join(".agents/skills/crystalline-routing").exists());
    }
}

#[test]
fn an_edit_between_install_and_uninstall_survives() {
    let b = sandbox();
    write(&b.home.join(".cursor/mcp.json"), MCP.as_bytes());
    cmd(&b).args(["install", "cursor"]).assert().success();
    let p = b.home.join(".cursor/mcp.json");
    let edited = std::fs::read_to_string(&p)
        .unwrap()
        .replace("other-mcp", "other-mcp@2");
    std::fs::write(&p, &edited).unwrap();
    cmd(&b).args(["uninstall", "cursor"]).assert().success();
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        MCP.replace("other-mcp", "other-mcp@2")
    );
}

#[test]
fn a_malformed_file_is_refused_and_the_other_parts_still_install() {
    let b = sandbox();
    write(&b.home.join(".cursor/mcp.json"), b"{ \"mcpServers\": ");
    let out = cmd(&b)
        .args(["--json", "install", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["mcp"]["status"], "failed");
    assert!(
        report["mcp"]["manual_command"]
            .as_str()
            .unwrap()
            .contains("mcpServers")
    );
    let error = report["mcp"]["error"].as_str().unwrap();
    assert!(
        error.contains(".cursor/mcp.json") && error.contains("line 1, column"),
        "the path, the line and the column: {error}"
    );
    assert_eq!(
        std::fs::read(b.home.join(".cursor/mcp.json")).unwrap(),
        b"{ \"mcpServers\": ",
        "left unchanged"
    );
    assert!(
        b.home.join(".cursor/hooks.json").is_file(),
        "the hook still installed"
    );
    let r = receipt(&b);
    let row = r["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "cursor")
        .unwrap();
    assert_eq!(
        row["parts"],
        json!({"mcp": false, "hooks": true, "skills": true}),
        "the failed part is not recorded"
    );
}

/// A hooks file whose `hooks` is not an object holds nothing of ours and
/// gets nothing: the hook part fails, the file stays, and the receipt does
/// not claim a hook the MCP gate would then trust.
#[test]
fn a_hooks_value_of_a_foreign_type_fails_the_hook_part_only() {
    let b = sandbox();
    write(
        &b.home.join(".cursor/hooks.json"),
        b"{\"version\": 1, \"hooks\": [\"x\"]}\n",
    );
    let out = cmd(&b)
        .args(["--json", "install", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["hooks"]["session_start"], "failed");
    assert!(report["hooks"].get("stop").is_none(), "{report}");
    assert!(report["hooks"].get("prompt").is_none(), "{report}");
    assert_eq!(
        std::fs::read(b.home.join(".cursor/hooks.json")).unwrap(),
        b"{\"version\": 1, \"hooks\": [\"x\"]}\n"
    );
    assert_eq!(report["mcp"]["status"], "registered");
    let r = receipt(&b);
    let row = r["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "cursor")
        .unwrap();
    assert_eq!(row["parts"]["hooks"], false);
}

#[test]
fn project_scope_is_refused_for_install_and_uninstall() {
    let b = sandbox();
    for verb in ["install", "uninstall"] {
        let out = cmd(&b)
            .args([verb, "cursor", "--project"])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        assert!(
            String::from_utf8_lossy(&out)
                .contains("project scope is not supported for Cursor yet; run without --project")
        );
    }
    assert!(!b.home.join(".cursor").exists());
}

#[test]
fn the_skip_flags_leave_their_part_alone() {
    let b = sandbox();
    cmd(&b)
        .args(["install", "cursor", "--skip-mcp", "--skip-skills"])
        .assert()
        .success();
    assert!(!b.home.join(".cursor/mcp.json").exists());
    assert!(!b.home.join(".agents/skills").exists());
    assert!(b.home.join(".cursor/hooks.json").is_file());
    let b = sandbox();
    cmd(&b)
        .args(["install", "cursor", "--skip-hooks"])
        .assert()
        .success();
    assert!(!b.home.join(".cursor/hooks.json").exists());
}

/// An explicit install points the harness at the crystalline on PATH, even
/// when the files name an older binary that still runs: keeping it would
/// leave Cursor on the old binary with no way to move it.
#[test]
fn an_explicit_install_replaces_a_stale_stored_program_with_the_path_binary() {
    use std::os::unix::fs::PermissionsExt;
    let b = sandbox();
    let old = b.home.join("old/bin/crystalline");
    write(&old, b"#!/bin/sh\necho 'crystalline 0.21.0'\n");
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = old.display().to_string();
    write(
        &b.home.join(".cursor/hooks.json"),
        json!({"version": 1, "hooks": {"sessionStart": [{"command": format!("{old} prompt system --format cursor --harness cursor")}]}})
            .to_string()
            .as_bytes(),
    );
    write(
        &b.home.join(".cursor/mcp.json"),
        json!({"mcpServers": {"crystalline": {"type": "stdio", "command": old, "args": ["mcp", "--harness", "cursor"]}}})
            .to_string()
            .as_bytes(),
    );
    let out = cmd(&b)
        .args(["install", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let read = |p: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(b.home.join(p)).unwrap()).unwrap()
    };
    assert_eq!(
        read(".cursor/hooks.json")["hooks"]["sessionStart"][0]["command"],
        format!("{} prompt system --format cursor --harness cursor", abs(&b))
    );
    assert_eq!(
        read(".cursor/mcp.json")["mcpServers"]["crystalline"]["command"],
        abs(&b)
    );
    let out = String::from_utf8_lossy(&out);
    assert!(
        !out.contains("0.21.0"),
        "no notice about the old binary: {out}"
    );
}

/// Decision 6 and review focus 4.
#[test]
fn a_session_start_reconcile_without_crystalline_on_path_keeps_the_absolute_hook() {
    let b = sandbox();
    cmd(&b).args(["install", "cursor"]).assert().success();
    // Pretend an older version installed it, so the session start reconciles.
    let p = crate::common::isolated_state_dir(&b.home).join("installs.json");
    let text = std::fs::read_to_string(&p)
        .unwrap()
        .replace(env!("CARGO_PKG_VERSION"), "0.22.0");
    std::fs::write(&p, text).unwrap();
    let before = std::fs::read(b.home.join(".cursor/hooks.json")).unwrap();
    let empty = tempfile::tempdir().unwrap();
    let mut c = cmd(&b);
    c.env("PATH", empty.path())
        .args([
            "prompt",
            "system",
            "--format",
            "cursor",
            "--harness",
            "cursor",
        ])
        .write_stdin("{\"source\":\"startup\"}");
    c.assert().success();
    assert_eq!(
        std::fs::read(b.home.join(".cursor/hooks.json")).unwrap(),
        before,
        "the absolute spelling is kept"
    );
    let r = std::fs::read_to_string(&p).unwrap();
    assert!(
        r.contains(env!("CARGO_PKG_VERSION")),
        "the refresh ran and stamped this version: {r}"
    );
}

/// A dotfiles setup links the harness files into a repository: install and
/// uninstall write through the link and keep the file private.
#[test]
fn a_linked_private_file_stays_a_link_and_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let b = sandbox();
    let real = b.home.join("dotfiles/cursor-mcp.json");
    write(&real, MCP.as_bytes());
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    let link = b.home.join(".cursor/mcp.json");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    cmd(&b).args(["install", "cursor"]).assert().success();
    let is_link = |p: &std::path::Path| {
        std::fs::symlink_metadata(p)
            .unwrap()
            .file_type()
            .is_symlink()
    };
    assert!(is_link(&link), "the link is kept");
    assert!(
        std::fs::read_to_string(&real)
            .unwrap()
            .contains("\"crystalline\""),
        "written through the link"
    );
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    cmd(&b).args(["uninstall", "cursor"]).assert().success();
    assert!(is_link(&link));
    assert_eq!(std::fs::read_to_string(&real).unwrap(), MCP);
}
