//! Windows integration tests for the daemon: the record split, the named-pipe
//! attach and the second-instance refusal, driven against the real
//! `crystalline` binary. The unix twin lives in service.rs; this file is
//! windows-only because it exercises the named pipe and Windows env isolation
//! (USERPROFILE, APPDATA, LOCALAPPDATA, which etcetera's Windows strategy
//! reads). It compiles to an empty test binary on every other platform, so the
//! coverage rides the windows-latest CI leg without touching local runs.
#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("crystalline")
}

/// An isolated home for one test: USERPROFILE, APPDATA and LOCALAPPDATA all
/// point below one scratch directory, so the state dir (and with it the lock,
/// the record and the derived pipe name) never collides with another test or
/// the developer's real install. On Windows etcetera resolves both the config
/// dir and the state dir to `APPDATA\crystalline`, so config.yaml and
/// service.json share the `roaming/crystalline` directory here.
struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Env {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cq-{tag}-{nanos}"));
        std::fs::create_dir_all(dir.join("roaming")).unwrap();
        std::fs::create_dir_all(dir.join("local")).unwrap();
        // An unresolvable embeddings provider keeps the daemon text-only so a
        // CI run never attempts the model download. The daemon warns and
        // continues; the record is published before the provider build anyway,
        // so the round trip below never waits on embeddings.
        let config_dir = dir.join("roaming/crystalline");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.yaml"),
            "embeddings:\n  provider: disabled-for-tests\n  model: none\n",
        )
        .unwrap();
        Env { dir }
    }

    fn apply(&self, cmd: &mut Command) {
        cmd.env("USERPROFILE", &self.dir)
            .env("APPDATA", self.dir.join("roaming"))
            .env("LOCALAPPDATA", self.dir.join("local"));
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.join("roaming/crystalline")
    }

    fn info_path(&self) -> PathBuf {
        self.state_dir().join("service.json")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Kill-on-drop so a failing assertion never leaks a daemon into the runner.
/// Killing the process releases its exclusive lock: Windows drops the region
/// lock when the owning process dies, so the next test's fresh home is clean.
struct Reap(Child);

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn a foreground `serve --daemon` bound to this env, returning the kill
/// guard. `--daemon` only silences the banner; the process serves in the
/// foreground of this child and acquires ownership itself, so its pid is the
/// pid the record names.
fn spawn_daemon(env: &Env) -> Reap {
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    serve
        .args(["serve", "--daemon"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Reap(serve.spawn().unwrap())
}

/// Poll `service.json` for up to 60s and return the parsed record. The daemon
/// writes it only after the pipe is bound, so a readable record means the pipe
/// is ready to attach. On the pre-split code this never appears on Windows: the
/// mandatory lock made the record unwritable and unreadable through any other
/// handle.
fn wait_for_record(env: &Env) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(text) = std::fs::read_to_string(env.info_path())
            && let Ok(v) = serde_json::from_str::<Value>(&text)
        {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "no readable service.json within 60s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A `crystalline mcp` child driven with newline-delimited JSON-RPC over its
/// stdio, mirroring service.rs's `Mcp` helper (same initialize params, the same
/// id counter and json! request shapes). It attaches over the named pipe with
/// no `--config`: for `mcp` an explicit config is forwarded only to a daemon
/// this call would spawn, so omitting it keeps parity with the `serve`
/// invocation above and still attaches to the running daemon.
///
/// Reads are bounded, unlike service.rs's plain blocking `read_line`: a
/// background thread pumps every stdout line into a channel and `read` waits on
/// it with a deadline, so a wedged pipe handshake fails the test within seconds
/// instead of hanging the windows-latest job. A CI hang is worse than a
/// failure, and this leg is the first to run the pipe path.
struct Bridge {
    // Held only to keep the child alive and kill it on drop; never read.
    _proc: Reap,
    stdin: ChildStdin,
    rx: Receiver<String>,
    id: i64,
}

impl Bridge {
    fn attach(env: &Env) -> Bridge {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        // Detached reader: it ends on its own when the child's stdout closes
        // (the Reap guard kills the child on drop), so nothing needs to join it.
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Bridge {
            _proc: Reap(child),
            stdin,
            rx,
            id: 0,
        }
    }

    fn send(&mut self, value: &Value) {
        self.stdin.write_all(value.to_string().as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The next non-empty JSON-RPC line, or a panic if none arrives before the
    /// deadline so the test fails fast rather than hanging CI.
    fn read(&mut self) -> Value {
        loop {
            match self.rx.recv_timeout(Duration::from_secs(30)) {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    return serde_json::from_str(trimmed).unwrap();
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("no response from the mcp bridge within 30s")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the mcp bridge closed its stdout before answering")
                }
            }
        }
    }

    /// Drive the handshake and return the initialize response.
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

    /// Send `tools/list` and return the raw response value.
    fn list_tools(&mut self) -> Value {
        self.id += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": self.id, "method": "tools/list", "params": {}
        }));
        self.read()
    }
}

/// The record appears while the daemon holds the exclusive lock, names the
/// daemon's real pid and its per-state-dir pipe, and an `mcp` bridge attaches
/// over that pipe end to end. On the pre-split code this fails in the first
/// poll: the mandatory lock made the record unreadable on Windows.
#[test]
fn daemon_publishes_a_readable_record_and_serves_mcp_over_the_pipe() {
    let env = Env::new("win-daemon");
    let daemon = spawn_daemon(&env);

    let record = wait_for_record(&env);
    assert_eq!(
        record["pid"].as_u64().unwrap() as u32,
        daemon.0.id(),
        "the record names the live daemon pid: {record}"
    );
    let pipe = record["socket_path"].as_str().unwrap();
    assert!(
        pipe.starts_with(r"\\.\pipe\crystalline-"),
        "per-state-dir pipe name, got {pipe}"
    );

    let mut bridge = Bridge::attach(&env);
    let init = bridge.initialize();
    assert!(
        init.get("result").is_some() && init.pointer("/result/serverInfo").is_some(),
        "initialize returns a server result over the pipe: {init}"
    );

    let tools = bridge.list_tools();
    let names: Vec<String> = tools
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.iter().any(|n| n == "search_engrams"),
        "tools/list served over the pipe includes search_engrams: {tools}"
    );

    drop(bridge);
    drop(daemon);
}

/// A second serve must fail fast and name the live owner's real pid: before the
/// split it could not even read who owned the lock, so it reported pid 0.
#[test]
fn a_second_serve_fails_fast_naming_the_owner() {
    let env = Env::new("win-second");
    let daemon = spawn_daemon(&env);
    let record = wait_for_record(&env);
    let owner_pid = record["pid"].as_u64().unwrap();

    let mut second = Command::new(bin());
    env.apply(&mut second);
    let out = second
        .args(["serve"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the second serve exits nonzero while the owner holds the lock"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("another Crystalline instance owns the index")
            && stderr.contains(&owner_pid.to_string()),
        "the refusal names the live owner (pid {owner_pid}): {stderr}"
    );

    drop(daemon);
}
