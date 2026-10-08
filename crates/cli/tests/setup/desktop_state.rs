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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("Daemon: bypassed"),
        "the export from the private copy says nothing: {stderr}"
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
    assert_eq!(
        out.status.code(),
        Some(0),
        "a clean merge leaves no problem: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A private file domain whose folder sits inside the private state folder
/// would move away with the rename, and one whose folder does not exist here
/// would be a broken registration. Neither is registered; both are named,
/// and the folder stays. Once this machine registers the name itself, its
/// registration is kept and the merge goes through.
#[test]
fn a_private_domain_with_an_unusable_folder_is_not_registered() {
    let home = tempfile::tempdir().unwrap();
    let folder = private_folder(home.path());
    let inside = folder.join("domains").join("inner");
    std::fs::create_dir_all(&inside).unwrap();
    let gone = home.path().join("gone");
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("inner".to_string(), DomainEntry::file(inside.clone()));
    cfg.domains
        .insert("gone".to_string(), DomainEntry::file(gone.clone()));
    crystalline_core::config::save_yaml(&folder.join("config.yaml"), &cfg).unwrap();

    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let merge = &report["merge"];
    assert_eq!(merge["registered"], serde_json::json!([]), "{merge}");
    let conflicts = merge["conflicts"].to_string();
    assert!(
        conflicts.contains("inside Claude Desktop's private folder"),
        "{conflicts}"
    );
    assert!(conflicts.contains("does not exist here"), "{conflicts}");
    assert!(
        folder.exists(),
        "nothing is renamed while a problem remains"
    );
    assert!(
        !real_config_path(home.path()).exists(),
        "nothing registered"
    );
    assert_eq!(out.status.code(), Some(1));

    // The person moves the files out and registers both names here.
    let mut real = GlobalConfig::default();
    for name in ["inner", "gone"] {
        let own = home.path().join("own").join(name);
        std::fs::create_dir_all(&own).unwrap();
        real.domains
            .insert(name.to_string(), DomainEntry::file(own));
    }
    std::fs::create_dir_all(real_config_path(home.path()).parent().unwrap()).unwrap();
    crystalline_core::config::save_yaml(&real_config_path(home.path()), &real).unwrap();
    let again = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    let merge = &again["merge"];
    assert_eq!(merge["conflicts"], serde_json::json!([]), "{merge}");
    assert_eq!(
        merge["kept_this_machine"].as_array().unwrap().len(),
        2,
        "{merge}"
    );
    assert!(merge["renamed_to"].is_string(), "{merge}");
    assert!(!folder.exists());
    let renamed = merge["renamed_to"].as_str().unwrap();
    let inner = merge["kept_this_machine"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .find(|line| line.contains("'inner'"))
        .unwrap()
        .to_string();
    assert!(
        inner.contains(&format!("now in {renamed}")),
        "the files inside the private folder are named where they went: {inner}"
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
        text.contains("moved 2 engrams of the virtual domain vnotes"),
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

/// Replace the stored text of one engram in a private index with text the
/// parser refuses, as an older build might have stored it.
fn break_engram(index: &Path, domain: &str, permalink: &str) {
    use crystalline_core::parse_engram;
    use crystalline_index::{DomainKind, EngramRecord, Store, TursoStore};
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = rt.block_on(TursoStore::open(index)).unwrap();
    let id = rt
        .block_on(store.upsert_domain(domain, None, DomainKind::Virtual))
        .unwrap();
    let rows = rt.block_on(store.all_engram_contents(id)).unwrap();
    let row = rows.iter().find(|r| r.permalink == permalink).unwrap();
    let stamp = rt.block_on(store.file_stamps(id)).unwrap()[&row.path].clone();
    let mut record =
        EngramRecord::from_engram(&parse_engram(&row.content).unwrap(), &row.path, stamp);
    record.content = "---\ntitle: [never closed\n---\nbody\n".to_string();
    rt.block_on(store.upsert_engram(id, &record)).unwrap();
}

#[test]
fn an_engram_the_import_cannot_read_keeps_the_folder_and_is_named() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state_with_a_virtual_domain(home.path());
    break_engram(&folder.join("index.db"), "vnotes", "tide-tables");

    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let merge = &report["merge"];
    let not_moved = merge["not_moved"].to_string();
    assert!(not_moved.contains("vnotes"), "{merge}");
    assert!(
        not_moved.contains("fix or remove them") && not_moved.contains("run the merge again"),
        "the sentence says what to do: {merge}"
    );
    assert!(
        not_moved.contains("tide-tables.md"),
        "the file is named: {merge}"
    );
    assert!(
        folder.exists(),
        "the folder stays while an engram was not moved"
    );
    assert_eq!(merge["renamed_to"], Value::Null, "{merge}");
    assert_eq!(merge["kept_both"], serde_json::json!([]), "{merge}");
    assert_eq!(out.status.code(), Some(1));
    let text = run(home.path(), &["doctor", "--fix", "--merge-desktop-state"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("[problem] 'vnotes'"), "{text}");
    assert!(text.contains("tide-tables.md"), "{text}");
}

/// A MANIFEST whose frontmatter declares `domain_name: <name>`.
fn manifest_declaring(name: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\nstatus: stable\ndomain_name: {name}\n---\n\n# {name}\n"
    )
}

/// A different folder whose MANIFEST declares a name this machine already
/// answers to under another domain is refused: two domains would claim one
/// canonical name. The folder stays.
#[test]
fn a_canonical_name_this_machine_answers_to_is_a_conflict() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state(home.path(), &["other"]);
    std::fs::write(
        home.path().join("docs").join("other").join("MANIFEST.md"),
        manifest_declaring("mine"),
    )
    .unwrap();
    let own = home.path().join("own").join("mine");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("MANIFEST.md"), manifest_declaring("mine")).unwrap();
    let mut real = GlobalConfig::default();
    real.domains
        .insert("mine".to_string(), DomainEntry::file(own));
    std::fs::create_dir_all(real_config_path(home.path()).parent().unwrap()).unwrap();
    crystalline_core::config::save_yaml(&real_config_path(home.path()), &real).unwrap();

    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let merge = &report["merge"];
    let conflict = merge["conflicts"][0].as_str().unwrap_or_default();
    assert!(
        conflict.contains("'other'") && conflict.contains("'mine'"),
        "{merge}"
    );
    assert_eq!(merge["registered"], serde_json::json!([]), "{merge}");
    assert_eq!(merge["renamed_to"], Value::Null, "{merge}");
    assert!(
        folder.exists(),
        "nothing is renamed while a conflict remains"
    );
    assert!(!real_config(home.path()).domains.contains_key("other"));
    assert_eq!(out.status.code(), Some(1));
}

/// A folder this machine already registers under another name is not
/// registered a second time: the private name is skipped and both names
/// are said.
#[test]
fn the_same_folder_under_another_name_is_skipped() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state(home.path(), &["theirs"]);
    let root = home.path().join("docs").join("theirs");
    // As `domain add` leaves it: the MANIFEST declares this machine's name.
    std::fs::write(root.join("MANIFEST.md"), manifest_declaring("mine")).unwrap();
    let mut real = GlobalConfig::default();
    real.domains
        .insert("mine".to_string(), DomainEntry::file(root));
    std::fs::create_dir_all(real_config_path(home.path()).parent().unwrap()).unwrap();
    crystalline_core::config::save_yaml(&real_config_path(home.path()), &real).unwrap();

    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let merge = &report["merge"];
    assert_eq!(merge["registered"], serde_json::json!([]), "{merge}");
    let kept = merge["kept_this_machine"].to_string();
    assert!(
        kept.contains("'theirs'") && kept.contains("'mine'"),
        "{merge}"
    );
    let cfg = real_config(home.path());
    assert!(!cfg.domains.contains_key("theirs"), "never a second name");
    assert!(
        !folder.exists(),
        "nothing was lost, so the folder is renamed"
    );
}

