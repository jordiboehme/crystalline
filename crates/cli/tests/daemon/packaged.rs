//! CLI commands run inside an app package (Claude Desktop's MSIX package on
//! Windows), through the two debug seams: `CRYSTALLINE_TEST_PACKAGE` says
//! "this process is inside an app package" and `CRYSTALLINE_TEST_DAEMON_TASK`
//! stands in for Task Scheduler. Windows sends every new file such a process
//! creates under AppData to the package's private copy, so a packaged
//! command sends its writes to the daemon or refuses (issue 153). No seam
//! can simulate the redirection itself; these tests pin that the client
//! process writes nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

/// A package full name for the seam.
const PACKAGE: &str = "Claude_1.0.0.0_x64__pzs8sxrjxfjjc";

/// Every file under `root` with its length and modification time.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, Option<SystemTime>)> {
    let mut found = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                found.insert(path.clone(), (0, None));
                stack.push(path);
            } else {
                found.insert(path, (meta.len(), meta.modified().ok()));
            }
        }
    }
    found
}

/// A home with a short path: a daemon the `serve` seam starts binds a unix
/// socket under it, and macOS allows 103 bytes for that path.
fn short_home() -> tempfile::TempDir {
    let base = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    tempfile::Builder::new()
        .prefix("cp-")
        .tempdir_in(base)
        .unwrap()
}

/// The config file a child isolated by `isolation_env` reads.
fn config_path(home: &Path) -> PathBuf {
    let dir = if cfg!(windows) {
        home.join("roaming")
    } else {
        home.join("config")
    };
    dir.join("crystalline").join("config.yaml")
}

/// The folders that hold this home's Crystalline state and config: what a
/// packaged command must leave as it found them.
fn state_folders(home: &Path) -> Vec<PathBuf> {
    vec![
        crate::common::isolated_state_dir(home),
        config_path(home).parent().unwrap().to_path_buf(),
        home.join("local"),
    ]
}

fn snapshot_state(home: &Path) -> Vec<BTreeMap<PathBuf, (u64, Option<SystemTime>)>> {
    state_folders(home).iter().map(|d| snapshot(d)).collect()
}

