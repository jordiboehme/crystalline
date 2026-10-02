//! `crystalline install gemini` and `crystalline uninstall gemini`.

use super::support::*;
use serde_json::json;

const SETTINGS: &str = include_str!("../../../../core/tests/fixtures/harness/gemini-settings.json");
const COMMENTED: &str =
    include_str!("../../../../core/tests/fixtures/harness/gemini-settings-commented.jsonc");

const GEMINI_MD: &str = "# My own Gemini rules\n\nBe brief.\n";

fn settings_value(b: &Sandbox) -> serde_json::Value {
    crystalline_core::jsonc_edit::parse_value(
        &std::fs::read_to_string(b.home.join(".gemini/settings.json")).unwrap(),
    )
    .unwrap()
}

fn row(b: &Sandbox) -> serde_json::Value {
    receipt(b)["installs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["harness"] == "gemini")
        .unwrap()
        .clone()
}

/// Review focus 2, end to end.
#[test]
fn the_real_gemini_settings_survive_install_and_uninstall_byte_for_byte() {
    for original in [SETTINGS, COMMENTED] {
        let b = sandbox();
        write(&b.home.join(".gemini/settings.json"), original.as_bytes());
        let out = cmd(&b)
            .args(["--json", "install", "gemini"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            report["backups"].as_array().unwrap().len(),
            1,
            "one backup for the one file: {report}"
        );
        let v = settings_value(&b);
        assert_eq!(
            v["mcpServers"]["crystalline"],
            json!({"command": "crystalline", "args": ["mcp", "--harness", "gemini"]})
        );
        let groups = v["hooks"]["SessionStart"].as_array().unwrap();
        let ours = groups.last().unwrap();
        assert_eq!(ours["matcher"], "startup");
        assert_eq!(ours["hooks"][0]["timeout"], 10000);
        cmd(&b).args(["uninstall", "gemini"]).assert().success();
        assert_eq!(
            std::fs::read_to_string(b.home.join(".gemini/settings.json")).unwrap(),
            original
        );
    }
}

#[test]
fn a_fresh_install_writes_one_settings_file_and_the_shared_skills() {
    let b = sandbox();
    cmd(&b).args(["install", "gemini"]).assert().success();
    let v = settings_value(&b);
    assert!(v["mcpServers"]["crystalline"].is_object(), "{v}");
    assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    assert!(
        b.home
            .join(".agents/skills/crystalline-routing/SKILL.md")
            .is_file()
    );
    assert!(!b.home.join(".gemini/GEMINI.md").exists());
    assert_eq!(
        row(&b)["parts"],
        json!({"mcp": true, "hooks": true, "skills": true})
    );
    // A group holding only our hook goes whole on uninstall.
    cmd(&b).args(["uninstall", "gemini"]).assert().success();
    let text = std::fs::read_to_string(b.home.join(".gemini/settings.json")).unwrap();
    assert!(!text.contains("crystalline"), "{text}");
    assert!(!text.contains("SessionStart"), "{text}");
}

#[test]
fn a_second_install_is_byte_identical() {
    let b = sandbox();
    cmd(&b).args(["install", "gemini"]).assert().success();
    let p = b.home.join(".gemini/settings.json");
    let before = std::fs::read(&p).unwrap();
    let out = cmd(&b)
        .args(["--json", "install", "gemini"])
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
fn install_never_creates_or_changes_gemini_md() {
    for flags in [
        &["install", "gemini"][..],
        &["install", "gemini", "--skip-hooks"],
    ] {
        let b = sandbox();
        cmd(&b).args(flags).assert().success();
        assert!(!b.home.join(".gemini/GEMINI.md").exists(), "{flags:?}");
        let b = sandbox();
        write(&b.home.join(".gemini/GEMINI.md"), GEMINI_MD.as_bytes());
        cmd(&b).args(flags).assert().success();
        cmd(&b).args(flags).assert().success();
        assert_eq!(
            std::fs::read_to_string(b.home.join(".gemini/GEMINI.md")).unwrap(),
            GEMINI_MD,
            "{flags:?}"
        );
        cmd(&b).args(["uninstall", "gemini"]).assert().success();
        assert_eq!(
            std::fs::read_to_string(b.home.join(".gemini/GEMINI.md")).unwrap(),
            GEMINI_MD
        );
    }
}

#[test]
fn a_hook_group_shared_with_a_foreign_hook_keeps_the_foreign_one() {
    let b = sandbox();
    cmd(&b).args(["install", "gemini"]).assert().success();
    let p = b.home.join(".gemini/settings.json");
    let mut v = settings_value(&b);
    v["hooks"]["SessionStart"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "command", "command": "echo mine"}));
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    cmd(&b).args(["uninstall", "gemini"]).assert().success();
    let left = settings_value(&b);
    let hooks = left["hooks"]["SessionStart"][0]["hooks"]
        .as_array()
        .unwrap();
    assert_eq!(hooks.len(), 1, "{left}");
    assert_eq!(hooks[0]["command"], "echo mine");
}

#[test]
fn a_malformed_settings_file_fails_mcp_and_hooks_and_leaves_the_file() {
    let b = sandbox();
    write(&b.home.join(".gemini/settings.json"), b"{ \"hooks\": ");
    let out = cmd(&b)
        .args(["--json", "install", "gemini"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["mcp"]["status"], "failed", "{report}");
    assert_eq!(report["hooks"]["session_start"], "failed", "{report}");
    assert_eq!(
        std::fs::read(b.home.join(".gemini/settings.json")).unwrap(),
        b"{ \"hooks\": "
    );
    assert!(
        b.home
            .join(".agents/skills/crystalline-routing/SKILL.md")
            .is_file()
    );
    assert_eq!(
        row(&b)["parts"],
        json!({"mcp": false, "hooks": false, "skills": true})
    );
}

#[test]
fn project_scope_is_refused_for_install_and_uninstall() {
    let b = sandbox();
    for verb in ["install", "uninstall"] {
        let out = cmd(&b)
            .args([verb, "gemini", "--project"])
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone();
        assert!(
            String::from_utf8_lossy(&out).contains(
                "project scope is not supported for Gemini CLI yet; run without --project"
            )
        );
    }
    assert!(!b.home.join(".gemini").exists());
}

#[test]
fn the_skip_flags_leave_their_part_alone() {
    let b = sandbox();
    cmd(&b)
        .args(["install", "gemini", "--skip-hooks", "--skip-skills"])
        .assert()
        .success();
    let v = settings_value(&b);
    assert!(v["mcpServers"]["crystalline"].is_object());
    assert!(v.get("hooks").is_none(), "{v}");
    assert!(!b.home.join(".agents/skills").exists());
    assert!(!b.home.join(".gemini/GEMINI.md").exists());

    let b = sandbox();
    cmd(&b)
        .args(["install", "gemini", "--skip-mcp"])
        .assert()
        .success();
    let v = settings_value(&b);
    assert!(v.get("mcpServers").is_none(), "{v}");
    assert!(v["hooks"]["SessionStart"].is_array());
}

/// A dotfiles setup links the settings file into a repository: install and
/// uninstall write through the link and keep the mode.
#[test]
fn a_linked_private_file_stays_a_link_and_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let b = sandbox();
    let real = b.home.join("dotfiles/gemini-settings.json");
    write(&real, SETTINGS.as_bytes());
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    let link = b.home.join(".gemini/settings.json");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    cmd(&b).args(["install", "gemini"]).assert().success();
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
            .contains("\"crystalline\"")
    );
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    cmd(&b).args(["uninstall", "gemini"]).assert().success();
    assert!(is_link(&link));
    assert_eq!(std::fs::read_to_string(&real).unwrap(), SETTINGS);
}