/// After a conflict the folder stays, and a second run finds the virtual
/// domain it moved the first time on both sides. Those engrams are the
/// first run's own copies, not ones this machine kept against the private
/// side.
#[test]
fn a_second_merge_does_not_count_its_own_moved_engrams_as_kept() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state_with_a_virtual_domain(home.path());
    let elsewhere = home.path().join("other-shared");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut real = GlobalConfig::default();
    real.domains
        .insert("shared".to_string(), DomainEntry::file(elsewhere));
    std::fs::create_dir_all(real_config_path(home.path()).parent().unwrap()).unwrap();
    crystalline_core::config::save_yaml(&real_config_path(home.path()), &real).unwrap();

    let first = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(
        first["merge"]["imported"],
        serde_json::json!([["vnotes", 2]])
    );
    assert!(folder.exists(), "a conflict keeps the folder");

    let again = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    let merge = &again["merge"];
    assert_eq!(merge["imported"], serde_json::json!([]), "{merge}");
    assert_eq!(merge["kept_both"], serde_json::json!([]), "{merge}");
}

#[test]
fn the_merge_refuses_a_domain_filter() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_state(home.path(), &["notes"]);
    let out = run(
        home.path(),
        &[
            "doctor",
            "--fix",
            "--merge-desktop-state",
            "--domain",
            "notes",
        ],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("leave out --domain"), "{stderr}");
    assert!(folder.exists());
}

