//! Binary integration tests for the degraded status server: the real
//! `crystalline` binary serving the stub over stdio when it cannot start.
//!
//! Each test holds an exclusive `fs4` lock on `service.lock` in the test
//! process itself - the very lock a rival daemon would hold - so the spawned
//! `crystalline mcp` child fails to acquire the index and falls through to the
//! degraded stub. A `service.json` owner record (pid = this alive test process)
//! drives the case selection: a strictly newer version reads as an upgrade
//! skew, this binary's own version as a plain conflict, an unreadable record as
//! generic. Unix-only: an exclusive advisory lock across two processes and a
//! short `/tmp` HOME are the mechanism.
#![cfg(unix)]

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs4::FileExt;
use serde_json::{Value, json};

fn bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("crystalline")
}

/// An isolated, short-path environment holding the index lock for one test.
struct Env {
    dir: PathBuf,
    /// The exclusively locked `service.lock` handle, held for the test's whole
    /// life so the spawned child can never acquire the index.
    _lock: File,
}

impl Env {
    /// Create the isolated dirs, then take and hold the exclusive index lock.
    fn locked(tag: &str) -> Env {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // A short base keeps any derived path well under the macOS limits and
        // matches the daemon integration tests' `Env`.
        let dir = PathBuf::from("/tmp").join(format!("cq-{tag}-{nanos}"));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        let state = dir.join("state/crystalline");
        std::fs::create_dir_all(&state).unwrap();

        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.join("service.lock"))
            .unwrap();
        FileExt::try_lock(&lock).expect("the test holds the index lock");
        Env { dir, _lock: lock }
    }

    fn apply(&self, cmd: &mut Command) {
        cmd.env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_CACHE_HOME", self.dir.join("cache"));
    }

    fn info_path(&self) -> PathBuf {
        self.dir.join("state/crystalline/service.json")
    }

    /// Write an owner record naming this alive test process, so the child's
    /// case selection sees a live daemon at `version`.
    fn write_record(&self, version: &str) {
        let record = json!({
            "pid": std::process::id(),
            "socket_path": "/nonexistent",
            "version": version,
            "started_at": "2026-07-16T00:00:00Z",
        });
        std::fs::write(self.info_path(), record.to_string()).unwrap();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A `crystalline mcp` child driven with raw newline-delimited JSON-RPC.
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
    id: i64,
}

