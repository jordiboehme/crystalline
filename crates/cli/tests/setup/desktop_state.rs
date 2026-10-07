//! The private state an old Claude Desktop extension may have left inside
//! Claude Desktop's package, as `doctor` and `status` report it. The scan
//! reads `LOCALAPPDATA`, which the isolated environment points at
//! `<home>/local` on every platform.

use std::path::{Path, PathBuf};

use crystalline_core::config::{DomainEntry, GlobalConfig};
use serde_json::Value;

pub(crate) fn private_folder(home: &Path) -> PathBuf {
    home.join("local")
        .join("Packages")
        .join("Claude_pzs8sxrjxfjjc")
        .join("LocalCache")
        .join("Roaming")
        .join("crystalline")
}

/// A private state with `files` file domains (each a folder under `home`)
/// and a 4 KiB `index.db`.
pub(crate) fn plant_private_state(home: &Path, files: &[&str]) -> PathBuf {
    let folder = private_folder(home);
    std::fs::create_dir_all(&folder).unwrap();
    let mut cfg = GlobalConfig::default();
    for name in files {
        let root = home.join("docs").join(name);
        std::fs::create_dir_all(&root).unwrap();
        cfg.domains
            .insert(name.to_string(), DomainEntry::file(root));
    }
    crystalline_core::config::save_yaml(&folder.join("config.yaml"), &cfg).unwrap();
    std::fs::write(folder.join("index.db"), vec![0u8; 4096]).unwrap();
    folder
}

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home);
    cmd.env("CRYSTALLINE_SERVICE_HTTP", "false")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn doctor_reports_a_split_state_as_a_problem() {
    let home = tempfile::tempdir().unwrap();
    plant_private_state(home.path(), &["notes", "work"]);
    let out = run(home.path(), &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1), "a split state is a problem");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let state = &report["desktop_states"][0];
    assert_eq!(state["domains"], 2);
    assert_eq!(state["index_bytes"], 4096);
    assert!(state["folder"].as_str().unwrap().contains("LocalCache"));

    let human = run(home.path(), &["doctor"]);
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(
        text.contains("[problem] Claude Desktop kept its own Crystalline state"),
        "{text}"
    );
    assert!(text.contains("--merge-desktop-state"), "{text}");
}

#[test]
fn status_names_a_split_state_in_one_line() {
    let home = tempfile::tempdir().unwrap();
    plant_private_state(home.path(), &["notes"]);
    let out = run(home.path(), &["status"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Desktop state: "), "{text}");
    let json: Value =
        serde_json::from_slice(&run(home.path(), &["status", "--json"]).stdout).unwrap();
    assert_eq!(json["desktop_states"][0]["domains"], 1);
}

#[test]
fn without_a_split_state_nothing_is_said() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["doctor", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a fresh home has no problem, so the split state test's exit code means something"
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["desktop_states"], serde_json::json!([]));
    let status = run(home.path(), &["status"]);
    assert!(!String::from_utf8_lossy(&status.stdout).contains("Desktop state"));
}