/// A private state holding one team domain, `brand`: its folder under
/// `home`, its origin block in the private config and its team state
/// (`origins/brand`) in the private folder, the way an old extension leaves
/// it. The real config turns GitHub on, so `origin status` answers here.
fn plant_private_team_domain(home: &Path) -> PathBuf {
    let folder = private_folder(home);
    let root = home.join("docs").join("brand");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), "# Manifest\n").unwrap();
    std::fs::write(root.join("a.md"), "# Team\n\nHello.\n").unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("config.yaml"),
        format!(
            "domains:\n  brand:\n    path: {}\n    origin:\n      repo: acme/brand-knowledge\n      branch: main\ngithub:\n  enabled: true\n",
            serde_json::to_string(&root).unwrap()
        ),
    )
    .unwrap();
    let origin = folder.join("origins").join("brand");
    std::fs::create_dir_all(origin.join("base")).unwrap();
    std::fs::write(origin.join("base").join("a.md"), "# Team\n\nHello.\n").unwrap();
    std::fs::write(
        origin.join("state.json"),
        r#"{"version":1,"repo":"acme/brand-knowledge","branch":"main","base_commit":"abc123","ref_etag":null,"last_checked":null,"files":{"a.md":{"sha256":"c3c11220a2499569be3fefd408a950e49125ad33d587a26dadbcb210127098fc","size":15}},"proposals":[],"history":[],"conflicts":[]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(real_config_path(home).parent().unwrap()).unwrap();
    std::fs::write(real_config_path(home), "github:\n  enabled: true\n").unwrap();
    folder
}

fn real_origins(home: &Path) -> PathBuf {
    crate::common::isolated_state_dir(home).join("origins")
}

