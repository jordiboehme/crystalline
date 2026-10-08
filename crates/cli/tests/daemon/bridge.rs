//! The real `crystalline mcp` as a packaged bridge, through the two debug
//! seams: `CRYSTALLINE_TEST_PACKAGE` says "this process is inside an app
//! package" and `CRYSTALLINE_TEST_DAEMON_TASK` stands in for Task Scheduler.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

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

struct Bridge {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Bridge {
    fn start(home: &Path, task: &str) -> Bridge {
        Bridge::start_with(home, task, &[])
    }

    /// As [`Bridge::start`], with `extra` set last, so it wins.
    fn start_with(home: &Path, task: &str, extra: &[(&str, &str)]) -> Bridge {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
        for (name, value) in crate::common::isolation_env(home) {
            cmd.env(name, value);
        }
        cmd.env_remove("RUST_LOG")
            .env("CRYSTALLINE_SERVICE_HTTP", "false")
            .env(
                "CRYSTALLINE_TEST_PACKAGE",
                "Claude_1.0.0.0_x64__pzs8sxrjxfjjc",
            )
            .env("CRYSTALLINE_TEST_DAEMON_TASK", task)
            .env("CRYSTALLINE_CHANNEL", "desktop");
        for (name, value) in extra {
            cmd.env(name, value);
        }
        cmd.arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Bridge { child, out }
    }

    fn ask(&mut self, request: Value) -> Value {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"))
    }

    /// Open the session the way `mcp_stub.rs` `Mcp::initialize` does:
    /// `initialize`, then the initialized notification, which gets no answer.
    fn open(&mut self) -> Value {
        let opened = self.ask(initialize());
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .unwrap();
        stdin.flush().unwrap();
        opened
    }

    fn finish(mut self) {
        drop(self.child.stdin.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("the bridge did not exit after stdin closed");
    }
}

/// A home with a short path: the daemon the `serve` seam starts binds a unix
/// socket under it, and macOS allows 103 bytes for that path, which a
/// default temp folder nearly spends on its own (`mcp_stub.rs` uses `/tmp`
/// for the same reason).
fn short_home() -> tempfile::TempDir {
    let base = if cfg!(unix) {
        std::path::PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    tempfile::Builder::new()
        .prefix("cq-")
        .tempdir_in(base)
        .unwrap()
}

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": { "name": "claude-ai", "version": "1" } } })
}

fn status_payload(bridge: &mut Bridge) -> Value {
    let called = bridge.ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "status", "arguments": {} } }));
    let text = called["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn a_packaged_bridge_with_no_task_names_it_and_writes_nothing() {
    let home = short_home();
    let before = snapshot(home.path());
    let mut bridge = Bridge::start(home.path(), "missing");
    let opened = bridge.open();
    let instructions = opened["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains(r"\Crystalline\Daemon"),
        "{instructions}"
    );
    assert!(instructions.contains("is missing"), "{instructions}");
    assert_eq!(status_payload(&mut bridge)["bridge"], "task_missing");
    bridge.finish();
    assert_eq!(
        before,
        snapshot(home.path()),
        "the bridge wrote nothing anywhere under the home"
    );
}

#[test]
fn a_packaged_bridge_whose_task_fails_says_it_did_not_start() {
    let home = short_home();
    let mut bridge = Bridge::start(home.path(), "fail");
    let opened = bridge.open();
    assert!(
        opened["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("did not start"),
        "{opened}"
    );
    assert_eq!(status_payload(&mut bridge)["bridge"], "task_did_not_start");
    bridge.finish();
}

#[test]
fn a_packaged_bridge_whose_task_query_is_refused_says_why() {
    let home = short_home();
    let mut bridge = Bridge::start(home.path(), "refused");
    let opened = bridge.open();
    let instructions = opened["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("did not start"), "{instructions}");
    assert!(
        instructions.contains("the test query was refused"),
        "{instructions}"
    );
    assert_eq!(status_payload(&mut bridge)["bridge"], "task_did_not_start");
    bridge.finish();
}

#[test]
fn a_packaged_bridge_runs_the_task_and_serves_the_daemon() {
    let home = short_home();
    let mut bridge = Bridge::start(home.path(), "serve");
    let opened = bridge.open();
    assert_eq!(
        opened["result"]["serverInfo"]["name"], "crystalline",
        "{opened}"
    );
    let tools = bridge.ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"search_engrams"),
        "the daemon's tools, not the stub: {names:?}"
    );
    bridge.finish();
    // The daemon came from the task, not from the bridge. Only the bridge's
    // own spawn (`spawn_daemon`) opens daemon.log for the child it starts;
    // the task seam starts the daemon with its output discarded, as Task
    // Scheduler does. So a daemon.log here means the bridge spawned it.
    let log = crate::common::isolated_state_dir(home.path()).join("daemon.log");
    let spawned_by_bridge = log.exists();
    let mut stop = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
    for (name, value) in crate::common::isolation_env(home.path()) {
        stop.env(name, value);
    }
    let _ = stop.args(["ctl", "shutdown"]).output();
    assert!(
        !spawned_by_bridge,
        "the bridge spawned the daemon itself ({} exists)",
        log.display()
    );
}

