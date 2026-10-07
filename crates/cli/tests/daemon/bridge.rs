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
            .env("CRYSTALLINE_CHANNEL", "desktop")
            .arg("mcp")
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
    let mut stop = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
    for (name, value) in crate::common::isolation_env(home.path()) {
        stop.env(name, value);
    }
    let _ = stop.args(["ctl", "shutdown"]).output();
}