/// A team domain only the old extension knew comes over with its team
/// state: the merge copies `origins/<name>` (the private copy stays in the
/// renamed folder), so the real side can report, update and share it.
#[test]
fn a_team_domain_comes_over_with_its_team_state() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_team_domain(home.path());
    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let merge = &report["merge"];
    assert_eq!(merge["conflicts"], serde_json::json!([]), "{merge}");
    assert_eq!(merge["registered"], serde_json::json!(["brand"]), "{merge}");

    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home.path());
    let status = cmd
        .env("CRYSTALLINE_SERVICE_HTTP", "false")
        .env_remove("CRYSTALLINE_GITHUB_TOKEN")
        .args(["--json", "origin", "status", "--domain", "brand"])
        .output()
        .unwrap();
    let status: Value = serde_json::from_slice(&status.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {} {}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        )
    });
    assert_eq!(status["errors"], serde_json::json!([]), "{status}");
    assert_eq!(status["domains"][0]["domain"], "brand", "{status}");
    assert_eq!(status["domains"][0]["base_commit"], "abc123", "{status}");

    assert_eq!(
        merge["origins_copied"],
        serde_json::json!(["brand"]),
        "{merge}"
    );
    let renamed = PathBuf::from(merge["renamed_to"].as_str().unwrap());
    assert!(!folder.exists());
    assert_eq!(
        snapshot(&renamed.join("origins").join("brand")),
        snapshot(&real_origins(home.path()).join("brand")),
        "copied, and the private copy stays where the rename put it"
    );
    let text = run(home.path(), &["doctor"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(!text.contains("no origin state on disk"), "{text}");
}

/// This machine already holds team state under the name (left from an
/// earlier domain): it is never overwritten, so the domain is not
/// registered and the folder stays.
#[test]
fn team_state_this_machine_already_has_is_never_overwritten() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_team_domain(home.path());
    let real = real_origins(home.path()).join("brand");
    std::fs::create_dir_all(&real).unwrap();
    let other = std::fs::read_to_string(folder.join("origins/brand/state.json"))
        .unwrap()
        .replace("abc123", "def456");
    std::fs::write(real.join("state.json"), &other).unwrap();
    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let merge = &report["merge"];
    assert_eq!(merge["registered"], serde_json::json!([]), "{merge}");
    let conflict = merge["conflicts"][0].as_str().unwrap();
    assert!(conflict.contains("'brand'"), "{conflict}");
    assert!(conflict.contains("team state"), "{conflict}");
    assert_eq!(
        std::fs::read_to_string(real.join("state.json")).unwrap(),
        other
    );
    assert!(
        folder.exists(),
        "nothing is renamed while a problem remains"
    );
    assert!(!real_config(home.path()).domains.contains_key("brand"));
    assert_eq!(out.status.code(), Some(1));
}

/// A copy that fails registers nothing and leaves the folder where it is.
#[test]
fn a_team_state_that_cannot_be_copied_keeps_the_domain_out() {
    let home = tempfile::tempdir().unwrap();
    let folder = plant_private_team_domain(home.path());
    // A file where the origins folder belongs: no copy can land. GitHub
    // stays off here, so doctor does not look for a token in that folder.
    std::fs::write(real_config_path(home.path()), "domains: {}\n").unwrap();
    let origins = real_origins(home.path());
    std::fs::create_dir_all(origins.parent().unwrap()).unwrap();
    std::fs::write(&origins, "not a folder").unwrap();
    let out = run(
        home.path(),
        &["doctor", "--fix", "--merge-desktop-state", "--json"],
    );
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    let merge = &report["merge"];
    assert_eq!(merge["registered"], serde_json::json!([]), "{merge}");
    let conflict = merge["conflicts"][0].as_str().unwrap();
    assert!(conflict.contains("could not be copied"), "{conflict}");
    assert!(
        folder.exists(),
        "nothing is renamed while a problem remains"
    );
    assert!(
        folder
            .join("origins")
            .join("brand")
            .join("state.json")
            .is_file()
    );
    assert!(!real_config(home.path()).domains.contains_key("brand"));
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_merged_team_domain_reads_as_plain_sentences() {
    let home = tempfile::tempdir().unwrap();
    plant_private_team_domain(home.path());
    let out = run(home.path(), &["doctor", "--fix", "--merge-desktop-state"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("copied the team state of brand from Claude Desktop's state"),
        "{text}"
    );
    assert!(
        text.contains("the private copies stay in the renamed folder"),
        "{text}"
    );
    assert!(text.contains("crystalline origin share"), "{text}");
    assert!(!text.contains("old extension"), "{text}");
}

/// With team domains turned off here, the report says how to turn them on
/// instead of promising that the domain shares as before.
#[test]
fn a_merged_team_domain_says_to_turn_team_domains_on() {
    let home = tempfile::tempdir().unwrap();
    plant_private_team_domain(home.path());
    std::fs::write(real_config_path(home.path()), "domains: {}\n").unwrap();
    let out = run(home.path(), &["doctor", "--fix", "--merge-desktop-state"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("copied the team state of brand from Claude Desktop's state. Team domains are turned off here: run crystalline config set github.enabled true"),
        "{text}"
    );
    assert!(!text.contains("shares as before"), "{text}");
    assert!(
        text.contains("run crystalline config set github.enabled true, then share them"),
        "{text}"
    );
}