impl Mcp {
    /// Spawn `crystalline mcp` (with `--embedded` unless `daemon_path` asks for
    /// the daemon-first path) under `env` with the mcpb channel marker set.
    fn spawn(env: &Env, embedded: bool, channel: Option<&str>) -> Mcp {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.arg("mcp");
        if embedded {
            cmd.arg("--embedded");
        }
        if let Some(channel) = channel {
            cmd.env("CRYSTALLINE_CHANNEL", channel);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Mcp {
            child,
            stdin: Some(stdin),
            out,
            id: 0,
        }
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(value.to_string().as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        loop {
            let mut line = String::new();
            let n = self.out.read_line(&mut line).unwrap();
            assert!(n > 0, "unexpected EOF from mcp child");
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return serde_json::from_str(trimmed).unwrap();
        }
    }

    /// Drive `initialize`, send the initialized notification and return the
    /// initialize response.
    fn initialize(&mut self) -> Value {
        self.id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": self.id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "it", "version": "0" }
            }
        }));
        let resp = self.read();
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        resp
    }

    /// Send `tools/list` and return the tool names.
    fn list_tools(&mut self) -> Vec<String> {
        self.id += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": self.id, "method": "tools/list", "params": {}
        }));
        self.read()
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Call `tool` and return the parsed JSON payload from its text block.
    fn call(&mut self, tool: &str) -> Value {
        self.id += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": self.id, "method": "tools/call",
            "params": { "name": tool, "arguments": {} }
        }));
        let resp = self.read();
        let text = resp
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        serde_json::from_str(text).unwrap_or(Value::Null)
    }

    /// Close stdin and wait for the child to exit, returning its exit code.
    fn close_and_wait(&mut self) -> i32 {
        self.stdin = None;
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            if start.elapsed() > Duration::from_secs(10) {
                panic!("mcp child did not exit after stdin close");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A strictly newer daemon owns the index and the mcpb channel is set: the
/// stub serves the extension-upgrade copy, exposes only `status` and its
/// payload names the newer daemon, then a clean exit on stdin close.
#[test]
fn embedded_newer_daemon_on_mcpb_channel_serves_the_upgrade_stub() {
    let env = Env::locked("stub-mcpb");
    env.write_record("99.0.0");

    let mut mcp = Mcp::spawn(&env, true, Some("mcpb"));
    let init = mcp.initialize();
    assert_eq!(
        init.pointer("/result/serverInfo/name")
            .and_then(Value::as_str),
        Some("crystalline"),
        "a RESULT, not an error: {init}"
    );
    let instructions = init
        .pointer("/result/instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        instructions.contains("https://github.com/jordiboehme/crystalline/releases"),
        "instructions carry the releases URL:\n{instructions}"
    );

    assert_eq!(
        mcp.list_tools(),
        vec!["status".to_string()],
        "only the status tool"
    );

    let payload = mcp.call("status");
    assert_eq!(payload["available"], json!(false));
    assert_eq!(payload["daemon_version"], json!("99.0.0"));
    assert_eq!(payload["channel"], json!("mcpb"));

    assert_eq!(mcp.close_and_wait(), 0, "a degraded session ends cleanly");
}

/// A live record at this binary's own version is a plain conflict: the copy
/// names the pid and offers no update hint.
#[test]
fn embedded_same_version_record_serves_the_conflict_stub() {
    let env = Env::locked("stub-conflict");
    env.write_record(env!("CARGO_PKG_VERSION"));

    let mut mcp = Mcp::spawn(&env, true, Some("mcpb"));
    let init = mcp.initialize();
    let instructions = init
        .pointer("/result/instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        instructions.contains("owns this machine's knowledge index"),
        "plain conflict copy:\n{instructions}"
    );
    assert!(
        !instructions.contains("install it over the current")
            && !instructions.contains("update this Crystalline installation"),
        "an equal-version record is never an upgrade skew:\n{instructions}"
    );

    let payload = mcp.call("status");
    assert_eq!(payload["available"], json!(false));
    assert_eq!(payload["daemon_version"], json!(env!("CARGO_PKG_VERSION")));

    assert_eq!(mcp.close_and_wait(), 0);
}

/// No readable owner record (the lock is held but `service.json` is absent):
/// the generic copy surfaces the raw startup reason and points at daemon.log.
#[test]
fn embedded_no_record_serves_the_generic_stub() {
    let env = Env::locked("stub-generic");
    // Deliberately no service.json: read_lock_info finds nothing to explain the
    // failure, so the stub degrades to the generic reason-carrying copy.

    let mut mcp = Mcp::spawn(&env, true, Some("mcpb"));
    let init = mcp.initialize();
    let instructions = init
        .pointer("/result/instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        instructions.contains("cannot run an embedded MCP server"),
        "generic copy carries the reason:\n{instructions}"
    );
    assert!(
        instructions.contains("daemon.log"),
        "generic copy points at daemon.log:\n{instructions}"
    );

    let payload = mcp.call("status");
    assert_eq!(payload["available"], json!(false));
    assert!(
        !payload.as_object().unwrap().contains_key("daemon_version"),
        "no live record: {payload}"
    );

    assert_eq!(mcp.close_and_wait(), 0);
}

/// The field incident itself: `crystalline mcp` WITHOUT `--embedded` against a
/// held lock. It first spends the daemon spawn-and-readiness budget (~15s)
/// before falling through to the embedded path and serving the stub, so this
/// is ignored by default to keep the suite fast. Run it explicitly with:
/// `cargo nextest run -p crystalline --run-ignored all -E 'binary(mcp_stub)'`
/// (nextest's `test()` predicate matches test names, so the mcp_stub file is
/// selected by `binary()`).
#[test]
#[ignore = "burns the ~15s daemon spawn budget before the stub serves"]
fn daemon_path_falls_through_to_the_stub_after_the_spawn_budget() {
    let env = Env::locked("stub-daemon");
    env.write_record("99.0.0");

    let mut mcp = Mcp::spawn(&env, false, Some("mcpb"));
    let init = mcp.initialize();
    assert_eq!(
        init.pointer("/result/serverInfo/name")
            .and_then(Value::as_str),
        Some("crystalline"),
        "the stub serves after the daemon path gives up: {init}"
    );
    assert_eq!(mcp.list_tools(), vec!["status".to_string()]);
    assert_eq!(mcp.close_and_wait(), 0);
}