/// `crystalline` isolated in `home`, packaged when `packaged` is set, with
/// the task seam answering `task`.
fn crystalline(home: &Path, packaged: bool, task: &str) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
    for (name, value) in crate::common::isolation_env(home) {
        cmd.env(name, value);
    }
    cmd.env_remove("RUST_LOG")
        .env_remove("CRYSTALLINE_CONFIG")
        .env_remove("CRYSTALLINE_CHANNEL")
        .env_remove("CRYSTALLINE_LAUNCHER_VERSION")
        .env("CRYSTALLINE_SERVICE_HTTP", "false")
        .env("CRYSTALLINE_TEST_DAEMON_TASK", task);
    if packaged {
        cmd.env("CRYSTALLINE_TEST_PACKAGE", PACKAGE);
    } else {
        cmd.env_remove("CRYSTALLINE_TEST_PACKAGE");
    }
    cmd
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_packaged_command_without_a_daemon_writes_no_state() {
    let home = short_home();
    let before = snapshot_state(home.path());
    let out = crystalline(home.path(), true, "missing")
        .args(["config", "set", "github.poll_secs", "120"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("outside Claude Desktop"), "{}", text(&out));
    assert_eq!(
        before,
        snapshot_state(home.path()),
        "the packaged command wrote nothing in the state or config folder"
    );
    assert!(!config_path(home.path()).exists());
}

#[test]
fn a_packaged_stop_hook_still_keeps_its_session_file() {
    let home = short_home();
    let work = tempfile::tempdir().unwrap();
    let config = config_path(home.path());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "domains:\n  test:\n    path: /nonexistent/test\n").unwrap();
    let transcript = work.path().join("t.jsonl");
    std::fs::write(&transcript, "{}\n".repeat(25)).unwrap();
    let payload = serde_json::json!({
        "session_id": "packaged-session",
        "transcript_path": transcript.display().to_string(),
        "hook_event_name": "Stop",
    });
    let mut child = crystalline(home.path(), true, "missing")
        .args(["hook", "stop"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload.to_string().as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let session = crate::common::isolated_state_dir(home.path())
        .join("hooks")
        .join("packaged-session.json");
    assert!(
        session.is_file(),
        "the hooks folder stays writable inside a package: {}",
        text(&out)
    );
}

/// Stop the daemon a `serve` seam started under `home`, if one answers.
fn stop_daemon(home: &Path) {
    let _ = crystalline(home, false, "missing")
        .args(["ctl", "shutdown"])
        .output();
}

/// Whether a daemon answers under `home`.
fn daemon_answers(home: &Path) -> bool {
    crystalline(home, false, "missing")
        .args(["ctl", "status", "--json"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// With no daemon and a task that does not start one, every packaged
/// command refuses, data verbs and reads included: a local engine would
/// create a private index and ownership record. The refusal names why and a
/// terminal outside Claude Desktop, and nothing is written.
#[test]
fn a_packaged_command_refuses_when_the_task_brings_no_daemon() {
    for (task, says) in [("missing", "is missing"), ("fail", "did not start")] {
        for args in [
            &["config", "set", "github.poll_secs", "120"][..],
            &["provision", "allow", "harbor"][..],
            &["search", "anything"][..],
        ] {
            let home = short_home();
            let before = snapshot_state(home.path());
            let out = crystalline(home.path(), true, task)
                .args(args)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(1), "{args:?}: {}", text(&out));
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stderr.contains(says), "{args:?}: {}", text(&out));
            assert!(
                stderr.contains("outside Claude Desktop"),
                "{args:?}: {}",
                text(&out)
            );
            assert_eq!(
                before,
                snapshot_state(home.path()),
                "{args:?} wrote nothing in the state or config folder"
            );
        }
    }
}

/// With the task that starts a daemon, a packaged command reaches it: the
/// setting lands in the config file the daemon writes, while the client
/// itself runs fenced and could not have written it.
#[test]
fn a_packaged_command_runs_the_task_and_writes_through_the_daemon() {
    let home = short_home();
    let out = crystalline(home.path(), true, "serve")
        .args(["config", "set", "github.poll_secs", "120"])
        .output()
        .unwrap();
    let answered = daemon_answers(home.path());
    let search = crystalline(home.path(), true, "serve")
        .args(["search", "anything", "--json"])
        .output()
        .unwrap();
    stop_daemon(home.path());
    assert!(out.status.success(), "{}", text(&out));
    assert!(answered, "the task seam started a daemon");
    let config = std::fs::read_to_string(config_path(home.path())).unwrap();
    assert!(config.contains("poll_secs: 120"), "{config}");
    assert!(search.status.success(), "{}", text(&search));
}

/// `serve` inside a package is refused before anything starts: the daemon
/// must never run with the package identity.
#[test]
fn serve_inside_a_package_is_refused_and_starts_nothing() {
    for args in [&["serve"][..], &["serve", "--daemon"][..]] {
        let home = short_home();
        let before = snapshot_state(home.path());
        let out = crystalline(home.path(), true, "missing")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", text(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("outside Claude Desktop"),
            "{args:?}: {}",
            text(&out)
        );
        assert!(!daemon_answers(home.path()), "{args:?} started no daemon");
        assert_eq!(before, snapshot_state(home.path()), "{args:?}");
    }
}

/// An explicit `--db` outside AppData is not enough to open an index inside
/// a package: the local engine still takes its ownership record in the
/// state folder. So the command refuses rather than opening one.
#[test]
fn a_packaged_command_with_a_db_override_opens_no_local_engine() {
    let home = short_home();
    let outside = tempfile::tempdir().unwrap();
    let db = outside.path().join("index.db");
    let before = snapshot_state(home.path());
    let out = crystalline(home.path(), true, "missing")
        .arg("--db")
        .arg(&db)
        .args(["search", "anything"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("outside Claude Desktop"), "{}", text(&out));
    assert!(!db.exists(), "no index was created");
    assert_eq!(before, snapshot_state(home.path()));
}
