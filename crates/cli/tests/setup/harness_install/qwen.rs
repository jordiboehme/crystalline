//! `crystalline install qwen` and `crystalline uninstall qwen`.

use super::support::*;
use serde_json::json;

const CRLF: &str = include_str!("../../../../core/tests/fixtures/harness/qwen-settings-crlf.json");

const QWEN_MD: &str = "# My own Qwen rules\n\nBe brief.\n";

fn settings_value(b: &Sandbox) -> serde_json::Value {
    crystalline_core::jsonc_edit::parse_value(
        &std::fs::read_to_string(b.home.join(".qwen/settings.json")).unwrap(),
    )
    .unwrap()
}

fn row(b: &Sandbox) -> serde_json::Value {
    receipt(b)["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "qwen")
        .unwrap()
        .clone()
}

#[test]
fn a_fresh_install_writes_mcp_hook_and_skills() {
    let b = sandbox();
    cmd(&b).args(["install", "qwen"]).assert().success();
    let v = settings_value(&b);
    assert_eq!(
        v["mcpServers"]["crystalline"],
        json!({"command": "crystalline", "args": ["mcp", "--harness", "qwen"]})
    );
    let groups = v["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["matcher"], "startup");
    let hook = &groups[0]["hooks"][0];
    assert_eq!(hook["timeout"], 10000);
    let command = hook["command"].as_str().unwrap();
    assert!(
        command.contains("--format hook-specific") && command.contains("--harness qwen"),
        "{command}"
    );
    assert!(
        b.home
            .join(".qwen/skills/crystalline-routing/SKILL.md")
            .is_file()
    );
    assert!(!b.home.join(".qwen/QWEN.md").exists());
    assert_eq!(
        row(&b)["parts"],
        json!({"mcp": true, "hooks": true, "skills": true})
    );
    cmd(&b).args(["uninstall", "qwen"]).assert().success();
    let text = std::fs::read_to_string(b.home.join(".qwen/settings.json")).unwrap();
    assert!(!text.contains("crystalline"), "{text}");
    assert!(!text.contains("SessionStart"), "{text}");
}

#[test]
fn the_skills_notice_is_printed() {
    let b = sandbox();
    let out = cmd(&b)
        .args(["install", "qwen"])
        .assert()
        .success()
        .get_output()
        .clone();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        all.contains("Qwen Code may need skills enabled before it loads them. Until that is confirmed, the skills also stay available over MCP."),
        "{all}"
    );
}

#[test]
fn a_crlf_settings_file_round_trips_byte_for_byte() {
    let b = sandbox();
    let p = b.home.join(".qwen/settings.json");
    write(&p, CRLF.as_bytes());
    cmd(&b).args(["install", "qwen"]).assert().success();
    let installed = std::fs::read_to_string(&p).unwrap();
    assert!(installed.contains("\"crystalline\""), "{installed}");
    assert!(installed.contains("\r\n"), "CRLF kept");
    assert!(
        !installed.replace("\r\n", "").contains('\n'),
        "no bare LF was introduced: {installed:?}"
    );
    cmd(&b).args(["uninstall", "qwen"]).assert().success();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), CRLF);
}

#[test]
fn a_second_install_is_byte_identical() {
    let b = sandbox();
    cmd(&b).args(["install", "qwen"]).assert().success();
    let p = b.home.join(".qwen/settings.json");
    let before = std::fs::read(&p).unwrap();
    let out = cmd(&b)
        .args(["--json", "install", "qwen"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(std::fs::read(&p).unwrap(), before);
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let backups = report["backups"].as_array().map_or(0, Vec::len);
    assert_eq!(backups, 0, "nothing changed, nothing backed up: {report}");
}

/// Amendment A1: install never edits an instruction file.
#[test]
fn install_never_creates_or_changes_qwen_md() {
    for flags in [
        &["install", "qwen"][..],
        &["install", "qwen", "--skip-hooks"],
    ] {
        let b = sandbox();
        cmd(&b).args(flags).assert().success();
        assert!(!b.home.join(".qwen/QWEN.md").exists(), "{flags:?}");
        let b = sandbox();
        write(&b.home.join(".qwen/QWEN.md"), QWEN_MD.as_bytes());
        cmd(&b).args(flags).assert().success();
        cmd(&b).args(flags).assert().success();
        assert_eq!(
            std::fs::read_to_string(b.home.join(".qwen/QWEN.md")).unwrap(),
            QWEN_MD,
            "{flags:?}"
        );
        cmd(&b).args(["uninstall", "qwen"]).assert().success();
        assert_eq!(
            std::fs::read_to_string(b.home.join(".qwen/QWEN.md")).unwrap(),
            QWEN_MD
        );
    }
}

#[test]
fn project_scope_is_refused_for_install_and_uninstall() {
    let b = sandbox();
    for verb in ["install", "uninstall"] {
        let out = cmd(&b)
            .args([verb, "qwen", "--project"])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        assert!(
            String::from_utf8_lossy(&out).contains(
                "project scope is not supported for Qwen Code yet; run without --project"
            )
        );
    }
    assert!(!b.home.join(".qwen").exists());
}

#[test]
fn a_malformed_settings_file_fails_mcp_and_hooks_and_leaves_the_file() {
    let b = sandbox();
    write(&b.home.join(".qwen/settings.json"), b"{ \"hooks\": ");
    let out = cmd(&b)
        .args(["--json", "install", "qwen"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["mcp"]["status"], "failed", "{report}");
    assert_eq!(report["hooks"]["session_start"], "failed", "{report}");
    assert_eq!(
        std::fs::read(b.home.join(".qwen/settings.json")).unwrap(),
        b"{ \"hooks\": "
    );
    assert!(
        b.home
            .join(".qwen/skills/crystalline-routing/SKILL.md")
            .is_file()
    );
    assert_eq!(
        row(&b)["parts"],
        json!({"mcp": false, "hooks": false, "skills": true})
    );
}

#[test]
fn a_linked_settings_file_stays_a_link_and_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let b = sandbox();
    let real = b.home.join("dotfiles/qwen-settings.json");
    write(&real, CRLF.as_bytes());
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    let link = b.home.join(".qwen/settings.json");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    cmd(&b).args(["install", "qwen"]).assert().success();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::read_to_string(&real)
            .unwrap()
            .contains("\"crystalline\"")
    );
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    cmd(&b).args(["uninstall", "qwen"]).assert().success();
    assert_eq!(std::fs::read_to_string(&real).unwrap(), CRLF);
}
