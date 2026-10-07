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
    // The task seam answers a ready machine task, so doctor never asks the
    // real Task Scheduler on a Windows runner.
    cmd.env("CRYSTALLINE_SERVICE_HTTP", "false")
        .env("CRYSTALLINE_TEST_DAEMON_TASK", "ready")
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

fn private_cli(home: &Path, folder: &Path, args: &[&str]) {
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, &home.join("elsewhere"));
    let out = cmd
        .env("CRYSTALLINE_SERVICE_HTTP", "false")
        .arg("--db")
        .arg(folder.join("index.db"))
        .args(args)
        .arg("--config")
        .arg(folder.join("config.yaml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A private state built by the binary itself: one file domain and one
/// virtual domain holding one engram.
fn plant_private_state_with_a_virtual_domain(home: &Path) -> PathBuf {
    let folder = plant_private_state(home, &["shared"]);
    std::fs::remove_file(folder.join("index.db")).unwrap();
    std::fs::write(
        home.join("docs").join("shared").join("MANIFEST.md"),
        crystalline_core::manifest_template("shared", "2026-01-01"),
    )
    .unwrap();
    private_cli(home, &folder, &["domain", "add", "vnotes", "--virtual"]);
    private_cli(
        home,
        &folder,
        &[
            "write",
            "vnotes",
            "Tide tables",
            "--content",
            "High water at the harbour is at noon.",
        ],
    );
    folder
}

fn real_config_path(home: &Path) -> PathBuf {
    let dir = if cfg!(windows) {
        home.join("roaming")
    } else {
        home.join("config")
    };
    dir.join("crystalline").join("config.yaml")
}

fn real_config(home: &Path) -> GlobalConfig {
    crystalline_core::config::load_yaml(&real_config_path(home)).unwrap()
}

/// Every file under `folder` with its bytes, by relative path.
fn snapshot(folder: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(folder).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

#[test]
fn the_merge_takes_the_config_union_moves_a_virtual_domain_and_renames_the_folder() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state_with_a_virtual_domain(home.path());
    let before = snapshot(&folder);
    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let merge = &report["merge"];
    assert_eq!(merge["conflicts"], serde_json::json!([]), "{merge}");
    assert_eq!(
        merge["imported"],
        serde_json::json!([["vnotes", 2]]),
        "{merge}"
    );
    let cfg = real_config(home.path());
    assert!(
        cfg.domains.contains_key("shared"),
        "the file domain is registered"
    );
    assert!(
        cfg.domains["vnotes"].is_virtual(),
        "the virtual domain is registered"
    );

    let read = run(home.path(), &["read", "tide-tables", "--domain", "vnotes"]);
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(String::from_utf8_lossy(&read.stdout).contains("High water at the harbour"));

    assert!(!folder.exists(), "the private folder moved away");
    let renamed = std::path::PathBuf::from(merge["renamed_to"].as_str().unwrap());
    assert!(
        renamed
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("crystalline.merged-")
    );
    assert!(renamed.join("index.db").is_file(), "renamed, never deleted");
    assert!(renamed.join("config.yaml").is_file());
    assert_eq!(
        snapshot(&renamed),
        before,
        "the merge only read the private folder: no file in it changed, none was added"
    );
}

#[test]
fn a_merge_reads_as_plain_sentences() {
    let home = tempfile::tempdir().unwrap();
    plant_private_state_with_a_virtual_domain(home.path());
    let out = run(home.path(), &["doctor", "--fix", "--merge-desktop-state"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("claude desktop:"), "{text}");
    assert!(
        text.contains("registered shared from Claude Desktop's state"),
        "{text}"
    );
    assert!(
        text.contains("moved 2 engram(s) of the virtual domain vnotes"),
        "{text}"
    );
    assert!(text.contains("renamed the private folder to "), "{text}");
    assert!(text.contains("(nothing was deleted)"), "{text}");
}

#[test]
fn a_second_merge_changes_nothing_and_says_so() {
    let home = tempfile::tempdir().unwrap();
    plant_private_state_with_a_virtual_domain(home.path());
    let first = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let renamed = PathBuf::from(first["merge"]["renamed_to"].as_str().unwrap());
    let config_before = std::fs::read(real_config_path(home.path())).unwrap();

    let again = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&again.stdout).unwrap();
    let merge = &report["merge"];
    assert_eq!(merge["registered"], serde_json::json!([]), "{merge}");
    assert_eq!(merge["imported"], serde_json::json!([]), "{merge}");
    assert_eq!(merge["renamed_to"], Value::Null, "{merge}");
    assert_eq!(
        std::fs::read(real_config_path(home.path())).unwrap(),
        config_before
    );
    let parent = renamed.parent().unwrap();
    let merged: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("crystalline"))
        .collect();
    assert_eq!(merged.len(), 1, "no second folder: {merged:?}");

    let text = run(home.path(), &["doctor", "--fix", "--merge-desktop-state"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("no private Claude Desktop state to merge"),
        "{text}"
    );
}

#[test]
fn a_name_with_two_roots_is_reported_and_the_folder_stays() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state(home.path(), &["notes"]);
    let elsewhere = home.path().join("other-notes");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut real = GlobalConfig::default();
    real.domains
        .insert("notes".to_string(), DomainEntry::file(elsewhere.clone()));
    std::fs::create_dir_all(real_config_path(home.path()).parent().unwrap()).unwrap();
    crystalline_core::config::save_yaml(&real_config_path(home.path()), &real).unwrap();

    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let conflict = report["merge"]["conflicts"][0].as_str().unwrap();
    assert!(conflict.contains("notes"), "{conflict}");
    assert!(
        conflict.contains("other-notes"),
        "both roots are named: {conflict}"
    );
    assert!(
        folder.exists(),
        "nothing is renamed while a conflict remains"
    );
    assert_eq!(
        real_config(home.path()).domains["notes"]
            .file_path()
            .unwrap(),
        elsewhere,
        "never overwritten"
    );
    assert_eq!(out.status.code(), Some(1));
}

/// With a conflict left, the folder stays, so a second run sees it again:
/// what the first run registered is now skipped as already here, and the
/// real config does not change.
#[test]
fn a_second_merge_with_a_conflict_left_registers_nothing_again() {
    let home = tempfile::tempdir().unwrap();
    plant_private_state(home.path(), &["notes", "extra"]);
    let elsewhere = home.path().join("other-notes");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut real = GlobalConfig::default();
    real.domains
        .insert("notes".to_string(), DomainEntry::file(elsewhere.clone()));
    std::fs::create_dir_all(real_config_path(home.path()).parent().unwrap()).unwrap();
    crystalline_core::config::save_yaml(&real_config_path(home.path()), &real).unwrap();

    let first = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["merge"]["registered"], serde_json::json!(["extra"]));
    let config_before = std::fs::read(real_config_path(home.path())).unwrap();

    let again = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    let merge = &again["merge"];
    assert_eq!(merge["registered"], serde_json::json!([]), "{merge}");
    assert_eq!(
        merge["already_registered"],
        serde_json::json!(["extra"]),
        "{merge}"
    );
    assert_eq!(merge["conflicts"].as_array().unwrap().len(), 1, "{merge}");
    assert_eq!(
        std::fs::read(real_config_path(home.path())).unwrap(),
        config_before,
        "the second run wrote nothing"
    );
    let text = run(home.path(), &["doctor", "--fix", "--merge-desktop-state"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("extra is already registered here the same way"),
        "{text}"
    );
    assert!(text.contains("the merge changed nothing"), "{text}");
}

#[test]
fn the_merge_refuses_inside_a_package() {
    let home = tempfile::tempdir().unwrap();
    plant_private_state(home.path(), &["notes"]);
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home.path());
    let out = cmd
        .env(
            "CRYSTALLINE_TEST_PACKAGE",
            "Claude_1.0.0.0_x64__pzs8sxrjxfjjc",
        )
        .args(["doctor", "--fix", "--merge-desktop-state"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("outside Claude Desktop"));
    assert!(private_folder(home.path()).exists());
}

#[cfg(unix)]
#[test]
fn the_merge_refuses_while_a_daemon_from_the_private_folder_runs() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state(home.path(), &["notes"]);
    // `hold-lock` publishes a real record with a real Crystalline pid. Its
    // state folder is the private one: on unix that is XDG_STATE_HOME.
    let roaming = folder.parent().unwrap();
    let mut holder = std::process::Command::new(assert_cmd::cargo::cargo_bin("crystalline"))
        .env("XDG_STATE_HOME", roaming)
        .env("XDG_CONFIG_HOME", roaming)
        .env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1")
        .args(["hold-lock", "--secs", "20"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(holder.stdout.as_mut().unwrap()),
        &mut ready,
    )
    .unwrap();
    let out = run(home.path(), &["doctor", "--fix", "--merge-desktop-state"]);
    let _ = holder.kill();
    let _ = holder.wait();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("still runs"), "{text}");
    assert!(
        text.contains("[problem] a Crystalline daemon from"),
        "the merge's own refusal, not only the scan's line: {text}"
    );
    assert!(folder.exists());
}