/// What `serve --from-task` changes for a daemon Task Scheduler started: it
/// works in the state folder, wherever it was started, and logs to
/// daemon.log, because nobody reads its stderr.
#[test]
fn a_task_started_daemon_works_in_its_state_folder_and_logs_there() {
    let home = short_home();
    let elsewhere = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
        for (name, value) in crate::common::isolation_env(home.path()) {
            cmd.env(name, value);
        }
        cmd.env_remove("RUST_LOG")
            .env("CRYSTALLINE_SERVICE_HTTP", "false")
            .current_dir(elsewhere.path())
            .args(args);
        cmd
    };
    let mut daemon = run(&["serve", "--from-task"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let state = crate::common::isolated_state_dir(home.path());
    let mut status = None;
    for _ in 0..100 {
        let out = run(&["ctl", "status", "--json"]).output().unwrap();
        if out.status.success() {
            status = Some(serde_json::from_slice::<Value>(&out.stdout).unwrap());
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let pid = daemon.id();
    if status.is_some() {
        let _ = run(&["ctl", "shutdown"]).output();
    } else {
        let _ = daemon.kill();
    }
    let _ = daemon.wait();
    let status = status.expect("the task-started daemon answers");
    assert_eq!(
        status["pid"],
        json!(pid),
        "the daemon the task started answers: {status}"
    );
    assert_eq!(
        status["started_by"],
        json!("autostart"),
        "recorded as an autostart, with no new start mode: {status}"
    );
    // Both sides canonical: macOS resolves /tmp to /private/tmp, and Windows
    // answers a canonical path with the \\?\ prefix.
    assert_eq!(
        std::fs::canonicalize(status["runs_in"]["working_dir"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&state).unwrap(),
        "it works in the state folder, not where it was started"
    );
    let log = std::fs::read_to_string(state.join("daemon.log")).unwrap();
    assert!(!log.is_empty(), "it logs to daemon.log");
}

/// The config file a child isolated by `isolation_env` reads.
fn isolated_config_path(home: &Path) -> PathBuf {
    let dir = if cfg!(windows) {
        home.join("roaming")
    } else {
        home.join("config")
    };
    dir.join("crystalline").join("config.yaml")
}

/// A daemon the sign-in task starts reads the system environment: the
/// `system` seam starts it without this shell's shaping variables. `status`
/// from a shell that sets one names it; a shell that sets none says nothing.
#[test]
fn status_names_the_variable_a_task_started_daemon_did_not_get() {
    let home = short_home();
    // HTTP off in the file: the seam strips the variable, and a daemon on
    // the default port would race every other test for 7411.
    let config = isolated_config_path(home.path());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "service:\n  http: false\n").unwrap();
    let mut bridge = Bridge::start_with(
        home.path(),
        "system",
        &[("CRYSTALLINE_SERVICE_HTTP", "127.0.0.1:7499")],
    );
    let opened = bridge.open();
    assert_eq!(
        opened["result"]["serverInfo"]["name"], "crystalline",
        "{opened}"
    );
    let run = |extra: &[(&str, &str)], args: &[&str]| {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
        for (name, value) in crate::common::isolation_env(home.path()) {
            cmd.env(name, value);
        }
        cmd.env_remove("RUST_LOG")
            .env_remove("CRYSTALLINE_SERVICE_HTTP");
        // A shaping variable the runner set would explain a difference too.
        for name in crystalline_service::shaping::shaping_set_here() {
            cmd.env_remove(name);
        }
        for (name, value) in extra {
            cmd.env(name, value);
        }
        cmd.args(args).output().unwrap()
    };
    let shell = [("CRYSTALLINE_SERVICE_HTTP", "127.0.0.1:7499")];
    let with_variable = run(&shell, &["status"]);
    let as_json = run(&shell, &["status", "--json"]);
    let without = run(&[], &["status"]);
    bridge.finish();
    let _ = run(&[], &["ctl", "shutdown"]);

    let said = String::from_utf8_lossy(&with_variable.stderr);
    assert!(
        said.contains("note: The daemon serves no HTTP endpoint, not 127.0.0.1:7499 from CRYSTALLINE_SERVICE_HTTP: a daemon the sign-in task started reads the system environment. Set CRYSTALLINE_SERVICE_HTTP as a user environment variable, or stop the daemon (`crystalline ctl shutdown`) and start it from this shell."),
        "{said}"
    );
    let json: Value = serde_json::from_slice(&as_json.stdout).unwrap();
    assert_eq!(
        json["config_mismatch"][0]["variable"], "CRYSTALLINE_SERVICE_HTTP",
        "{json}"
    );
    assert!(
        !String::from_utf8_lossy(&without.stderr).contains("The daemon serves"),
        "nothing set here, nothing said"
    );
}
