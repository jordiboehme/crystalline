//! `doctor`'s daemon task section through the debug seam
//! `CRYSTALLINE_TEST_DAEMON_TASK`. Windows only: the check runs only there,
//! and only when the task concerns the machine, which a `Packages\Claude_*`
//! folder under the isolated `LOCALAPPDATA` makes it do.
#![cfg(windows)]

use std::path::Path;

use serde_json::{Value, json};

fn doctor(home: &Path, task: &str, args: &[&str]) -> std::process::Output {
    std::fs::create_dir_all(
        home.join("local")
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc"),
    )
    .unwrap();
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home);
    cmd.env("CRYSTALLINE_SERVICE_HTTP", "false")
        .env("CRYSTALLINE_TEST_DAEMON_TASK", task)
        .arg("doctor")
        .args(args)
        .output()
        .unwrap()
}

/// This user's task under the 0.24.0 name is a warning. `--fix` would move
/// it, but a test binary is not the MSI's, so it says why it did not.
#[test]
fn doctor_reports_a_task_with_the_old_name_and_fix_says_why_it_stays() {
    let home = tempfile::tempdir().unwrap();
    let old = format!(
        r"\Crystalline Daemon for {}",
        std::env::var("USERNAME").unwrap()
    );
    let out = doctor(home.path(), "legacy", &["--json"]);
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(
        report["daemon_task"]["finding"],
        json!({ "state": "ready", "name": old })
    );
    assert_eq!(report["daemon_task"]["old_name"], json!(old));
    assert_eq!(out.status.code(), Some(0), "a warning, not a problem");

    let text = doctor(home.path(), "legacy", &[]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("has the name from Crystalline 0.24.0"),
        "{text}"
    );

    let fixed = doctor(home.path(), "legacy", &["--fix", "--json"]);
    let fixed: Value = serde_json::from_slice(&fixed.stdout).unwrap();
    assert!(
        fixed["daemon_task"]["error"]
            .as_str()
            .unwrap()
            .contains(r"Program Files\Crystalline\bin"),
        "{fixed}"
    );
    assert_eq!(fixed["daemon_task"]["old_name"], json!(old));
}

#[test]
fn doctor_reports_a_refused_task_query_as_a_problem() {
    let home = tempfile::tempdir().unwrap();
    let out = doctor(home.path(), "refused", &["--json"]);
    let report: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(
        report["daemon_task"]["finding"],
        json!({ "state": "refused", "name": r"\Crystalline\Daemon" })
    );
    assert_eq!(
        report["daemon_task"]["refused"],
        "the test query was refused"
    );
    assert_eq!(out.status.code(), Some(1));
}
