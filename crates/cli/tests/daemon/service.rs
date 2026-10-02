//! Integration tests for the daemon, spawning the real `crystalline` binary.
//!
//! Each test runs in an isolated, deliberately short temp HOME (so the unix
//! socket path stays under the platform limit). etcetera uses the XDG strategy
//! on Linux and macOS, so setting the XDG_*_HOME variables redirects the state,
//! config and cache directories. These tests are unix-only: they use unix domain
//! sockets and `kill -9` for the stale-lock scenario.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crystalline_core::config::{self, GlobalConfig, ResponseFormat, ServiceConfig};
use serde_json::{Value, json};

fn bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("crystalline")
}

/// An isolated, short-path environment for one test.
struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Env {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // A short base keeps the macOS unix-socket path within 104 bytes.
        let dir = PathBuf::from("/tmp").join(format!("cq-{tag}-{nanos}"));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        Env { dir }
    }

    /// Isolate a child (and, through inheritance, any daemon it spawns) into
    /// this test's directories, with the HTTP endpoint turned off.
    ///
    /// The endpoint is on at `127.0.0.1:7411` by default, and a port is the one
    /// thing a temp directory cannot isolate: several of these tests run daemons
    /// concurrently under nextest, and they would race each other and the
    /// developer's own daemon for that single real port. `CRYSTALLINE_SERVICE_HTTP=false`
    /// makes the hermeticity explicit rather than lucky. The one test that wants
    /// an endpoint passes `--http <free port>`, and the flag wins over this
    /// variable.
    fn apply(&self, cmd: &mut Command) {
        cmd.env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_CACHE_HOME", self.dir.join("cache"))
            .env("CRYSTALLINE_SERVICE_HTTP", "false")
            // A real daemon spawned here runs its background origin poller
            // unconditionally; none of these tests turn `github.enabled` on
            // for an origin-connected domain today, but the OS keychain
            // service name is a hardcoded constant none of the four
            // variables above reach, so a daemon that ever did would ask the
            // real login keychain from its own timer, not just from a
            // one-shot CLI call. This is the boolean kill switch
            // (`crystalline_remote::token::refuse_real_keychain`), not
            // `CRYSTALLINE_TEST_TOKEN_STORE_DIR`: it falls back to the file
            // store under this daemon's own isolated state dir rather than
            // a directory of its own.
            .env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1");
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.join("state/crystalline")
    }
    fn lock_path(&self) -> PathBuf {
        self.state_dir().join("service.lock")
    }
    fn info_path(&self) -> PathBuf {
        self.state_dir().join("service.json")
    }
    fn sock_path(&self) -> PathBuf {
        self.state_dir().join("service.sock")
    }
    fn config_path(&self) -> PathBuf {
        self.dir.join("config/crystalline/config.yaml")
    }

    /// Create a domain directory with a MANIFEST and a seed engram, then register
    /// it in the config, pinning the wire format to json. Config selection rides
    /// on the isolated `XDG_CONFIG_HOME` (`apply` below), never an explicit
    /// `--config`: the default config path already resolves to `config_path()`,
    /// and an explicit override would mean "bypass the daemon" (see
    /// `crystalline_service::use_daemon`), which would wrongly force the direct
    /// index path and collide with a running daemon's lock. Leaving it off lets
    /// `domain add` route its sync through a live daemon exactly as a plain
    /// invocation does.
    fn setup_domain(&self, name: &str) {
        self.setup_domain_inner(name, true);
    }

    /// Like [`Self::setup_domain`] but leaves `service.response_format` on its
    /// TOON default instead of pinning json, so a data command exercises the
    /// daemon seam under the default wire format. Used by the one test that
    /// proves the CLI receives structured JSON regardless of that format.
    fn setup_domain_toon(&self, name: &str) {
        self.setup_domain_inner(name, false);
    }

    fn setup_domain_inner(&self, name: &str, pin_json: bool) {
        // The pin (when requested) keeps the suite's assertions on data
        // semantics; the TOON default gets its dedicated tests in
        // crates/service/tests. Written once, before the first `domain add`
        // creates config.yaml: `domain add` only ever loads the existing file
        // and rewrites its `domains` map, so this rides along untouched through
        // every later `setup_domain` call in the same `Env`.
        if !self.config_path().exists() {
            std::fs::create_dir_all(self.config_path().parent().unwrap()).unwrap();
            let cfg = if pin_json {
                GlobalConfig {
                    service: Some(ServiceConfig {
                        response_format: Some(ResponseFormat::Json),
                        ..ServiceConfig::default()
                    }),
                    ..GlobalConfig::default()
                }
            } else {
                GlobalConfig::default()
            };
            config::save_yaml(&self.config_path(), &cfg).unwrap();
        }

        let dir = self.dir.join(format!("kb-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            format!(
                "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- {name}\n\n## When to Use\n\n- Route here for {name}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("seed.md"),
            "---\ntype: engram\ntitle: Seed\npermalink: seed\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nseed body token\n",
        )
        .unwrap();
        let mut cmd = Command::new(bin());
        self.apply(&mut cmd);
        let ok = cmd
            .args(["domain", "add", name])
            .arg(&dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "domain add");
    }

    /// Run a one-shot command, returning (success, stdout).
    fn run(&self, args: &[&str]) -> (bool, String) {
        let mut cmd = Command::new(bin());
        self.apply(&mut cmd);
        let out = cmd.args(args).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy_owned(out.stdout),
        )
    }

    /// Like [`Self::run`], but also returns stderr - needed to see a command's
    /// failure message, which never lands on stdout.
    fn run_full(&self, args: &[&str]) -> (bool, String, String) {
        let mut cmd = Command::new(bin());
        self.apply(&mut cmd);
        let out = cmd.args(args).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy_owned(out.stdout),
            String::from_utf8_lossy_owned(out.stderr),
        )
    }

    /// Poll ctl status until the daemon answers, or panic after ~8s.
    fn wait_ready(&self) {
        let start = Instant::now();
        loop {
            let (ok, _) = self.run(&["ctl", "status", "--json"]);
            if ok {
                return;
            }
            if start.elapsed() > Duration::from_secs(8) {
                panic!("daemon did not become ready");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Wait until the daemon's pass after its initial sync has written
    /// `config.yaml`, or panic after ~8s.
    ///
    /// Readiness comes first and the initial sync runs behind it, so a daemon
    /// that answers `ctl status` may still be about to line the domain names
    /// up and record `canonical_seen` for every registered domain. A test that
    /// snapshots the file, or edits it, right after [`Self::wait_ready`] races
    /// that write; it waits here instead. Only for a configuration whose
    /// domains the pass records, which is every domain `setup_domain`
    /// registers.
    fn wait_names_recorded(&self) {
        let start = Instant::now();
        loop {
            let recorded =
                config::load_yaml::<GlobalConfig>(&self.config_path()).is_ok_and(|cfg| {
                    !cfg.domains.is_empty()
                        && cfg.domains.values().all(|e| e.canonical_seen.is_some())
                });
            if recorded {
                return;
            }
            if start.elapsed() > Duration::from_secs(8) {
                panic!(
                    "the daemon never recorded the domain names: {}",
                    std::fs::read_to_string(self.config_path()).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The pid from the owner record. Reads `service.json`, never the lock
    /// file itself: the record moved there so a reader never has to touch a
    /// handle another process holds an exclusive lock on.
    fn lock_pid(&self) -> Option<u64> {
        let text = std::fs::read_to_string(self.info_path()).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        v.get("pid").and_then(Value::as_u64)
    }

    /// The last `lines` lines of the daemon log a spawned daemon writes under
    /// this environment's state directory, for a failure message: a daemon a
    /// bridge started has no other place a test can read what it did.
    fn daemon_log_tail(&self, lines: usize) -> String {
        let path = self.state_dir().join("daemon.log");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let all: Vec<&str> = text.lines().collect();
                all[all.len().saturating_sub(lines)..].join("\n")
            }
            Err(e) => format!("({} could not be read: {e})", path.display()),
        }
    }

    /// The whole owner record, for the tests that assert on more than the pid.
    fn lock_record(&self) -> Option<Value> {
        let text = std::fs::read_to_string(self.info_path()).ok()?;
        serde_json::from_str(&text).ok()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Best-effort: stop any daemon this test left running, then remove the dir.
        if let Some(pid) = self.lock_pid() {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A `crystalline mcp` child driven with raw newline-delimited JSON-RPC.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    id: i64,
}

impl Mcp {
    fn spawn(env: &Env) -> Mcp {
        Mcp::spawn_inner(env, false)
    }

    /// Spawn an `mcp` client that starts a read-only daemon when none is running.
    fn spawn_read_only(env: &Env) -> Mcp {
        Mcp::spawn_inner(env, true)
    }

    /// Spawn an `mcp` client with `CRYSTALLINE_SERVICE_READ_ONLY=true` in the
    /// environment and no `--read-only` flag, so the daemon it starts derives
    /// read-only mode from the environment overlay. The spawned daemon inherits
    /// the parent's environment, so the variable reaches `serve` without being
    /// passed as a flag.
    fn spawn_env_read_only(env: &Env) -> Mcp {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.env("CRYSTALLINE_SERVICE_READ_ONLY", "true");
        cmd.arg("mcp");
        cmd.arg("--config").arg(env.config_path());
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
            stdin,
            out,
            id: 0,
        }
    }

    /// Spawn `mcp --embedded`: the whole stack in this process, no daemon.
    fn spawn_embedded(env: &Env) -> Mcp {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.args(["mcp", "--embedded"]);
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
            stdin,
            out,
            id: 0,
        }
    }

    /// Spawn an `mcp` daemon that owns a config and index other than the
    /// environment's own defaults, `--db` given ahead of the subcommand the
    /// way the global flag is placed, `--config` after it. Used to prove a
    /// bypassing command reads the untouched default index rather than
    /// colliding with a daemon that never held it in the first place.
    fn spawn_with_db(env: &Env, config: &Path, db: &Path) -> Mcp {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.arg("--db").arg(db);
        cmd.arg("mcp");
        cmd.arg("--config").arg(config);
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
            stdin,
            out,
            id: 0,
        }
    }

    /// Spawn an `mcp` client whose working directory is `dir`, with an
    /// optional `--db` ahead of the subcommand, passed exactly as written: a
    /// relative one stays relative, which is the point of the tests that use
    /// this.
    fn spawn_in(env: &Env, dir: &Path, db: Option<&str>) -> Mcp {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.current_dir(dir);
        if let Some(db) = db {
            cmd.arg("--db").arg(db);
        }
        cmd.arg("mcp");
        cmd.arg("--config").arg(env.config_path());
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
            stdin,
            out,
            id: 0,
        }
    }

    /// Spawn an `mcp` client, and with it the daemon it starts, carrying one
    /// extra environment variable.
    ///
    /// The daemon inherits this process's environment, so a knob the *daemon*
    /// has to see has to be set here, where it is started, and not on the `ctl`
    /// client that later asks it to do the work. The only user is the rebuild
    /// hold, below.
    fn spawn_with_env(env: &Env, key: &str, value: &str) -> Mcp {
        Mcp::spawn_configured(env, false, Some((key, value)))
    }

    fn spawn_inner(env: &Env, read_only: bool) -> Mcp {
        Mcp::spawn_configured(env, read_only, None)
    }

    fn spawn_configured(env: &Env, read_only: bool, extra: Option<(&str, &str)>) -> Mcp {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        if let Some((key, value)) = extra {
            cmd.env(key, value);
        }
        cmd.arg("mcp");
        if read_only {
            cmd.arg("--read-only");
        }
        cmd.arg("--config").arg(env.config_path());
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
            stdin,
            out,
            id: 0,
        }
    }

    /// Send `tools/list` and return the tool names.
    fn list_tools(&mut self) -> Vec<String> {
        self.id += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": self.id, "method": "tools/list", "params": {}
        }));
        let resp = self.read();
        resp.pointer("/result/tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn send(&mut self, value: &Value) {
        self.stdin.write_all(value.to_string().as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
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

    fn initialize(&mut self) {
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
        assert!(resp.get("result").is_some(), "initialize: {resp}");
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    }

    fn send_call(&mut self, tool: &str, args: Value) {
        self.id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": self.id,
            "method": "tools/call",
            "params": { "name": tool, "arguments": args }
        }));
    }

    fn read_tool_value(&mut self) -> Value {
        let resp = self.read();
        let text = resp
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        serde_json::from_str(text).unwrap_or(Value::Null)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn single_daemon_two_clients_and_stale_recovery() {
    let env = Env::new("share");
    env.setup_domain("eng");

    // The first client spawns the daemon and attaches.
    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    // A second client attaches to the same daemon.
    let mut c2 = Mcp::spawn(&env);
    c2.initialize();
    std::thread::sleep(Duration::from_millis(300));

    // ctl status shows two sessions and the pid matches the lock file.
    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "ctl status");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        status["sessions"],
        json!(2),
        "two shared sessions: {status}"
    );
    let pid = status["pid"].as_u64().unwrap();
    assert_eq!(env.lock_pid(), Some(pid), "lock pid equals daemon pid");

    // Both clients search concurrently over the one daemon.
    c1.send_call("search_engrams", json!({ "query": "token" }));
    c2.send_call("search_engrams", json!({ "query": "token" }));
    let r1 = c1.read_tool_value();
    let r2 = c2.read_tool_value();
    assert!(r1["total"].as_u64().unwrap() >= 1, "c1 search: {r1}");
    assert!(r2["total"].as_u64().unwrap() >= 1, "c2 search: {r2}");

    // Hard-kill the daemon; the next client recovers via stale-lock takeover.
    drop(c1);
    drop(c2);
    Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .status()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));

    let mut c3 = Mcp::spawn(&env);
    c3.initialize();
    env.wait_ready();
    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok);
    let status: Value = serde_json::from_str(&out).unwrap();
    let pid2 = status["pid"].as_u64().unwrap();
    assert_ne!(pid2, pid, "a fresh daemon took over the stale lock");

    // ctl shutdown stops the daemon and removes the lock and socket.
    drop(c3);
    let (ok, _) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown");
    // Shutdown cleanup is asynchronous and the daemon also releases its
    // domain host locks on the way down, so the socket and the lock file
    // disappear at slightly different moments; give both the same deadline.
    let start = Instant::now();
    while (env.sock_path().exists() || env.lock_path().exists())
        && start.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!env.sock_path().exists(), "socket removed on shutdown");
    assert!(!env.lock_path().exists(), "lock removed on shutdown");
}

/// The Claude Desktop extension's daemon: started attached with a bounded
/// life, it reports that life, keeps serving a client that returns inside the
/// grace (Desktop restarting its server) and leaves on its own once the last
/// client is gone, taking its lock and record with it.
#[test]
fn an_extension_started_daemon_leaves_once_its_last_client_is_gone() {
    let env = Env::new("idle");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn_with_env(&env, "CRYSTALLINE_CHANNEL", "mcpb");
    c1.initialize();
    env.wait_ready();
    let status = status_json(&env);
    assert_eq!(status["idle_exit_secs"], json!(5), "bounded life: {status}");
    assert_eq!(status["started_by"], json!("autostart"), "{status}");
    let pid = status["pid"].as_u64().unwrap();
    let (ok, out) = env.run(&["status"]);
    assert!(ok, "status: {out}");
    assert!(
        out.contains("Lifetime: exits 5s after its last client disconnects"),
        "the bounded life renders: {out}"
    );

    // The client is killed outright (`Mcp::drop` is `child.kill()`, SIGKILL on
    // unix: the abrupt end Desktop's teardown is, not a polite stdin close)
    // and another comes back inside the grace: same daemon.
    drop(c1);
    std::thread::sleep(Duration::from_secs(2));
    let mut c2 = Mcp::spawn(&env);
    c2.initialize();
    std::thread::sleep(Duration::from_secs(6));
    let status = status_json(&env);
    assert_eq!(
        status["pid"].as_u64(),
        Some(pid),
        "the returning client kept the daemon: {status}\ndaemon.log tail:\n{}",
        env.daemon_log_tail(60)
    );
    assert_eq!(status["sessions"], json!(1), "{status}");

    // The last client leaves: gone within the grace plus its own shutdown.
    // A failure here carries the daemon's own account of it - whether the
    // idle exit fired at all and, if it did, which shutdown step it stopped
    // in - because this is the test that has failed on macOS runners with
    // nothing else to go on.
    drop(c2);
    let start = Instant::now();
    while (env.lock_path().exists() || env.info_path().exists())
        && start.elapsed() < Duration::from_secs(15)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !env.lock_path().exists() && !env.info_path().exists(),
        "the daemon left on its own after {:?}\ndaemon.log tail:\n{}",
        start.elapsed(),
        env.daemon_log_tail(60)
    );
}

/// A stub counts as a client from the moment it attaches, not from its
/// client's first request: a Claude Desktop stub whose client stays silent
/// past the grace must not lose its daemon under it.
#[test]
fn a_silent_stub_holds_an_extension_started_daemon_past_the_grace() {
    let env = Env::new("silent");
    env.setup_domain("eng");

    // Spawned, never spoken to: no initialize for longer than the grace.
    let mut c1 = Mcp::spawn_with_env(&env, "CRYSTALLINE_CHANNEL", "mcpb");
    env.wait_ready();
    std::thread::sleep(Duration::from_secs(7));
    let status = status_json(&env);
    assert_eq!(status["idle_exit_secs"], json!(5), "{status}");
    assert_eq!(
        status["sessions"],
        json!(1),
        "the silent stub is a counted session: {status}"
    );
    let pid = status["pid"].as_u64().unwrap();

    // It still serves once the client finally speaks.
    c1.initialize();
    let status = status_json(&env);
    assert_eq!(status["pid"].as_u64(), Some(pid), "same daemon: {status}");

    drop(c1);
    let start = Instant::now();
    while (env.lock_path().exists() || env.info_path().exists())
        && start.elapsed() < Duration::from_secs(15)
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !env.lock_path().exists() && !env.info_path().exists(),
        "gone after its only client left"
    );
}

/// The control: a daemon any other client started has no bounded life and is
/// still there long after the grace an extension's daemon would have left in.
#[test]
fn a_plain_autostarted_daemon_outlives_its_clients() {
    let env = Env::new("stay");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();
    let status = status_json(&env);
    assert!(
        status["idle_exit_secs"].is_null(),
        "no bounded life: {status}"
    );
    let pid = status["pid"].as_u64().unwrap();

    drop(c1);
    std::thread::sleep(Duration::from_secs(7));
    let status = status_json(&env);
    assert_eq!(
        status["pid"].as_u64(),
        Some(pid),
        "still the same daemon: {status}"
    );
    assert_eq!(status["sessions"], json!(0), "{status}");

    let (ok, _) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown");
    wait_lock_released(&env);
    assert!(!env.lock_path().exists(), "lock removed on shutdown");
}

/// The 2026-09-23 incident, from the stopping daemon's side: a daemon asked to
/// stop while a blocking task is still running (a model download there, a
/// parked `spawn_blocking` here) is gone within the shutdown deadline, and the
/// index opens from another process straight after.
///
/// Before the fix the daemon dropped its record, socket and lock file at once
/// and then waited in the tokio runtime's drop for the blocking task, holding
/// `index.db` the whole time, so every successor failed on the index. The
/// files are therefore not the proof here: the process is. The daemon is this
/// test's own child rather than one an `mcp` bridge spawned, so its exit is
/// read with `try_wait` and a zombie left for a living bridge to reap cannot
/// pass for a live process.
#[test]
fn a_stopping_daemon_does_not_wait_for_a_blocking_task() {
    let env = Env::new("park");
    env.setup_domain("eng");

    let stderr_path = env.dir.join("serve.stderr");
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    cmd.env("CRYSTALLINE_TEST_PARK_BLOCKING_SECS", "60");
    let mut daemon = cmd
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();
    env.wait_ready();
    // The hook parked its task before the socket came up. Checked, because a
    // real model download may also be running here and would make the old
    // shutdown linger by itself: without this line the test could pass for
    // the wrong reason on a machine with no network.
    let before = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    assert!(
        before.contains("test hook: parking a blocking task for 60s"),
        "the blocking task is parked before the daemon is asked to stop:\n{before}"
    );

    let (ok, out) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown: {out}");
    let asked = Instant::now();
    let status = loop {
        if let Some(status) = daemon.try_wait().unwrap() {
            break Some(status);
        }
        if asked.elapsed() > Duration::from_secs(12) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    let Some(status) = status else {
        let _ = daemon.kill();
        let _ = daemon.wait();
        panic!(
            "the daemon was still running {:?} after ctl shutdown; its stderr:\n{stderr}",
            asked.elapsed()
        );
    };
    assert!(status.success(), "a clean exit: {status:?}\n{stderr}");
    assert!(
        stderr.trim_end().ends_with("crystalline stopped"),
        "the farewell line was flushed before the exit:\n{stderr}"
    );
    assert!(
        stderr.contains("stopping (ctl shutdown)"),
        "the first shutdown line names the reason:\n{stderr}"
    );
    assert!(
        !stderr.contains("did not finish in"),
        "the steps finished on their own; the watchdog never fired:\n{stderr}"
    );
    assert!(!env.info_path().exists(), "record removed");
    assert!(!env.sock_path().exists(), "socket removed");
    assert!(!env.lock_path().exists(), "lock file removed");

    // The index lock went with the process: a direct read opens it at once.
    let db = env.state_dir().join("index.db");
    let (ok, stdout, stderr) = env.run_full(&["search", "seed", "--db", db.to_str().unwrap()]);
    assert!(
        ok,
        "the index opens right after the daemon left: {stdout}\n{stderr}"
    );
}

/// A minimal OpenAI-compatible embeddings endpoint on a loopback port: every
/// input gets the same unit vector, so every pair of engrams is related. One
/// request per connection, answered with `Connection: close`. The thread
/// lives as long as the test process.
fn serve_flat_embeddings() -> String {
    use std::io::Read as _;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            if stream.read_exact(&mut body).is_err() {
                continue;
            }
            let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let inputs = match &request["input"] {
                Value::Array(items) => items.len(),
                Value::String(_) => 1,
                _ => 0,
            };
            let data: Vec<Value> = (0..inputs)
                .map(|i| json!({ "embedding": [1.0, 0.0, 0.0], "index": i }))
                .collect();
            let reply = json!({ "data": data }).to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    format!("http://{addr}/v1")
}

/// Lesson 10 for the contradiction check: a daemon asked to stop while the
/// contradiction pass is loading its model (held on a blocking thread by the
/// test hook, where a real run downloads about 1.1 GB) is gone within the
/// shutdown deadline, and the index opens from another process straight
/// after. The embeddings come from a loopback endpoint, so the pass has lead
/// vectors and a pending pair, which is what makes it ask for the model.
#[test]
fn a_stopping_daemon_does_not_wait_for_a_contradiction_model_load() {
    let env = Env::new("nli");
    let endpoint = serve_flat_embeddings();
    std::fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    let cfg = GlobalConfig {
        service: Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        }),
        embeddings: Some(config::EmbeddingsConfig {
            provider: "openai-compatible".to_string(),
            // Named after the default model only for its line-similarity
            // floor: a model without one runs no contradiction pass at all.
            // The loopback endpoint answers the same flat vector whatever
            // the name.
            model: "granite-embedding-97m-multilingual-r2".to_string(),
            endpoint: Some(endpoint),
            api_key_env: None,
        }),
        evolve: Some(config::EvolveConfig {
            contradictions: Some("full".to_string()),
        }),
        ..GlobalConfig::default()
    };
    config::save_yaml(&env.config_path(), &cfg).unwrap();
    env.setup_domain("eng");
    let root = env.dir.join("kb-eng");
    for (slug, node) in [("eighteen", "18"), ("twenty", "20")] {
        std::fs::write(
            root.join(format!("{slug}.md")),
            format!(
                "---\ntype: engram\ntitle: {slug}\npermalink: {slug}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nThe build runs on a pinned runtime.\n\n- [fact] The build uses Node {node}\n"
            ),
        )
        .unwrap();
    }

    let stderr_path = env.dir.join("serve.stderr");
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    cmd.env("CRYSTALLINE_TEST_NLI_LOAD_HOLD_SECS", "60");
    let mut daemon = cmd
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();
    // The sync, the embed pass and the handover run in the background; the
    // pass reaches the loader once both lead vectors exist. No `wait_ready`
    // first: `ctl status` reads the store, and a pass that wrongly held the
    // store lock across the load would park it there until the load ended,
    // which would hide exactly the fault this test is for. The hook's line
    // comes after the socket is up.
    let started = Instant::now();
    loop {
        let log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        if log.contains("test hook: holding the contradiction model load for 60s") {
            break;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = daemon.kill();
            let _ = daemon.wait();
            panic!("the contradiction pass never asked for its model; stderr:\n{log}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Timed from before the request: a pass that held the store lock across
    // the load would stall the ctl answer itself, not only the exit.
    let asked = Instant::now();
    let (ok, out) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown: {out}");
    let status = loop {
        if let Some(status) = daemon.try_wait().unwrap() {
            break Some(status);
        }
        if asked.elapsed() > Duration::from_secs(12) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let gone_after = asked.elapsed();
    let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    let Some(status) = status else {
        let _ = daemon.kill();
        let _ = daemon.wait();
        panic!(
            "the daemon was still running {gone_after:?} after ctl shutdown; its stderr:\n{stderr}"
        );
    };
    assert!(
        gone_after < Duration::from_secs(12),
        "gone inside the shutdown deadline, not after the held load: {gone_after:?}\n{stderr}"
    );
    assert!(status.success(), "a clean exit: {status:?}\n{stderr}");
    assert!(
        !stderr.contains("did not finish in"),
        "the steps finished on their own; the watchdog never fired:\n{stderr}"
    );
    assert!(!env.lock_path().exists(), "lock file removed");

    // A successor opens the index at once.
    let db = env.state_dir().join("index.db");
    let (ok, stdout, stderr) = env.run_full(&["search", "seed", "--db", db.to_str().unwrap()]);
    assert!(
        ok,
        "the index opens right after the daemon left: {stdout}\n{stderr}"
    );
}

/// End to end: a daemon started read-only reports it over ctl status, hides
/// the write-gated tools from tools/list and refuses a write call by name with
/// the read-only error.
#[test]
fn read_only_daemon_reports_hides_and_refuses() {
    let env = Env::new("ro");
    env.setup_domain("eng");

    // `mcp --read-only` spawns the daemon in read-only mode, then attaches.
    let mut c1 = Mcp::spawn_read_only(&env);
    c1.initialize();
    env.wait_ready();

    // ctl status reports the mode.
    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "ctl status");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(status["read_only"], json!(true), "status: {status}");

    // tools/list hides the write-gated tools and keeps the reads, `skills`
    // among them: reading a skill is a read. `evolve_engrams` is hidden too,
    // on its own gate: it reads, but every finding it returns prescribes a
    // mutation. The six collaboration tools are all absent here because the
    // two gates compose: GitHub is off, which withholds the five that need it,
    // and read-only withholds `configure`. With collaboration on, a read-only
    // instance would list `update_domain` and `origin_status` (a pull is a
    // derived-truth update like sync, status is a pure read).
    let names = c1.list_tools();
    assert_eq!(names.len(), 10, "read-only exposes 10 tools: {names:?}");
    for hidden in [
        "write_engram",
        "edit_engram",
        "move_engram",
        "split_engram",
        "delete_engram",
        "remove_domain",
        "evolve_engrams",
    ] {
        assert!(
            !names.contains(&hidden.to_string()),
            "{hidden} hidden: {names:?}"
        );
    }
    assert!(names.contains(&"skills".to_string()), "{names:?}");

    // Calling a hidden tool by name returns the read-only error, not a panic.
    c1.send_call(
        "write_engram",
        json!({ "domain": "eng", "title": "Nope", "content": "no" }),
    );
    let resp = c1.read();
    let msg = resp
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        msg.contains("read-only"),
        "read-only error expected: {resp}"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// End to end: a daemon whose read-only mode comes from
/// `CRYSTALLINE_SERVICE_READ_ONLY` (not the `--read-only` flag) reports it over
/// ctl status and refuses a write over the socket, so a container can serve
/// read-only through the environment alone.
#[test]
fn env_read_only_daemon_reports_and_refuses_a_write() {
    let env = Env::new("env-ro");
    env.setup_domain("eng");

    // The domain was registered read-write above; the daemon starts read-only
    // purely from the environment variable.
    let mut c1 = Mcp::spawn_env_read_only(&env);
    c1.initialize();
    env.wait_ready();

    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "ctl status");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(status["read_only"], json!(true), "status: {status}");

    // A write over the socket returns the read-only error, not a success.
    c1.send_call(
        "write_engram",
        json!({ "domain": "eng", "title": "Nope", "content": "no" }),
    );
    let resp = c1.read();
    let msg = resp
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        msg.contains("read-only"),
        "read-only error expected: {resp}"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

#[test]
fn watcher_indexes_external_write_without_duplicates() {
    let env = Env::new("watch");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    // Write a new engram file directly into the domain folder.
    std::fs::write(
        env.dir.join("kb-eng/watched.md"),
        "---\ntype: engram\ntitle: Watched\npermalink: watched\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nwatched unique body\n",
    )
    .unwrap();

    // Poll search until the watcher has indexed it (bounded wait).
    let mut found = false;
    for _ in 0..60 {
        c1.send_call(
            "search_engrams",
            json!({ "query": "watched", "domains": ["eng"] }),
        );
        let r = c1.read_tool_value();
        let hits = r["hits"].as_array().cloned().unwrap_or_default();
        let matching = hits
            .iter()
            .filter(|h| h["permalink"] == json!("watched"))
            .count();
        if matching >= 1 {
            assert_eq!(matching, 1, "no duplicate rows for the indexed file: {r}");
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(found, "the watcher indexed the external write");

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// A full reindex is no longer a window in which the index is empty: the daemon
/// keeps answering the whole time, from the rows it had before the rebuild
/// started, and says out loud that a rebuild is in flight.
///
/// The window is made deterministic rather than raced for. The daemon is
/// started with `CRYSTALLINE_TEST_REBUILD_HOLD_MS`, which the shared reindex
/// driver honours after it stamps a domain's marker and before it walks the
/// domain's files - exactly the state the two observation flags below are
/// looking for. The knob goes on the *daemon*, not on the `ctl` client that
/// asks for the rebuild, because the daemon is the process that runs the
/// driver. Without it the flags sample a window a 600-engram rebuild can close
/// inside one polling lap, which is a race, and a test that observes a race is
/// a test that flakes.
///
/// The corpus stays large, because the "never an empty page" assertion wants
/// laps to sample and that one is about what a reader sees at every instant,
/// not about the hold.
#[test]
fn the_daemon_keeps_answering_during_a_full_reindex() {
    let env = Env::new("rbld");
    env.setup_domain("eng");

    // Seed the corpus before the daemon exists, so the watcher has nothing to
    // race and the sync below is one plain direct pass.
    const ENGRAMS: usize = 600;
    for i in 0..ENGRAMS {
        std::fs::write(
            env.dir.join(format!("kb-eng/e{i}.md")),
            format!(
                "---\ntype: engram\ntitle: E {i}\npermalink: e{i}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nrebuildtoken payload number {i}\n"
            ),
        )
        .unwrap();
    }
    let (ok, _) = env.run(&["sync"]);
    assert!(ok, "seed sync");

    let total_before = search_total(&env);
    assert_eq!(total_before, ENGRAMS as u64, "the corpus is indexed");

    // The daemon runs the driver, so the hold is set here, where the daemon is
    // started. Comfortably above two polling laps, so the state the flags are
    // looking for cannot close between two samples of it.
    let mut c1 = Mcp::spawn_with_env(&env, "CRYSTALLINE_TEST_REBUILD_HOLD_MS", "800");
    c1.initialize();
    env.wait_ready();

    // The rebuild runs in its own process against the live daemon.
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let mut rebuild = cmd
        .args(["ctl", "reindex", "--full"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut saw_live_activity = false;
    let mut saw_marker = false;
    let started = Instant::now();
    loop {
        let finished = rebuild.try_wait().unwrap().is_some();
        // One read and one status per lap, both through the daemon that is
        // rebuilding.
        assert_eq!(
            search_total(&env),
            total_before,
            "the daemon answers from the rows it already had, never an empty page"
        );
        let status = status_json(&env);
        if status["activity"]["now"]
            .as_array()
            .is_some_and(|now| now.iter().any(|a| a["kind"] == json!("reindex")))
        {
            saw_live_activity = true;
        }
        if domains_rebuilding(&status).contains(&"eng".to_string()) {
            saw_marker = true;
        }
        if finished {
            break;
        }
        // Paced rather than spun: two daemon round-trips per lap as fast as
        // they complete would make the test's own contention part of why the
        // window is wide enough to observe, which is not a property to rely on.
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        rebuild.wait().unwrap().success(),
        "the full reindex succeeded"
    );
    assert!(
        saw_live_activity,
        "ctl status reported the reindex while it ran"
    );
    assert!(
        saw_marker,
        "ctl status reported the domain's rebuild marker while it ran"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(800),
        "the daemon honoured the hold, so the window above was held open rather than caught: {:?}",
        started.elapsed()
    );

    // And afterwards: the marker is gone and the rows are all still there.
    let status = status_json(&env);
    assert!(
        domains_rebuilding(&status).is_empty(),
        "a finished rebuild leaves no marker: {status}"
    );
    assert_eq!(search_total(&env), total_before);

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// `reindex --wipe` is the one verb that still destroys the index, and it needs
/// the file to itself. A running daemon is holding it, so the wipe must refuse
/// and name the holder - never discard a healthy index out from under a process
/// that is still serving from it.
///
/// The failure mode this pins is specific: the wipe opens resiliently, and a
/// resilient open discards a database it cannot open. A file held by another
/// process is exactly such a database, so without the lock check the wipe would
/// delete a live index and report success.
#[test]
fn reindex_wipe_refuses_while_a_daemon_holds_the_index() {
    let env = Env::new("wipe");
    env.setup_domain("eng");
    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    let db = env.state_dir().join("index.db");
    let size_before = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
    assert!(size_before > 0, "the daemon's index exists");

    let (ok, _out, err) = env.run_full(&["reindex", "--wipe"]);
    assert!(!ok, "the wipe refuses while the daemon holds the index");
    assert!(
        err.contains("crystalline") && err.to_lowercase().contains("daemon"),
        "the refusal names the holder and what to do: {err}"
    );

    // The index file is still there and the daemon still answers from it.
    assert!(db.exists(), "the held index file was not discarded");
    let (ok, out) = env.run(&["--json", "search", "seed"]);
    assert!(ok, "the daemon still serves: {out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["total"].as_u64().unwrap_or(0) >= 1, "{v}");

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// The hit count for the corpus token, through whatever route the environment
/// resolves - a running daemon, here.
fn search_total(env: &Env) -> u64 {
    let (ok, out) = env.run(&["--json", "search", "rebuildtoken"]);
    assert!(ok, "search failed: {out}");
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("search json: {e}: {out}"));
    v["total"]
        .as_u64()
        .unwrap_or_else(|| panic!("no total: {v}"))
}

fn status_json(env: &Env) -> Value {
    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "ctl status failed: {out}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("status json: {e}: {out}"))
}

/// The domains whose rows the status report says are mid-rebuild.
fn domains_rebuilding(status: &Value) -> Vec<String> {
    status["domains"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["rebuild_started"].is_string())
        .filter_map(|d| d["name"].as_str().map(str::to_string))
        .collect()
}

/// The daemon gap this covers: a domain registered by `domain add` after the
/// daemon started is not in its startup config snapshot, so its watcher never
/// knew the root existed either. `domain add` must still route its own sync
/// through the running daemon (ctl sync resolves the domain from a fresh
/// config read) and that same resolution must add a live watch, with no
/// daemon restart.
#[test]
fn domain_add_while_daemon_running_syncs_and_watches_the_new_domain() {
    let env = Env::new("dyndom");
    env.setup_domain("eng");

    // The daemon starts against a config that only knows about "eng".
    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    // Register a second domain while that daemon is still running. This
    // itself proves the ctl sync round trip succeeds for a domain the
    // daemon has never heard of.
    env.setup_domain("docs");

    // The daemon's own MCP session finds the new domain's pre-existing seed
    // file, without ever restarting the daemon.
    c1.send_call(
        "search_engrams",
        json!({ "query": "seed body token", "domains": ["docs"] }),
    );
    let r = c1.read_tool_value();
    assert!(
        r["total"].as_u64().unwrap_or(0) >= 1,
        "search over MCP finds the new domain's pre-existing files: {r}"
    );

    // An externally written file in the new domain is picked up by the
    // watcher within a bounded wait, proving the dynamic watch (not just the
    // one-off sync) is live for a domain discovered after startup.
    std::fs::write(
        env.dir.join("kb-docs/external.md"),
        "---\ntype: engram\ntitle: External\npermalink: external\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\ndocs external unique body\n",
    )
    .unwrap();

    let mut found = false;
    for _ in 0..60 {
        c1.send_call(
            "search_engrams",
            json!({ "query": "external unique body", "domains": ["docs"] }),
        );
        let r = c1.read_tool_value();
        let hits = r["hits"].as_array().cloned().unwrap_or_default();
        if hits.iter().any(|h| h["permalink"] == json!("external")) {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        found,
        "the watcher picked up an external write in a domain added after daemon start"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// A rename a crash stopped right after writing its journal is finished by
/// the next daemon before it answers anything: the first call a client makes
/// already sees the new name, the configuration carries it with the old name
/// as an alias, and the domain's state folder moved with it.
#[test]
fn a_daemon_finishes_a_half_done_rename_before_it_serves() {
    finishes_a_half_done_rename_before_serving("renjrnl", Mcp::spawn);
}

/// The same for the embedded MCP stack, which opens the index itself when no
/// daemon runs.
#[test]
fn the_embedded_stack_finishes_a_half_done_rename_before_it_serves() {
    finishes_a_half_done_rename_before_serving("renemb", Mcp::spawn_embedded);
}

/// The same for a one-shot command with no daemon running: it opens the
/// index itself and finishes the rename before it reads anything.
#[test]
fn a_standalone_command_finishes_a_half_done_rename_first() {
    let env = Env::new("renone");
    env.setup_domain("eng");
    plant_rename_journal(&env);

    let (ok, out) = env.run(&["--json", "search", "seed"]);
    assert!(ok, "{out}");
    let hits: Value = serde_json::from_str(&out).unwrap();
    assert!(
        hits["hits"]
            .as_array()
            .is_some_and(|h| !h.is_empty() && h.iter().all(|h| h["domain"] == "platform")),
        "the hits carry the new name: {hits}"
    );
    assert!(!env.state_dir().join("rename-journal.json").exists());
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(cfg.domains.contains_key("platform") && !cfg.domains.contains_key("eng"));
}

/// The journal a rename of `eng` to `platform` leaves when it stops right
/// after writing it, before its first step: started against this env's own
/// index, configuration and state directory, which every opener below has
/// to agree on to finish it.
fn plant_rename_journal(env: &Env) {
    let index = std::fs::canonicalize(env.state_dir().join("index.db"))
        .unwrap()
        .display()
        .to_string();
    plant_rename_journal_for(env, &index);
}

/// [`plant_rename_journal`] for the index its store names `index`.
fn plant_rename_journal_for(env: &Env, index: &str) {
    // A Postgres index leaves no file here, so the directory may be new.
    std::fs::create_dir_all(env.state_dir()).unwrap();
    let canonical = |p: PathBuf| std::fs::canonicalize(p).unwrap().display().to_string();
    std::fs::write(
        env.state_dir().join("rename-journal.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "old": "eng",
            "new": "platform",
            "local_only": true,
            "origin": "explicit",
            "old_spellings": ["eng"],
            "owner": {
                "index": index,
                "config": canonical(env.config_path()),
                "state_dir": canonical(env.state_dir()),
            },
            "done": [],
        }))
        .unwrap(),
    )
    .unwrap();
}

/// A journal of a rename of `eng` to `platform` recorded against this env's
/// own index and the configuration file `config`, with `done` already run:
/// the shape a crash leaves when the configuration was reached through
/// another spelling than the one this machine opens now.
fn plant_rename_journal_recorded_with(env: &Env, config: &Path, done: &[&str]) {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap().display().to_string();
    std::fs::write(
        env.state_dir().join("rename-journal.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "old": "eng",
            "new": "platform",
            "local_only": true,
            "origin": "explicit",
            "old_spellings": ["eng"],
            "owner": {
                "index": canonical(&env.state_dir().join("index.db")),
                "config": canonical(config),
                "state_dir": canonical(&env.state_dir()),
            },
            "done": done,
        }))
        .unwrap(),
    )
    .unwrap();
}

/// A journal whose recorded configuration is no longer the one this machine
/// opens is finished by nobody on its own. `doctor` names it as a problem,
/// with what is done and what is left, and says to make the recorded
/// configuration this machine's own again before the plain command; `--fix`
/// leaves it alone; a command pointed at the recorded configuration with
/// `--config` is refused and moves nothing; and the plain command with the
/// recorded configuration made this machine's own finishes it.
#[test]
fn doctor_names_a_rename_journal_nobody_finishes_and_the_command_that_does() {
    let env = env_with_movable_state("rendoc");
    let recorded = env.dir.join("recorded.yaml");
    std::fs::copy(env.config_path(), &recorded).unwrap();
    plant_rename_journal_recorded_with(&env, &recorded, &[]);

    let (ok, out, err) = env.run_full(&["--json", "doctor"]);
    assert!(!ok, "a stuck rename is a problem: {out}{err}");
    let report: Value = serde_json::from_str(&out).unwrap();
    let rename = &report["rename"];
    assert_eq!(rename["belongs_here"], json!(false), "{rename}");
    assert_eq!(rename["done"], json!([]), "{rename}");
    assert_eq!(rename["remaining"][0], json!("index_row"), "{rename}");
    let recorded_text = std::fs::canonicalize(&recorded)
        .unwrap()
        .display()
        .to_string();
    assert_eq!(
        rename["finish"],
        json!("crystalline domain rename eng platform --local"),
        "{rename}"
    );
    assert!(
        rename["restore"][0]
            .as_str()
            .is_some_and(|r| r.contains("CRYSTALLINE_CONFIG") && r.contains(&recorded_text)),
        "{rename}"
    );

    let (_, human, _) = env.run_full(&["doctor", "--fix"]);
    assert!(
        human.contains("the rename of 'eng' to 'platform' is half done")
            && human.contains("crystalline doctor --discard-rename"),
        "{human}"
    );
    assert!(
        env.state_dir().join("rename-journal.json").is_file(),
        "--fix never drops a journal"
    );

    // Pointed at the recorded configuration, the rename is refused: only
    // this machine's own configuration finishes a journal.
    let before = rename_state_snapshot(&env, &[&recorded]);
    let (ok, out, err) = env.run_full(&[
        "domain",
        "rename",
        "eng",
        "platform",
        "--local",
        "--config",
        recorded.to_str().unwrap(),
    ]);
    assert!(!ok, "{out}{err}");
    assert!(rename_state_snapshot(&env, &[&recorded]) == before);
    assert!(env.state_dir().join("rename-journal.json").is_file());

    // The recorded configuration made this machine's own again.
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &recorded)
        .args(["domain", "rename", "eng", "platform", "--local"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the plain command finishes the rename: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!env.state_dir().join("rename-journal.json").exists());
    assert!(
        env.state_dir()
            .join("origins/platform/state.json")
            .is_file()
    );
    let cfg: GlobalConfig = config::load_yaml(&recorded).unwrap();
    assert!(cfg.domains.contains_key("platform") && !cfg.domains.contains_key("eng"));
}

/// `doctor --discard-rename` drops a journal on request, and says that the
/// steps it already ran stay as they are.
#[test]
fn doctor_discards_a_rename_journal_only_when_asked() {
    let env = env_with_movable_state("rendis");
    let recorded = env.dir.join("recorded.yaml");
    std::fs::copy(env.config_path(), &recorded).unwrap();
    plant_rename_journal_recorded_with(&env, &recorded, &["index_row"]);

    let (_, human, _) = env.run_full(&["doctor", "--discard-rename"]);
    assert!(
        human.contains("[discarded]")
            && human.contains("done index_row")
            && human.contains("finish or undo them by hand"),
        "{human}"
    );
    assert!(!env.state_dir().join("rename-journal.json").exists());
    let (_, out, _) = env.run_full(&["--json", "doctor"]);
    let report: Value = serde_json::from_str(&out).unwrap();
    assert!(report["rename"].is_null(), "{report}");
}

/// A command that names another index with `--db` leaves a rename journal of
/// this machine's own index alone: nothing moves, the configuration keeps the
/// old name, and a warning names the command that finishes it for real. The
/// plain command afterward, against the index the journal belongs to,
/// finishes it.
#[test]
fn a_standalone_command_on_another_index_leaves_the_rename_alone() {
    let env = Env::new("renoth");
    env.setup_domain("eng");
    let origins = env.state_dir().join("origins");
    std::fs::create_dir_all(origins.join("eng")).unwrap();
    plant_rename_journal(&env);
    let other = env.dir.join("other.db");
    let other = other.to_str().unwrap();

    let (ok, out, err) = env.run_full(&["--json", "--db", other, "search", "seed"]);
    assert!(ok, "{out}{err}");
    assert!(
        err.contains("left alone here") && err.contains("crystalline domain rename eng platform"),
        "the warning names the rename and how to finish it: {err}"
    );
    assert!(env.state_dir().join("rename-journal.json").is_file());
    assert!(origins.join("eng").is_dir() && !origins.join("platform").exists());
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(cfg.domains.contains_key("eng") && !cfg.domains.contains_key("platform"));

    let (ok, out) = env.run(&["--json", "search", "seed"]);
    assert!(ok, "{out}");
    assert!(!env.state_dir().join("rename-journal.json").exists());
    assert!(origins.join("platform").is_dir());
}

/// Two processes on one Postgres index never both run a rename journal:
/// Postgres has no file lock to keep a second opener out, so the ownership
/// of the state directory the journal lives in is what serializes them. A
/// standalone command while another process holds it leaves the journal
/// alone; once that process is gone, the same command finishes it. Skipped
/// with a note unless `CRYSTALLINE_TEST_POSTGRES_URL` is set and `psql` runs.
#[test]
fn two_openers_of_one_postgres_index_never_both_run_a_rename() {
    let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
        eprintln!("note: skipping the postgres leg (CRYSTALLINE_TEST_POSTGRES_URL is unset)");
        return;
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let schema = format!("renpg_{nanos}");
    let psql = |sql: &str| {
        Command::new("psql")
            .args(["-v", "ON_ERROR_STOP=1", "-q", "-d", &url, "-c", sql])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !psql(&format!("CREATE SCHEMA {schema}")) {
        eprintln!("note: skipping the postgres leg (psql could not create a schema)");
        return;
    }
    struct DropSchema<'a>(&'a dyn Fn(&str) -> bool, String);
    impl Drop for DropSchema<'_> {
        fn drop(&mut self) {
            (self.0)(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.1));
        }
    }
    let _cleanup = DropSchema(&psql, schema.clone());

    let env = Env::new("renpg");
    let sep = if url.contains('?') { '&' } else { '?' };
    let db_url = format!("{url}{sep}options=-c%20search_path%3D{schema},public");
    std::fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    std::fs::write(
        env.config_path(),
        format!(
            "database:\n  backend: postgres\n  url: \"{db_url}\"\nservice:\n  response_format: json\n"
        ),
    )
    .unwrap();
    env.setup_domain("eng");
    // The store names a Postgres index by host and database, no credentials
    // and no query.
    let after_scheme = url.split("://").nth(1).unwrap();
    let host_db = after_scheme
        .rsplit_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(after_scheme);
    let host_db = host_db.split(['?', '#']).next().unwrap();
    plant_rename_journal_for(&env, host_db);
    let config_path = env.config_path();
    let config_arg = config_path.to_str().unwrap();

    let wedge = Wedge::spawn(&env);
    let (ok, out, err) = env.run_full(&["--json", "search", "seed", "--config", config_arg]);
    assert!(ok, "{out}{err}");
    assert!(err.contains("not finished by this command"), "{err}");
    assert!(env.state_dir().join("rename-journal.json").is_file());
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(cfg.domains.contains_key("eng") && !cfg.domains.contains_key("platform"));
    drop(wedge);

    let (ok, out, err) = env.run_full(&["--json", "search", "seed", "--config", config_arg]);
    assert!(ok, "{out}{err}");
    assert!(
        !env.state_dir().join("rename-journal.json").exists(),
        "{err}"
    );
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(cfg.domains.contains_key("platform") && !cfg.domains.contains_key("eng"));
}

/// A sync that names its index with `--config` does not line a domain's
/// name up with a new MANIFEST name: an adoption renames this machine's own
/// state and configuration, which only a sync of the machine's own index may
/// do. The plain sync afterward does it.
#[test]
fn a_sync_with_an_override_adopts_no_name() {
    let env = Env::new("adoptov");
    env.setup_domain("ops");
    let dir = env.dir.join("kb-plat");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: Plat\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Plat\n",
    )
    .unwrap();
    let (ok, out, err) = env.run_full(&["domain", "add", "--path", dir.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    let manifest = std::fs::read_to_string(dir.join("MANIFEST.md")).unwrap();
    assert!(manifest.contains("domain_name: kb-plat"), "{manifest}");
    std::fs::write(
        dir.join("MANIFEST.md"),
        manifest.replace("domain_name: kb-plat", "domain_name: platform"),
    )
    .unwrap();

    let config_path = env.config_path();
    let (ok, out, err) = env.run_full(&["sync", "--config", config_path.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    assert!(
        err.contains("note: domain names were not lined up")
            && err.contains("run `crystalline sync` without --db and --config"),
        "the sync says the names were not lined up, and which command does it: {err}"
    );
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key("kb-plat") && !cfg.domains.contains_key("platform"),
        "{:?}",
        cfg.domains.keys()
    );

    let (ok, out, err) = env.run_full(&["sync"]);
    assert!(ok, "{out}{err}");
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key("platform") && !cfg.domains.contains_key("kb-plat"),
        "{out}{err} {:?}",
        cfg.domains.keys()
    );
}

/// A standalone command reaching past a process that holds this machine's
/// state directory (here the wedge's lock, as a daemon holds it) does not
/// run the rename journal, even against the index the journal belongs to:
/// that process finishes it, and two processes never run one journal.
#[test]
fn a_standalone_command_leaves_the_rename_to_the_lock_holder() {
    let env = Env::new("renlck");
    env.setup_domain("eng");
    plant_rename_journal(&env);
    let index = env.state_dir().join("index.db");
    let index = index.to_str().unwrap().to_string();

    let wedge = Wedge::spawn(&env);
    let (ok, out, err) = env.run_full(&["--json", "--db", &index, "search", "seed"]);
    assert!(ok, "{out}{err}");
    assert!(
        err.contains("not finished by this command"),
        "the command says why it left the rename: {err}"
    );
    assert!(env.state_dir().join("rename-journal.json").is_file());
    // Nor does sending the same rename again: that would run the journal
    // beside the holder, which finishes it itself.
    let (ok, out, err) = env.run_full(&[
        "--db", &index, "domain", "rename", "eng", "platform", "--local",
    ]);
    assert!(!ok, "a resend beside the holder is refused: {out}{err}");
    assert!(
        err.contains("holds this machine's state directory"),
        "the refusal names the holder: {err}"
    );
    assert!(env.state_dir().join("rename-journal.json").is_file());
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(cfg.domains.contains_key("eng") && !cfg.domains.contains_key("platform"));
    drop(wedge);

    // The holder gone, the same command against the same index finishes it.
    let (ok, out, err) = env.run_full(&["--json", "--db", &index, "search", "seed"]);
    assert!(ok, "{out}{err}");
    assert!(
        !env.state_dir().join("rename-journal.json").exists(),
        "{err}"
    );
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(cfg.domains.contains_key("platform") && !cfg.domains.contains_key("eng"));
}

/// The state a rename moves, read back byte for byte: the origin and draft
/// folders under the state directory, the accounts database, the provision
/// receipt and the configuration file, each keyed by its path.
fn rename_state_snapshot(env: &Env, extra: &[&Path]) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(path: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        if path.is_dir() {
            out.push((path.to_path_buf(), Vec::new()));
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            entries.sort();
            for entry in entries {
                walk(&entry, out);
            }
        } else if path.is_file() {
            out.push((path.to_path_buf(), std::fs::read(path).unwrap()));
        }
    }
    let mut out = Vec::new();
    let state = env.state_dir();
    for part in ["origins", "overlays", "web-auth.db", "provisions.json"] {
        walk(&state.join(part), &mut out);
    }
    walk(&env.config_path(), &mut out);
    for path in extra {
        walk(path, &mut out);
    }
    out
}

/// A domain `eng` with state a rename would move: an origin folder and a
/// draft folder under this machine's state directory.
fn env_with_movable_state(tag: &str) -> Env {
    let env = Env::new(tag);
    env.setup_domain("eng");
    let state = env.state_dir();
    std::fs::create_dir_all(state.join("origins/eng")).unwrap();
    std::fs::write(state.join("origins/eng/state.json"), "{}").unwrap();
    std::fs::create_dir_all(state.join("overlays/eng")).unwrap();
    std::fs::write(state.join("overlays/eng/draft.json"), "{}").unwrap();
    env
}

/// A rename through `--db` naming another index than this machine's own is
/// refused before anything moves: this machine's state folders, its
/// configuration and its own index all keep the old name, and the refusal
/// names what does not match and the command to run instead.
#[test]
fn a_rename_against_another_index_is_refused_and_moves_nothing() {
    let env = env_with_movable_state("renodb");
    let other = env.dir.join("other.db");
    let other = other.to_str().unwrap();
    // The other index knows `eng` too, so nothing but the guard stops it.
    let (ok, out, err) = env.run_full(&["--db", other, "sync"]);
    assert!(ok, "{out}{err}");
    let before = rename_state_snapshot(&env, &[]);

    let (ok, out, err) = env.run_full(&[
        "--db", other, "domain", "rename", "eng", "platform", "--local",
    ]);
    assert!(!ok, "a rename against another index is refused: {out}{err}");
    assert!(
        err.contains("runs only against this machine's own index")
            && err.contains("is not this machine's own index")
            && err.contains("crystalline domain rename eng platform --local` without --db"),
        "the refusal names the index and the command to run: {err}"
    );
    assert!(
        rename_state_snapshot(&env, &[]) == before,
        "nothing moved: state folders and configuration are unchanged"
    );
    assert!(!env.state_dir().join("rename-journal.json").exists());
    let (ok, out) = env.run(&["--json", "domain", "list"]);
    assert!(ok, "{out}");
    assert_eq!(
        domain_names(&serde_json::from_str(out.trim()).unwrap()),
        ["eng"]
    );
}

/// The same for `--config` naming another configuration file, here a copy
/// of this machine's own, so it registers the same domain over the same
/// index: neither file changes and nothing under the state directory moves.
#[test]
fn a_rename_through_another_configuration_is_refused_and_moves_nothing() {
    let env = env_with_movable_state("renocfg");
    let other = env.dir.join("other.yaml");
    std::fs::copy(env.config_path(), &other).unwrap();
    let before = rename_state_snapshot(&env, &[&other]);

    let (ok, out, err) = env.run_full(&[
        "domain",
        "rename",
        "eng",
        "platform",
        "--local",
        "--config",
        other.to_str().unwrap(),
    ]);
    assert!(
        !ok,
        "a rename through another configuration is refused: {out}{err}"
    );
    assert!(
        err.contains("is not this machine's own configuration")
            && !err.contains("is not this machine's own index"),
        "the refusal names the configuration, and only that: {err}"
    );
    assert!(
        rename_state_snapshot(&env, &[&other]) == before,
        "nothing moved: state folders and both configuration files are unchanged"
    );
    assert!(!env.state_dir().join("rename-journal.json").exists());
}

/// `--db` and `--config` spelling this machine's own index and configuration
/// open nothing else, so the rename runs.
#[test]
fn a_rename_naming_this_machines_own_index_runs() {
    let env = env_with_movable_state("renown");
    let index = env.state_dir().join("index.db");
    let config_path = env.config_path();
    let (ok, out, err) = env.run_full(&[
        "--db",
        index.to_str().unwrap(),
        "domain",
        "rename",
        "eng",
        "platform",
        "--local",
        "--config",
        config_path.to_str().unwrap(),
    ]);
    assert!(ok, "{out}{err}");
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(cfg.domains.contains_key("platform") && !cfg.domains.contains_key("eng"));
    assert!(
        env.state_dir()
            .join("origins/platform/state.json")
            .is_file()
    );
    assert!(
        env.state_dir()
            .join("overlays/platform/draft.json")
            .is_file()
    );
}

/// Make the MANIFEST of `name` (set up by [`Env::setup_domain`]) declare
/// `declared` as its domain name, the way an owner changing it upstream does.
fn declare_manifest_name(env: &Env, name: &str, declared: &str) {
    let manifest = env.dir.join(format!("kb-{name}")).join("MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let text = match text.lines().find(|l| l.starts_with("domain_name:")) {
        Some(line) => text.replacen(line, &format!("domain_name: {declared}"), 1),
        None => text.replacen("---\n", &format!("---\ndomain_name: {declared}\n"), 1),
    };
    std::fs::write(&manifest, text).unwrap();
}

/// Wait until `log` holds `needle`, or fail naming what it holds.
fn wait_for_log(log: &Path, needle: &str, child: &mut Child) -> String {
    let start = Instant::now();
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if text.contains(needle) {
            return text;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the process ended ({status}) before it logged '{needle}': {text}");
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "'{needle}' never reached the log: {text}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A domain registered by its folder alone, so its name `kb-eng` is derived
/// and follows its MANIFEST, with an origin folder under the state directory
/// that a rename would move.
fn env_with_derived_domain(tag: &str) -> Env {
    let env = Env::new(tag);
    env.setup_domain("ops");
    let dir = env.dir.join("kb-eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: Eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Eng\n",
    )
    .unwrap();
    let (ok, out, err) = env.run_full(&["domain", "add", "--path", dir.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key("kb-eng"),
        "{:?}",
        cfg.domains.keys()
    );
    std::fs::create_dir_all(env.state_dir().join("origins/kb-eng")).unwrap();
    std::fs::write(env.state_dir().join("origins/kb-eng/state.json"), "{}").unwrap();
    env
}

/// A daemon started on another index than this machine's own (`serve --db`)
/// holds this machine's state directory, but it neither renames a domain nor
/// lines names up with their MANIFESTs: either would move this machine's
/// state folders and write its configuration while this machine's own index
/// keeps the old names. A rename sent to it is refused in words that name
/// the index, and nothing moves.
#[test]
fn a_daemon_on_another_index_refuses_a_rename_and_lines_up_no_names() {
    let env = env_with_derived_domain("svfor");
    declare_manifest_name(&env, "eng", "platform");
    let other = env.dir.join("other.db");
    let log = env.dir.join("serve.log");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .arg("--db")
        .arg(&other)
        .args(["serve", "--config"])
        .arg(env.config_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    env.wait_ready();
    let text = wait_for_log(&log, "not lined up with their MANIFESTs here", &mut child);
    assert!(text.contains("is not this machine's own index"), "{text}");
    let before = rename_state_snapshot(&env, &[]);

    let (ok, out, err) = env.run_full(&["domain", "rename", "kb-eng", "renamed", "--local"]);
    assert!(!ok, "the daemon refuses the rename: {out}{err}");
    assert!(
        err.contains("is not this machine's own index") && err.contains("Nothing was renamed"),
        "{err}"
    );
    let (ok, out, err) = env.run_full(&["ctl", "sync"]);
    assert!(ok, "{out}{err}");
    assert!(
        rename_state_snapshot(&env, &[]) == before,
        "nothing moved: state folders and configuration are unchanged"
    );
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key("kb-eng"),
        "{:?}",
        cfg.domains.keys()
    );

    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.wait();
}

/// A daemon started before this machine's configuration file exists, the
/// way a fresh install starts one, then a domain added through it and
/// renamed through it: the configuration written after the start is still
/// this machine's own, so the rename goes through. `config_home` points
/// `XDG_CONFIG_HOME` somewhere else for every command, when given.
fn serve_before_the_configuration_exists_then_rename(env: &Env, config_home: Option<&Path>) {
    let command = || {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        if let Some(home) = config_home {
            cmd.env("XDG_CONFIG_HOME", home);
        }
        cmd
    };
    let config_file = config_home
        .map(|home| home.join("crystalline/config.yaml"))
        .unwrap_or_else(|| env.config_path());
    assert!(!config_file.exists(), "no configuration yet");
    let dir = env.dir.join("kb-eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n",
    )
    .unwrap();

    let mut child = command()
        .args(["serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !command()
        .args(["ctl", "status", "--json"])
        .output()
        .unwrap()
        .status
        .success()
    {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "daemon not ready"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let run = |args: &[&str]| {
        let out = command().args(args).output().unwrap();
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let (ok, out) = run(&["domain", "add", "eng", dir.to_str().unwrap()]);
    assert!(ok, "{out}");
    assert!(config_file.is_file(), "the daemon wrote the configuration");
    let (ok, out) = run(&["domain", "rename", "eng", "platform", "--local"]);
    assert!(ok, "this machine's own daemon renames: {out}");
    let cfg: GlobalConfig = config::load_yaml(&config_file).unwrap();
    assert!(cfg.domains.contains_key("platform") && !cfg.domains.contains_key("eng"));

    let _ = run(&["ctl", "shutdown"]);
    let _ = child.wait();
}

/// The plain case: a daemon started with no configuration file yet renames
/// once one was written through it. On macOS the test's `/tmp` is itself a
/// symlink.
#[test]
fn a_daemon_started_before_the_configuration_exists_still_renames() {
    let env = Env::new("svnocfg");
    serve_before_the_configuration_exists_then_rename(&env, None);
}

/// The same with the configuration folder reached through a symlink, on
/// every platform.
#[test]
fn a_daemon_started_before_a_symlinked_configuration_exists_still_renames() {
    let env = Env::new("svlnkcfg");
    let real = env.dir.join("real-config");
    std::fs::create_dir_all(&real).unwrap();
    let link = env.dir.join("config-link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    serve_before_the_configuration_exists_then_rename(&env, Some(&link));
}

/// A daemon that cannot name this machine's own index (its default
/// configuration does not load, and it was started on another one with
/// `--config`) refuses a rename and says why, and nothing moves: an unknown
/// answer never falls back to renaming unchecked.
#[test]
fn a_daemon_that_cannot_name_this_machines_index_refuses_a_rename() {
    let env = env_with_movable_state("svunk");
    let good = env.dir.join("good.yaml");
    std::fs::copy(env.config_path(), &good).unwrap();
    std::fs::write(env.config_path(), "domains: [this is not a mapping\n").unwrap();
    let log = env.dir.join("serve.log");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve", "--config"])
        .arg(&good)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    env.wait_ready();
    let before = rename_state_snapshot(&env, &[&good]);

    let (ok, out, err) = env.run_full(&["domain", "rename", "eng", "platform", "--local"]);
    assert!(!ok, "the rename is refused: {out}{err}");
    assert!(
        err.contains("cannot be named")
            && err.contains("Nothing was renamed")
            && err.contains("restart the daemon"),
        "{err}"
    );
    assert!(
        rename_state_snapshot(&env, &[&good]) == before,
        "nothing moved"
    );
    assert!(!env.state_dir().join("rename-journal.json").exists());
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(text.contains("could not be named"), "{text}");

    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.wait();
}

/// The embedded MCP stack on another index than this machine's own lines no
/// names up after its first sync either.
#[test]
fn the_embedded_stack_on_another_index_lines_up_no_names() {
    let env = env_with_derived_domain("emfor");
    declare_manifest_name(&env, "eng", "platform");
    let other = env.dir.join("other.db");
    let log = env.dir.join("mcp.log");
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let mut child = cmd
        .arg("--db")
        .arg(&other)
        .args(["mcp", "--embedded"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let mut mcp = Mcp {
        child,
        stdin,
        out,
        id: 0,
    };
    let before = rename_state_snapshot(&env, &[]);
    mcp.initialize();
    let text = wait_for_log(
        &log,
        "not lined up with their MANIFESTs here",
        &mut mcp.child,
    );
    assert!(text.contains("is not this machine's own index"), "{text}");
    assert!(rename_state_snapshot(&env, &[]) == before);
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key("kb-eng"),
        "{:?}",
        cfg.domains.keys()
    );
}

/// The daemon's file watcher hands an edit of a MANIFEST to the name table:
/// a domain whose MANIFEST starts declaring another name answers to it,
/// with nothing but the edit on disk.
#[test]
fn the_watcher_moves_a_name_a_manifest_edit_declares() {
    let env = Env::new("watchname");
    env.setup_domain("eng");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    env.wait_ready();
    assert!(
        !env.run(&["--json", "search", "seed", "--domain", "platform"])
            .1
            .contains("\"hits\":[{"),
        "no domain answers to the new name yet"
    );
    declare_manifest_name(&env, "eng", "platform");

    // Asked of the daemon, whose own name table resolves `--domain`: the
    // listing would read the MANIFEST from disk and prove nothing here.
    let hits_under = |name: &str| {
        let (_, out) = env.run(&["--json", "search", "seed", "--domain", name]);
        serde_json::from_str::<Value>(out.trim())
            .ok()
            .and_then(|v| v["hits"].as_array().map(Vec::len))
            .unwrap_or(0)
    };
    let start = Instant::now();
    while hits_under("platform") == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "the watcher never moved the name"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.wait();
}

/// This machine's own daemon still lines a derived name up with a changed
/// MANIFEST.
#[test]
fn this_machines_own_daemon_still_lines_names_up() {
    let env = env_with_derived_domain("svown");
    let dir = env.dir.join("kb-eng");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    env.wait_ready();
    let _ = dir;
    declare_manifest_name(&env, "eng", "platform");
    let (ok, out, err) = env.run_full(&["ctl", "sync"]);
    assert!(ok, "{out}{err}");
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key("platform") && !cfg.domains.contains_key("kb-eng"),
        "{out}{err} {:?}",
        cfg.domains.keys()
    );
    assert!(
        env.state_dir()
            .join("origins/platform/state.json")
            .is_file()
    );
    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.wait();
}

fn finishes_a_half_done_rename_before_serving(tag: &str, spawn: fn(&Env) -> Mcp) {
    let env = Env::new(tag);
    env.setup_domain("eng");
    let origins = env.state_dir().join("origins");
    std::fs::create_dir_all(origins.join("eng")).unwrap();
    std::fs::write(origins.join("eng/state.json"), "{}").unwrap();
    plant_rename_journal(&env);

    let mut c1 = spawn(&env);
    c1.initialize();
    c1.send_call("list_domains", json!({}));
    let listed = c1.read_tool_value();
    let names = domain_names(&listed);
    assert!(
        names.contains(&"platform".to_string()) && !names.contains(&"eng".to_string()),
        "the first answer already carries the new name: {listed}"
    );
    c1.send_call(
        "search_engrams",
        json!({ "query": "seed body token", "domains": ["eng"] }),
    );
    let hits = c1.read_tool_value();
    assert!(
        hits["total"].as_u64().unwrap_or(0) >= 1,
        "the old name still reaches the content: {hits}"
    );

    assert!(!env.state_dir().join("rename-journal.json").exists());
    assert!(origins.join("platform/state.json").is_file());
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(!cfg.domains.contains_key("eng"), "{:?}", cfg.domains.keys());
    assert_eq!(cfg.domains["platform"].aliases, vec!["eng".to_string()]);

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// The reported bug's exact shape: a bare `crystalline sync --domain <name>`
/// with a daemon running. `sync_dispatch` (`main.rs`) routes this over the
/// daemon's ctl socket instead of the direct path in `cmd.rs`, and a
/// per-file failure used to ride inside the daemon's own `data.reports[].failed`
/// as an ordinary field, so the ctl envelope around it stayed "ok" and the
/// process exited 0 regardless. The daemon path now runs the identical
/// failure check `cmd::sync` runs on the direct path, so a user cannot tell
/// which one handled their command from the exit code or the message.
#[test]
fn sync_over_a_running_daemon_fails_when_a_file_could_not_be_indexed() {
    let env = Env::new("syncfail");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    // A file whose frontmatter repeats a key lands after the daemon started,
    // so this sync is the first thing to see it - the same duplicate-`tags`
    // shape the original report hit.
    std::fs::write(
        env.dir.join("kb-eng/bad.md"),
        "---\ntype: engram\ntitle: Bad\npermalink: bad\ntags: [a]\ntags: [b]\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody.\n",
    )
    .unwrap();

    let (ok, stdout, stderr) = env.run_full(&["sync", "--domain", "eng"]);
    assert!(
        !ok,
        "a partial failure over the daemon must fail the process, not exit 0: stdout={stdout} stderr={stderr}"
    );
    // The daemon path prints the full report before failing, same as the
    // direct path - `sync_dispatch` always renders the daemon's JSON answer
    // through `print_value`, so the shape differs from the direct path's
    // plain-text summary line, but the evidence is the same either way: the
    // report, the failing file's path and the reason are all still on
    // stdout, printed before the process fails.
    assert!(
        stdout.contains("\"failed\""),
        "the report still prints in full before the failure: {stdout}"
    );
    assert!(
        stdout.contains("bad.md"),
        "the failing file is named: {stdout}"
    );
    assert!(
        stdout.contains("duplicate entry with key"),
        "the reason travels with it: {stdout}"
    );
    assert!(
        stderr.contains("failed to sync") && stderr.contains("eng"),
        "the failure names the count and the domain on stderr: {stderr}"
    );

    // A clean sync over the same still-running daemon succeeds: the check
    // only fires on an actual failure, so the two paths cannot drift apart
    // on the happy path either.
    std::fs::remove_file(env.dir.join("kb-eng/bad.md")).unwrap();
    let (ok, out) = env.run(&["sync", "--domain", "eng"]);
    assert!(ok, "a clean sync over the daemon still succeeds: {out}");

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// The other failure class the daemon path used to ignore entirely: not one
/// file with broken frontmatter, but a whole domain `Engine::sync_take_over`
/// could not scan at all - it logs a warning, records `{"domain", "error"}`
/// in the response's top-level `failed` array and moves on to the next
/// domain, rather than aborting the sweep. That per-domain record rode in
/// the ctl envelope exactly like a per-file one, so a daemon-routed sync
/// with one unscannable domain among several still exited 0. A missing
/// directory is the easy way to trigger it: `scan_domain` errors loudly the
/// moment its walk root itself cannot be read (see its own comment), and a
/// removed directory hits that same branch as a permission error would.
#[test]
fn sync_over_a_running_daemon_fails_when_a_domain_could_not_be_scanned() {
    let env = Env::new("scanfail");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    // Registered while the daemon is up, exactly like the sibling
    // `domain_add_while_daemon_running_...` test, so this domain is fully
    // synced and known-good before its directory disappears out from under
    // it.
    env.setup_domain("broken");
    std::fs::remove_dir_all(env.dir.join("kb-broken")).unwrap();

    // No `--domain` filter: `Engine::sync_take_over` only soft-skips a
    // domain whose scan failed when sweeping everything (`only.is_none()`);
    // naming one domain that fails would abort with a plain error instead,
    // which is not the shape this bug needs (multiple domains, one bad).
    let (ok, stdout, stderr) = env.run_full(&["sync"]);
    assert!(
        !ok,
        "a domain that could not be scanned at all must fail the process, not exit 0: stdout={stdout} stderr={stderr}"
    );
    // The full report still prints before the failure, "eng" included, so a
    // healthy domain's result is never hidden by a sibling's failure.
    assert!(
        stdout.contains("\"eng\""),
        "eng's own report still prints: {stdout}"
    );
    assert!(
        stdout.contains("broken"),
        "the daemon's failed-domains array still names it: {stdout}"
    );
    assert!(
        stderr.contains("could not be scanned") && stderr.contains("broken"),
        "the failure names the domain and says it could not be scanned, not that a file failed to parse: {stderr}"
    );
    assert!(
        !stderr.contains("file(s) failed to sync"),
        "the wrong failure class must not be claimed - no file parse failure happened here: {stderr}"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// The colleague's fourth defect, exactly as reported: a healthy daemon is
/// running and `crystalline doctor` used to die on
/// `could not open the index ... File is locked by another process`, because
/// doctor was the one index-touching command with no daemon route. It now
/// asks the daemon for the file stamps its orphan and unindexed checks need,
/// so the diagnosis a person runs while the service is up actually runs, and
/// it still splits "not indexed yet" from "cannot be indexed until the
/// frontmatter is fixed".
///
/// The second domain is registered by editing `config.yaml` directly rather
/// than through `domain add`: `domain add` routes a sync through the running
/// daemon, and a watched domain's well-formed file would be indexed within
/// the debounce, so there would be no unindexed file left to report. The
/// daemon never watches a domain it did not know at startup, which keeps
/// `good.md` unindexed for the length of the test while `bad.md` stays
/// unindexable whatever anyone runs.
#[test]
fn doctor_over_a_running_daemon_reports_instead_of_failing_on_the_index_lock() {
    let env = Env::new("docdaemon");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();
    // The edits of `config.yaml` below load, change and save it, and the
    // daemon's own write after its initial sync must not land in between.
    env.wait_names_recorded();

    let docs = env.dir.join("kb-docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(
        docs.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: docs\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# docs\n\n## Scope\n\n- docs\n\n## When to Use\n\n- Route here for docs\n",
    )
    .unwrap();
    std::fs::write(
        docs.join("good.md"),
        "---\ntype: engram\ntitle: Good\npermalink: good\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nA perfectly well-formed engram nobody has indexed yet.\n",
    )
    .unwrap();
    std::fs::write(
        docs.join("bad.md"),
        "---\ntype: engram\ntitle: \"Bad\npermalink: bad\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody.\n",
    )
    .unwrap();
    let mut cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    cfg.domains.insert(
        "docs".to_string(),
        crystalline_core::config::DomainEntry::file(&docs),
    );
    config::save_yaml(&env.config_path(), &cfg).unwrap();

    let (ok, stdout, stderr) = env.run_full(&["--json", "doctor"]);
    assert!(
        !stdout.contains("locked by another process")
            && !stderr.contains("locked by another process"),
        "doctor never collides with the service it diagnoses: stdout={stdout} stderr={stderr}"
    );
    assert!(
        !ok,
        "the two unhealthy files are problems, so doctor exits 1: stdout={stdout} stderr={stderr}"
    );
    let report: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("doctor still produced its report: {e}: stdout={stdout} stderr={stderr}")
    });
    assert_eq!(
        report["index"]["source"],
        json!("daemon"),
        "the index reads went through the running daemon: {report}"
    );
    assert!(
        report["names"].is_object() && report["names"].get("error").is_none(),
        "the name findings came from the daemon too (ctl name_report): {report}"
    );
    let docs_report = report["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == json!("docs"))
        .expect("the second domain is in the report")
        .clone();
    assert_eq!(
        docs_report["unindexed"],
        json!(["MANIFEST.md", "good.md"]),
        "the well-formed files are reported as not indexed yet, the manifest among them: {docs_report}"
    );
    assert_eq!(
        docs_report["unsyncable"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        vec!["bad.md".to_string()],
        "the broken file is the other class, not merely unsynced: {docs_report}"
    );
    assert!(
        docs_report["unsyncable"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("frontmatter YAML is invalid"),
        "the reason travels with it: {docs_report}"
    );

    // The domain the daemon does watch is clean, so the daemon-served stamps
    // are read as stamps, not as "nothing is indexed".
    let eng = report["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == json!("eng"))
        .expect("the watched domain is in the report")
        .clone();
    assert_eq!(eng["unindexed"], json!([]), "{eng}");
    assert_eq!(eng["index_checked"], json!(true), "{eng}");

    // The human report says where its index answers came from, and keeps the
    // two file classes apart there too.
    let (_, human, _) = env.run_full(&["doctor"]);
    assert!(
        human.contains("index: read through the running daemon"),
        "the report names the route it took: {human}"
    );
    assert!(
        human.contains("run: crystalline sync --domain docs\n")
            && human.contains("cannot be indexed until the frontmatter is fixed"),
        "and gives each class its own guidance: {human}"
    );

    // `--domain` sends a name over the socket instead of asking for every
    // domain, which is a different resolution on the daemon side. It must
    // still be answered there: a refusal would fall through to the direct
    // open, hit the same lock and report a partial run, which reads as
    // "doctor works" from the outside while being the bug again.
    let (_, filtered, stderr) = env.run_full(&["--json", "doctor", "--domain", "docs"]);
    let report: Value = serde_json::from_str(&filtered)
        .unwrap_or_else(|e| panic!("a filtered run still reports: {e}: {filtered} {stderr}"));
    assert_eq!(
        report["index"]["source"],
        json!("daemon"),
        "a named domain is served by the daemon too, not fallen back to a direct open: {report}"
    );
    let names: Vec<String> = report["domains"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(names, vec!["docs".to_string()], "{report}");
    assert_eq!(
        report["domains"][0]["unindexed"],
        json!(["MANIFEST.md", "good.md"])
    );

    // A virtual domain has no files to stamp, so the daemon answers with
    // nothing for it. That is an answer, not a refusal: the run stays on the
    // daemon route and says the count was not read rather than printing a
    // zero it never looked up.
    let mut cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    cfg.domains.insert(
        "ideas".to_string(),
        crystalline_core::config::DomainEntry {
            kind: crystalline_core::config::DomainKind::Virtual,
            path: None,
            origin: None,
            provision: None,
            review: None,
            ..Default::default()
        },
    );
    config::save_yaml(&env.config_path(), &cfg).unwrap();
    let (ok, virtual_json, stderr) = env.run_full(&["--json", "doctor", "--domain", "ideas"]);
    assert!(
        ok,
        "a virtual domain with nothing wrong exits 0: {virtual_json} {stderr}"
    );
    let report: Value = serde_json::from_str(&virtual_json).unwrap();
    assert_eq!(report["index"]["source"], json!("daemon"), "{report}");
    assert_eq!(report["domains"][0]["engrams"], Value::Null, "{report}");
    let (_, virtual_human, _) = env.run_full(&["doctor", "--domain", "ideas"]);
    assert!(
        virtual_human.contains("ok (virtual, engram count not read)"),
        "no fabricated count: {virtual_human}"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// `doctor --fix` over a running daemon respells a link that names a domain
/// by a name only this machine uses: the daemon runs the fix (ctl
/// `fix_local_spellings`) and then the report (ctl `name_report`), and the
/// next plain run is clean for that finding.
#[test]
fn doctor_fix_over_a_running_daemon_respells_local_only_links() {
    let env = Env::new("docnames");
    env.setup_domain("eng");
    env.setup_domain("ops");
    // `eng` keeps its explicit local name here while its MANIFEST calls it
    // `engineering`; a link in `ops` spells it `eng`.
    std::fs::write(
        env.dir.join("kb-eng/MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ndomain_name: engineering\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- eng\n\n## When to Use\n\n- Route here for eng\n",
    )
    .unwrap();
    let note = env.dir.join("kb-ops/note.md");
    std::fs::write(
        &note,
        "---\ntype: engram\ntitle: Note\npermalink: note\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nSee [[eng:seed]] for the seed.\n",
    )
    .unwrap();

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    // The daemon's startup sync indexes the edits; wait until it reports
    // the link.
    let start = Instant::now();
    let report = loop {
        let (_, out) = env.run(&["--json", "doctor"]);
        let report: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
        if report["names"]["local_spellings"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            break report;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the daemon never reported the link: {out}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(report["index"]["source"], json!("daemon"), "{report}");

    let (ok, out, err) = env.run_full(&["--json", "doctor", "--fix"]);
    let fixed: Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: stdout={out} stderr={err}"));
    assert!(ok, "{fixed}");
    assert_eq!(fixed["names"]["fixed"], json!(1), "{fixed}");
    assert_eq!(fixed["names"]["local_spellings"], json!([]), "{fixed}");
    let text = std::fs::read_to_string(&note).unwrap();
    assert!(text.contains("[[engineering:seed]]"), "{text}");

    let (ok, out) = env.run(&["--json", "doctor"]);
    let again: Value = serde_json::from_str(&out).unwrap();
    assert!(ok, "{again}");
    assert_eq!(again["names"]["local_spellings"], json!([]), "{again}");

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// The domain names in a `list_domains` result value.
fn domain_names(value: &Value) -> Vec<String> {
    value["domains"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d["name"].as_str().map(str::to_string))
        .collect()
}

/// The routing bug this covers: CLI verbs are socket-first, so with a live
/// daemon they used to route `config set`, `domain add`, `status` and every
/// other verb over the socket even when the caller passed an explicit
/// `--config`/`--db`. The daemon serves ITS OWN default config and index, so an
/// override's read (or, worse, its write) landed in the daemon's world instead
/// of the named file. Here a real daemon owns the default config and index; the
/// overridden commands must operate purely on a separate location and leave the
/// daemon's config and index untouched.
#[test]
fn explicit_overrides_bypass_a_running_daemon() {
    let env = Env::new("bypass");
    env.setup_domain("eng");

    // A real daemon owns this isolated HOME's default config and index.
    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();
    // The snapshot below is compared byte for byte, so the daemon's own write
    // after its initial sync has to have landed first.
    env.wait_names_recorded();

    // Snapshot the daemon's own config before any overridden command runs.
    let daemon_config_before = std::fs::read_to_string(env.config_path()).unwrap();

    // A second, entirely separate config + index the overrides target.
    let side = env.dir.join("side");
    std::fs::create_dir_all(&side).unwrap();
    let side_config = side.join("config.yaml");
    let side_db = side.join("index.db");

    // `config set --config <side>` writes the side file and never routes the
    // write over the socket, so the daemon's config stays byte for byte identical.
    let (ok, _) = env.run(&[
        "--json",
        "config",
        "set",
        "github.enabled",
        "true",
        "--config",
        side_config.to_str().unwrap(),
    ]);
    assert!(ok, "config set --config <side>");
    let side_raw = std::fs::read_to_string(&side_config).unwrap();
    assert!(
        side_raw.contains("enabled: true"),
        "the side config got the change: {side_raw}"
    );
    assert_eq!(
        std::fs::read_to_string(env.config_path()).unwrap(),
        daemon_config_before,
        "an overridden config set left the running daemon's config file untouched"
    );

    // A data verb: `domain add --config <side> --db <side>` registers into the
    // side config and indexes into the side db, never the daemon's. Without the
    // fix the sync half of `domain add` routed to the daemon, which cannot even
    // resolve a domain the side config alone registered, so this failed.
    let side_domain = env.dir.join("kb-side");
    std::fs::create_dir_all(&side_domain).unwrap();
    std::fs::write(
        side_domain.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: side\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# side\n\n## Scope\n\n- side\n\n## When to Use\n\n- Route here for side\n",
    )
    .unwrap();
    std::fs::write(
        side_domain.join("seed.md"),
        "---\ntype: engram\ntitle: Side\npermalink: side\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nsidebandtoken body\n",
    )
    .unwrap();
    let (ok, _) = env.run(&[
        "--json",
        "domain",
        "add",
        "side",
        side_domain.to_str().unwrap(),
        "--config",
        side_config.to_str().unwrap(),
        "--db",
        side_db.to_str().unwrap(),
    ]);
    assert!(ok, "domain add --config/--db <side>");
    assert!(
        side_db.exists(),
        "the overridden domain add created the side index"
    );

    // The side config registers 'side'; the daemon's config still knows only 'eng'.
    let side_cfg: GlobalConfig = config::load_yaml(&side_config).unwrap();
    assert!(
        side_cfg.domains.contains_key("side"),
        "the side config registered 'side': {side_cfg:?}"
    );
    let daemon_cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        daemon_cfg.domains.contains_key("eng"),
        "the daemon still knows 'eng': {daemon_cfg:?}"
    );
    assert!(
        !daemon_cfg.domains.contains_key("side"),
        "the overridden domain add never reached the daemon's config: {daemon_cfg:?}"
    );

    // `status --config/--db <side>` opens the side index directly and reports
    // its 'side' domain, proving status bypassed the daemon too.
    let (ok, out) = env.run(&[
        "--json",
        "status",
        "--config",
        side_config.to_str().unwrap(),
        "--db",
        side_db.to_str().unwrap(),
    ]);
    assert!(ok, "status --config/--db <side>");
    let side_status: Value = serde_json::from_str(&out).unwrap();
    let side_domains: Vec<String> = side_status["domains"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        side_domains.contains(&"side".to_string()),
        "the side index reports 'side' via an overridden status: {side_status}"
    );

    // The daemon's own index never saw the side domain: its MCP session lists
    // only 'eng' and finds nothing for the side engram's unique token.
    c1.send_call("list_domains", json!({}));
    let listed = c1.read_tool_value();
    let names = domain_names(&listed);
    assert!(
        names.contains(&"eng".to_string()),
        "daemon lists 'eng': {listed}"
    );
    assert!(
        !names.contains(&"side".to_string()),
        "the daemon's index never learned about 'side': {listed}"
    );
    c1.send_call("search_engrams", json!({ "query": "sidebandtoken" }));
    let hits = c1.read_tool_value();
    assert_eq!(
        hits["total"].as_u64().unwrap_or(0),
        0,
        "the daemon's index holds none of the side domain's content: {hits}"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// A data verb that took the direct path because of `--db`/`--config` says so
/// on stderr, the same sentence `status` prints for the same reason
/// (`status_with_an_override_says_bypassed`), so an empty answer from the
/// wrong index is never mistaken for a genuine miss - field finding 3.
///
/// The daemon here owns a separate config and index of its own, never the
/// plain default: `search --config <other>` (no `--db`) then reads that
/// untouched default directly, which is empty rather than locked, so the
/// command succeeds and the note is the only sign anything was bypassed.
/// `--json` carries no such note; its stdout is the same empty result either
/// way.
#[test]
fn search_with_an_override_notes_the_bypass_on_stderr() {
    let env = Env::new("search-bypass-note");

    let side_db = env.dir.join("side.db");
    let side_config = env.dir.join("side.yaml");
    let side_domain = env.dir.join("kb-side");
    std::fs::create_dir_all(&side_domain).unwrap();
    std::fs::write(
        side_domain.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: side\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# side\n\n## Scope\n\n- side\n\n## When to Use\n\n- Route here for side\n",
    )
    .unwrap();
    std::fs::write(
        side_domain.join("seed.md"),
        "---\ntype: engram\ntitle: Seed\npermalink: seed\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nseed body token\n",
    )
    .unwrap();
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let ok = cmd
        .arg("--db")
        .arg(&side_db)
        .arg("domain")
        .arg("add")
        .arg("side")
        .arg(&side_domain)
        .arg("--config")
        .arg(&side_config)
        .status()
        .unwrap()
        .success();
    assert!(ok, "domain add into the side config/index");

    // The daemon serves that side index, never the plain default one that
    // `search --config <other>` (no `--db`) is about to read directly.
    let mut c1 = Mcp::spawn_with_db(&env, &side_config, &side_db);
    c1.initialize();

    let other_config = env.dir.join("other.yaml");
    let (ok, stdout, stderr) =
        env.run_full(&["search", "seed", "--config", other_config.to_str().unwrap()]);
    assert!(ok, "search --config <other> must succeed: {stderr}");
    assert_eq!(stdout, "no results\n", "stdout carries no daemon note");
    assert!(
        stderr.contains("Daemon: bypassed (--db/--config override); reading the index directly"),
        "the same sentence status prints for a bypass: {stderr}"
    );

    let (ok, json_stdout) = env.run(&[
        "--json",
        "search",
        "seed",
        "--config",
        other_config.to_str().unwrap(),
    ]);
    assert!(ok, "search --json --config <other> must succeed");
    assert_eq!(
        json_stdout,
        "{\"count\":0,\"hits\":[],\"limit\":10,\"mode\":\"text\",\"page\":1,\"total\":0}\n",
        "--json stdout is byte-identical to today"
    );

    drop(c1);
    let _ = Command::new(bin())
        .args(["--db"])
        .arg(&side_db)
        .arg("ctl")
        .arg("shutdown")
        .output();
}

/// A `--db` client pointed straight at a daemon's own index meets the same
/// named-holder composer the non-override standalone fallback already uses
/// (`search_names_the_daemon_and_the_remedy_when_the_index_cannot_be_opened`),
/// never the raw backend lock text leading - field finding 5.
#[test]
fn search_with_a_db_override_names_the_holder_on_a_held_lock() {
    let env = Env::new("search-db-override-locked");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();
    let pid = env.lock_pid().expect("the daemon published its pid");

    let db = env.state_dir().join("index.db");
    let (ok, stdout, stderr) = env.run_full(&["search", "seed", "--db", db.to_str().unwrap()]);
    assert!(
        !ok,
        "a --db pointed at the daemon's own held index must fail: {stdout}"
    );
    assert!(
        stderr.contains(&format!("(pid {pid})")),
        "the holder is named: {stderr}"
    );
    assert!(
        stderr.contains("--db or --config"),
        "the override remedy is given: {stderr}"
    );
    assert!(
        !stderr.contains("reading the index directly"),
        "a failed direct read must never claim it read the index directly: {stderr}"
    );
    let holder_at = stderr
        .find("owns the index at")
        .unwrap_or_else(|| panic!("the holder is named: {stderr}"));
    let raw_at = stderr
        .find("The index reported: ")
        .unwrap_or_else(|| panic!("the backend's own words are kept: {stderr}"));
    assert!(
        holder_at < raw_at,
        "the holder is named before the raw lock text, which never leads: {stderr}"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// `crystalline edit ... set_frontmatter` assigns a lifecycle field from
/// --key/--value and never waits on stdin for content it does not use, so the
/// same one-call fix an evolve queue prescribes works from the command line.
#[test]
fn edit_set_frontmatter_assigns_a_field_without_reading_stdin() {
    let env = Env::new("set-fm");
    env.setup_domain("eng");

    let (ok, out) = env.run(&[
        "edit",
        "seed",
        "eng",
        "set_frontmatter",
        "--key",
        "status",
        "--value",
        "deprecated",
        "--json",
    ]);
    assert!(ok, "edit set_frontmatter: {out}");
    let text = std::fs::read_to_string(env.dir.join("kb-eng/seed.md")).unwrap();
    assert!(text.contains("status: deprecated"), "{text}");
    assert!(!text.contains("status: current"), "{text}");

    let _ = env.run(&["ctl", "shutdown"]);
}

/// `crystalline edit ... set_frontmatter --key tags --values a,b` replaces the
/// tag list with the folded, comma-separated one.
#[test]
fn edit_set_frontmatter_sets_tags_from_a_comma_list() {
    let env = Env::new("set-fm-tags");
    env.setup_domain("eng");

    let (ok, out) = env.run(&[
        "edit",
        "seed",
        "eng",
        "set_frontmatter",
        "--key",
        "tags",
        "--values",
        "api,Retry Policy",
        "--json",
    ]);
    assert!(ok, "edit set_frontmatter tags: {out}");
    let text = std::fs::read_to_string(env.dir.join("kb-eng/seed.md")).unwrap();
    assert!(
        text.contains("tags:\n  - api\n  - retry-policy\n"),
        "{text}"
    );

    // --values with nothing after it is the empty list: every tag goes.
    let (ok, out) = env.run(&[
        "edit",
        "seed",
        "eng",
        "set_frontmatter",
        "--key",
        "tags",
        "--values",
        "--json",
    ]);
    assert!(ok, "edit set_frontmatter empty tags: {out}");
    let text = std::fs::read_to_string(env.dir.join("kb-eng/seed.md")).unwrap();
    assert!(!text.contains("tags:"), "{text}");
    assert!(!text.contains("retry-policy"), "{text}");

    let _ = env.run(&["ctl", "shutdown"]);
}

/// The CLI-daemon seam must stay format-independent. A daemon on the TOON
/// response-format default still answers a CLI data command with structured
/// engine JSON, because the command routes over the ctl `tool` command rather
/// than the MCP stream whose list-shaped tools would emit TOON. Without that
/// routing `--json` would hand back one opaque TOON string instead of a
/// parseable object, breaking its byte contract.
#[test]
fn data_command_against_a_toon_daemon_returns_json() {
    let env = Env::new("toon-seam");
    // No json pin: the daemon serves the TOON response-format default.
    env.setup_domain_toon("eng");

    // A real daemon owns this HOME's default config and index.
    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    // A data verb through the real CLI, routed to the running daemon.
    let (ok, out) = env.run(&["search", "seed body token", "--json"]);
    assert!(ok, "search --json: {out}");
    let value: Value = serde_json::from_str(out.trim())
        .expect("the CLI returns structured JSON even when the daemon default is TOON");
    assert!(
        value["total"].as_u64().unwrap_or(0) >= 1,
        "search finds the seeded engram as parseable JSON: {value}"
    );
    assert!(value["hits"].is_array(), "hits is a JSON array: {value}");

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

#[test]
fn http_smoke_initialize_list_and_search() {
    let env = Env::new("http");
    env.setup_domain("eng");

    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve", "--http", &addr, "--config"])
        .arg(env.config_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_port(&addr);
    // Give the router a moment after the port opens.
    std::thread::sleep(Duration::from_millis(300));

    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/");

    // /health answers without an MCP handshake: static liveness for load
    // balancers and uptime monitors.
    let health = client.get(format!("http://{addr}/health")).send().unwrap();
    assert_eq!(health.status().as_u16(), 200, "GET /health is 200");
    let body: Value = health.json().unwrap();
    assert_eq!(body["status"], "ok", "{body}");
    assert_eq!(
        body["version"].as_str().unwrap(),
        crystalline_core::VERSION,
        "{body}"
    );

    // `crystalline healthcheck` probes the same endpoint over a plain
    // TcpStream, no daemon socket involved: this is the exact command the
    // container image runs as its Docker HEALTHCHECK.
    let healthcheck = Command::new(bin())
        .args(["healthcheck", &addr])
        .output()
        .unwrap();
    assert!(
        healthcheck.status.success(),
        "healthcheck against a live daemon exits 0: {healthcheck:?}"
    );
    let healthcheck_stdout = String::from_utf8_lossy(&healthcheck.stdout);
    assert!(
        healthcheck_stdout.contains("\"status\":\"ok\""),
        "healthcheck prints the health body: {healthcheck_stdout}"
    );

    // initialize
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "http", "version": "0" } }
            })
            .to_string(),
        )
        .send()
        .unwrap();
    let session = resp
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    let init = parse_jsonrpc(&resp.text().unwrap());
    assert!(
        init.pointer("/result/protocolVersion").is_some(),
        "initialize over HTTP: {init}"
    );
    let session = session.expect("streamable HTTP returns a session id");

    // initialized notification
    let _ = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .body(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string())
        .send()
        .unwrap();

    // tools/list
    let list = parse_jsonrpc(
        &client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", &session)
            .body(
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} })
                    .to_string(),
            )
            .send()
            .unwrap()
            .text()
            .unwrap(),
    );
    let tools = list
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .unwrap();
    // What a writable default install lists. Nothing declaring provisioning
    // does not hide `provision` - that gate refuses at call time, since
    // `add_domain` and `update_domain` can create a declaration mid-call - but
    // GitHub collaboration being off does withhold the five tools that need
    // it, so the count here is the default one rather than every tool this
    // server implements (see crystalline-service's mcp_collab suite for the
    // full gating matrix).
    assert_eq!(tools.len(), 20, "a default install's tools over HTTP");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"configure"), "{names:?}");
    assert!(names.contains(&"add_domain"), "{names:?}");
    assert!(names.contains(&"evolve_engrams"), "{names:?}");

    // one search
    let search = parse_jsonrpc(
        &client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", &session)
            .body(
                json!({
                    "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                    "params": { "name": "search_engrams", "arguments": { "query": "token" } }
                })
                .to_string(),
            )
            .send()
            .unwrap()
            .text()
            .unwrap(),
    );
    assert!(
        search.pointer("/result/content/0/text").is_some(),
        "search over HTTP returns content: {search}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// An unrelated process holding the HTTP address is not fatal: the daemon says
/// which address failed, why, and what still works, then keeps serving MCP and
/// ctl over its socket. With the endpoint on by default this is the way most
/// people will ever meet a port conflict, so the line has to be actionable.
///
/// The squatted address is an ephemeral port this test binds itself, never the
/// real `127.0.0.1:7411` default: a test must not fight the developer's own
/// daemon for the one real port. The `--http` flag also proves it wins over the
/// `CRYSTALLINE_SERVICE_HTTP=false` this env applies.
#[test]
fn an_occupied_http_address_is_not_fatal_and_says_so() {
    let env = Env::new("busy");
    env.setup_domain("eng");

    let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = squatter.local_addr().unwrap().to_string();

    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve", "--http", &addr, "--config"])
        .arg(env.config_path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The warning rides the daemon's stderr. Reading it on a thread keeps a line
    // that never arrives from blocking this test forever: the deadline below
    // fails it instead.
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut warning = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) if line.contains("HTTP endpoint failed on") => {
                warning = Some(line);
                break;
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let warning = warning.expect("the daemon reports the HTTP bind it could not take");
    assert!(
        warning.contains(&addr),
        "the line names the address that failed: {warning}"
    );
    assert!(
        warning.contains("MCP over the socket is unaffected"),
        "the line says what still works: {warning}"
    );
    // Both opt-out spellings, and the flag one first: this daemon took its
    // address from `--http`, which beats every `service.http` spelling, so a
    // line that named only the config key would be advice that does nothing
    // for the very case this test drives.
    assert!(
        warning.contains("serve --http off"),
        "the line names the opt-out that applies to a flag-configured bind: {warning}"
    );
    assert!(
        warning.contains("service.http=false"),
        "the line names the config opt-out too: {warning}"
    );

    // The daemon itself is healthy: its socket answers as if nothing happened.
    env.wait_ready();
    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "ctl status still answers over the socket: {out}");

    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.kill();
    let _ = child.wait();
}

/// A foreground `serve` prints the copyright line even when stderr is
/// redirected, because AGPL section 13 means a user of a running instance has
/// to be able to see where the source is. The line sits after the
/// `is_terminal()` guard that owns the ASCII banner and inside `if
/// !daemon_flag`, and only running it proves that placement.
///
/// This Env's `apply` sets `CRYSTALLINE_SERVICE_HTTP=false` and no `--http`
/// flag is passed, so the fixture serves no HTTP endpoint at all - which is
/// what the second assertion below trades on.
#[test]
fn a_foreground_serve_prints_the_copyright_line_to_a_redirected_stderr() {
    let env = Env::new("copyright");
    env.setup_domain("eng");

    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve", "--config"])
        .arg(env.config_path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stderr = BufReader::new(child.stderr.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut serving_line = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) if line.contains("serving on") => {
                serving_line = Some(line);
                break;
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    serving_line.expect("the daemon announces where it is serving");

    // The copyright line follows right after; collect a little more to see it.
    let mut copyright_line = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) if line.contains("Copyright") => {
                copyright_line = Some(line);
                break;
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let stderr =
        copyright_line.expect("a redirected foreground run still prints the copyright line");
    assert!(
        stderr.contains(crystalline_service::daemon::COPYRIGHT_HOLDER),
        "a redirected foreground run still names the holder: {stderr}"
    );
    assert!(
        !stderr.contains("crystalline HTTP endpoint on"),
        "and this fixture serves no HTTP endpoint, so the assertion above is about the banner line and nothing else: {stderr}"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// A flag that contradicts configuration says so at startup, naming both
/// values and the key, and the daemon serves anyway. This Env sets
/// CRYSTALLINE_SERVICE_HTTP=false, so `--http <port>` is a real difference.
#[test]
fn an_exposure_flag_that_contradicts_configuration_says_so() {
    let env = Env::new("notice");
    env.setup_domain("eng");

    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve", "--http", &addr, "--config"])
        .arg(env.config_path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stderr = BufReader::new(child.stderr.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut notice = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) if line.contains("service.http") => {
                notice = Some(line);
                break;
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let notice = notice.expect("the daemon names the flag that contradicts its configuration");
    assert!(
        notice.contains(&addr),
        "it names what the flag asked for: {notice}"
    );
    assert!(
        notice.contains("service.http says no HTTP endpoint"),
        "and what configuration says, as the absence it is rather than a value \
         the other daemons bind: {notice}"
    );
    assert!(
        notice.contains("crystalline config set service.http"),
        "and how to make it permanent: {notice}"
    );

    // A notice, not a refusal: the daemon is serving.
    wait_port(&addr);
    env.wait_ready();

    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.kill();
    let _ = child.wait();
}

/// Parse a JSON-RPC response that may be plain JSON or an SSE `data:` frame.
fn parse_jsonrpc(body: &str) -> Value {
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("data:")
            && let Ok(v) = serde_json::from_str::<Value>(rest.trim())
        {
            return v;
        }
    }
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Poll until a departing daemon has let the index lock go.
///
/// `Ownership`'s drop removes the owner record, unlocks and only then removes
/// the lock file, so the lock file's absence is the last step and the honest
/// signal that a fresh `serve` can take the lock. Without it a test that runs
/// `ctl shutdown` and immediately starts a second daemon races the first one's
/// teardown, and on a slow runner the second `serve` loses the lock and exits
/// before it ever binds. Best effort: a holder killed with `-9` leaves the
/// file behind with the lock already released, so a timeout returns quietly
/// and lets the caller proceed.
fn wait_lock_released(env: &Env) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        if !env.lock_path().exists() && !env.info_path().exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_port(addr: &str) {
    wait_port_within(addr, Duration::from_secs(8));
}

/// Like `wait_port`, but with an explicit budget. Use this at a call site
/// that has extra, deliberate work to do before the port can open (such as
/// waiting for a prior daemon on the same state directory to release its
/// lock), so a slow shared CI runner gets the time it needs without loosening
/// the budget everyone else relies on to fail fast.
fn wait_port_within(addr: &str, budget: Duration) {
    let start = Instant::now();
    loop {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        let elapsed = start.elapsed();
        if elapsed > budget {
            panic!("HTTP endpoint did not open on {addr} within {budget:?} (waited {elapsed:?})");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// [`wait_port_within`], for a child this test holds a handle to: whichever
/// comes first, the port opening or the process exiting, is what it reports.
///
/// The distinction is the whole reason this exists. `the_owner_record_...`
/// failed on CI repeatedly with nothing but "the endpoint did not open", and a
/// budget raised from 8 to 30 seconds did not help - which is the tell that the
/// daemon was not slow but gone. A wait that cannot tell those apart sends the
/// next person to look at timing, which is where the last two attempts went.
fn wait_port_or_exit(addr: &str, budget: Duration, child: &mut std::process::Child) {
    let start = Instant::now();
    loop {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "the daemon exited with {status} after {:?} instead of binding {addr}; its \
                 output is above",
                start.elapsed()
            );
        }
        let elapsed = start.elapsed();
        if elapsed > budget {
            panic!(
                "the daemon is still running but never bound {addr} within {budget:?} \
                 (waited {elapsed:?})"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `crystalline healthcheck` against a port nothing is listening on: the
/// connection is refused immediately, so this needs no daemon spawn and no
/// wait, unlike the success path piggybacked on the HTTP smoke test above.
///
/// The aggregate 4s read deadline itself is not exercised here (that would
/// need a peer that trickles bytes forever, i.e. a dedicated trickle server);
/// this only checks the failure-message shape, which every I/O path shares.
#[test]
fn healthcheck_against_nothing_exits_nonzero() {
    let port = free_port();
    let output = Command::new(bin())
        .args(["healthcheck", &format!("127.0.0.1:{port}")])
        .stdout(Stdio::null())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "healthcheck against a closed port exits nonzero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.lines().count(),
        1,
        "healthcheck failure is a single line, not an anyhow Debug chain: {stderr:?}"
    );
    assert!(
        !stderr.contains("Caused by"),
        "healthcheck failure must not print anyhow's multi-line Debug chain: {stderr:?}"
    );
    assert!(
        stderr.contains(&format!("127.0.0.1:{port}")),
        "healthcheck failure names the address it failed against: {stderr:?}"
    );
}

// --- status output and daemon visibility ------------------------------------

/// With a daemon running, `status` renders human text with a daemon line;
/// `--json` yields the daemon's merged report as one JSON object.
#[test]
fn status_with_a_daemon_renders_text_with_the_daemon_line() {
    let env = Env::new("status-up");
    env.setup_domain("eng");

    let mut client = Mcp::spawn(&env);
    client.initialize();
    env.wait_ready();

    let (ok, out) = env.run(&["status"]);
    assert!(ok, "{out}");
    assert!(out.starts_with("Daemon: running (pid "), "{out}");
    assert!(out.contains("Index: "), "{out}");
    assert!(out.contains("Activity: "), "{out}");
    assert!(out.contains("eng\t"), "{out}");
    assert!(
        !out.contains("Lifetime:"),
        "a plain daemon has no lifetime line: {out}"
    );

    let (ok, out) = env.run(&["status", "--json"]);
    assert!(ok, "{out}");
    let value: Value = serde_json::from_str(out.trim()).expect("one JSON object");
    assert!(value["pid"].as_u64().is_some(), "{value}");
    assert!(value["domains"].is_array(), "{value}");
}

/// Without a daemon and with nothing registered, `status` says so in its
/// first line and points a first-time user at `domain add` rather than
/// `sync` - there is nothing to sync yet either.
#[test]
fn status_without_a_daemon_says_not_running() {
    let env = Env::new("status-down");

    let (ok, out) = env.run(&["status"]);
    assert!(ok, "{out}");
    assert!(
        out.starts_with("Daemon: not running; reading the index directly"),
        "{out}"
    );
    assert!(
        out.contains("No domains registered yet. Run: crystalline domain add"),
        "{out}"
    );
}

/// Without a daemon, once a domain is registered but never synced, `status`
/// still renders the same human shape from a direct index read and shows the
/// domain as not indexed yet rather than disappearing - `sync` is now the
/// right pointer, unlike the nothing-registered case above.
#[test]
fn status_without_a_daemon_and_no_index_reports_registered_domains_as_not_indexed_yet() {
    let env = Env::new("status-down-registered");
    std::fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    std::fs::write(
        env.config_path(),
        "domains:\n  eng:\n    path: /tmp/does-not-need-to-exist\n",
    )
    .unwrap();

    let (ok, out) = env.run(&["status"]);
    assert!(ok, "{out}");
    assert!(
        out.starts_with("Daemon: not running; reading the index directly"),
        "{out}"
    );
    assert!(out.contains("No index at "), "{out}");
    assert!(out.contains("Run: crystalline sync"), "{out}");
    assert!(out.contains("eng\t(not indexed yet)"), "{out}");
}

/// `domain list` reaches the index the same way every other verb does, so a
/// machine with a daemon gets real counts. Before the routing it opened the
/// database itself, and the daemon that owns the file turned every count into
/// the "(not indexed)" a person reads as lost work.
#[test]
fn domain_list_with_a_daemon_reports_its_counts() {
    let env = Env::new("list-up");
    env.setup_domain("eng");

    let mut client = Mcp::spawn(&env);
    client.initialize();
    env.wait_ready();

    let (ok, out) = env.run(&["domain", "list"]);
    assert!(ok, "{out}");
    assert!(out.contains("eng\t"), "{out}");
    assert!(
        out.contains(" engrams"),
        "a running daemon should still yield counts: {out}"
    );
    assert!(
        !out.contains("(not indexed)") && !out.contains("(counts not read)"),
        "{out}"
    );
}

/// `domain rename` reaches a running daemon over the ctl `domain_rename`
/// command (`control.rs`) exactly the way `domain remove` and `domain review`
/// already do, rather than falling through to the standalone path and
/// opening the index a second time while the daemon holds it.
#[test]
fn domain_rename_with_a_daemon_renames_it() {
    let env = Env::new("rename-up");
    env.setup_domain("eng");

    let mut client = Mcp::spawn(&env);
    client.initialize();
    env.wait_ready();

    let (ok, out) = env.run(&["domain", "rename", "eng", "platform", "--local"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("Renamed 'eng' to 'platform' on this machine only"),
        "{out}"
    );

    // `--local` leaves the MANIFEST (and so the canonical name it still
    // declares, "eng") untouched, so the NAME column reads "platform (eng)"
    // rather than a bare "platform": read the registered name itself over
    // `--json` instead of the human column, which is what is actually under
    // test here (the daemon route, not the NAME-column rendering).
    let (ok, out) = env.run(&["--json", "domain", "list"]);
    assert!(ok, "{out}");
    let listed: Value = serde_json::from_str(out.trim()).unwrap();
    let names: Vec<&str> = listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["platform"], "{listed}");
}

/// A virtual domain's declared canonical name shows in the
/// NAME column with a running daemon too, read from `list_domains`'s own
/// name fields (canonical_name, aliases, name_origin, shadowed, renaming),
/// the same data the listing carries - not only standalone, where the index
/// is opened directly. `--local` leaves the virtual domain's MANIFEST engram
/// declaring its old name, so its local name ("platform") and its canonical
/// name ("kb") differ, exactly the file-domain case `domain_rename_with_a_daemon_renames_it`
/// exercises above, but for a database-backed domain.
#[test]
fn domain_list_shows_a_virtual_domains_declared_name_with_a_daemon() {
    let env = Env::new("list-virtual");
    std::fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    config::save_yaml(&env.config_path(), &GlobalConfig::default()).unwrap();

    let mut client = Mcp::spawn(&env);
    client.initialize();
    env.wait_ready();

    let (ok, out) = env.run(&["domain", "add", "kb", "--virtual"]);
    assert!(ok, "{out}");
    let (ok, out) = env.run(&["domain", "rename", "kb", "platform", "--local"]);
    assert!(ok, "{out}");

    let (ok, out) = env.run(&["--json", "domain", "list"]);
    assert!(ok, "{out}");
    let listed: Value = serde_json::from_str(out.trim()).unwrap();
    let row = listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "platform")
        .unwrap();
    assert_eq!(
        row["canonical_name"], "kb",
        "the daemon's own list_domains carries the virtual domain's declared name: {row}"
    );
}

/// With the index unreachable, `domain list` still answers: the registrations
/// come from configuration, and only the counts are missing. They say so in
/// words, rather than reading as a domain nobody has synced.
#[test]
fn domain_list_degrades_when_the_index_cannot_be_reached() {
    let env = Env::new("list-degraded");
    env.setup_domain("eng");
    // A directory is not a database, so the open fails the way a file nobody
    // may read does, with no daemon in the picture to answer instead.
    let wall = env.dir.join("not-a-database");
    std::fs::create_dir_all(&wall).unwrap();

    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd
        .args(["domain", "list", "--db"])
        .arg(&wall)
        .output()
        .unwrap();
    assert!(out.status.success(), "the listing still answers");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("eng\t"), "{stdout}");
    assert!(stdout.contains("(counts not read)"), "{stdout}");
    assert!(
        stderr.contains("engram counts were not read"),
        "the note says which half is missing: {stderr}"
    );
    assert!(
        stderr.contains("crystalline doctor --fix"),
        "and names a remedy rather than a raw lock error: {stderr}"
    );

    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd
        .args(["--json", "domain", "list", "--db"])
        .arg(&wall)
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(value["counts"]["read"], json!(false), "{value}");
    assert!(
        value["counts"]["reason"]
            .as_str()
            .is_some_and(|r| !r.is_empty()),
        "a null count is ambiguous without the reason beside it: {value}"
    );
    assert_eq!(value["domains"][0]["name"], json!("eng"), "{value}");
}

/// A verb that cannot answer any part of its question without the index
/// refuses in the same words, naming the daemon and a remedy rather than
/// handing a person a raw lock error to guess at.
#[test]
fn status_refuses_readably_when_the_index_cannot_be_reached() {
    let env = Env::new("status-unreachable");
    env.setup_domain("eng");
    let wall = env.dir.join("not-a-database");
    std::fs::create_dir_all(&wall).unwrap();

    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd.args(["status", "--db"]).arg(&wall).output().unwrap();
    assert!(!out.status.success(), "status needs the index");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`crystalline status` needs the index and could not reach it"),
        "{stderr}"
    );
    assert!(stderr.contains("crystalline doctor --fix"), "{stderr}");
}

/// A daemon that is not one: a live pid, a published record and a socket that
/// answers every ctl request with the same canned line. Enough for a client to
/// attach and read a reply, which is all a test of "what does a bad answer look
/// like to a person" needs - a real daemon cannot be made to fail its own
/// `status` on demand.
struct FakeDaemon {
    stand_in: Child,
}

impl FakeDaemon {
    /// `reply` is written back verbatim for every request, newline added.
    /// `None` closes the connection having written nothing, which is the
    /// truncated answer a daemon dying mid-exchange leaves behind.
    fn spawn(env: &Env, reply: Option<&'static str>) -> FakeDaemon {
        FakeDaemon::publish(env, Some(reply), Duration::ZERO)
    }

    /// [`FakeDaemon::spawn`] that waits `delay` before it answers.
    fn spawn_slow(env: &Env, reply: &'static str, delay: Duration) -> FakeDaemon {
        FakeDaemon::publish(env, Some(Some(reply)), delay)
    }

    /// A daemon that is alive and published but whose socket cannot be
    /// connected to at all: the record names a live pid, and nothing is
    /// listening. This is the shape that reaches the standalone fallback,
    /// because that is the one state in which a client gives up on the socket
    /// and opens the index itself.
    fn unreachable(env: &Env) -> FakeDaemon {
        FakeDaemon::publish(env, None, Duration::ZERO)
    }

    /// `listen` is the reply behaviour, or `None` to bind no socket at all.
    fn publish(env: &Env, listen: Option<Option<&'static str>>, delay: Duration) -> FakeDaemon {
        std::fs::create_dir_all(env.state_dir()).unwrap();
        // A disposable child stands in for the daemon's pid, the same trick
        // `status_notes_an_unreachable_daemon_on_stderr` uses. A far-future
        // version keeps a client from trying to displace it.
        let stand_in = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(
            env.lock_path(),
            serde_json::to_string(&json!({
                "pid": stand_in.id(),
                "socket_path": env.sock_path().display().to_string(),
                "version": "99.0.0",
                "started_at": "2026-01-01T00:00:00Z",
            }))
            .unwrap(),
        )
        .unwrap();
        let sock = env.sock_path();
        let _ = std::fs::remove_file(&sock);
        let Some(reply) = listen else {
            return FakeDaemon { stand_in };
        };
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                // The `ctl` handshake line, then the request line.
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                line.clear();
                let _ = reader.read_line(&mut line);
                std::thread::sleep(delay);
                if let Some(reply) = reply {
                    let _ = stream.write_all(format!("{reply}\n").as_bytes());
                    let _ = stream.flush();
                }
            }
        });
        FakeDaemon { stand_in }
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        let _ = self.stand_in.kill();
        let _ = self.stand_in.wait();
    }
}

/// The daemon is reachable and its answer is not usable. `domain list` still
/// answers: the registrations come from configuration, and only the counts
/// ride on the daemon. Letting the ctl error fail the command would have put
/// the daemon's bare error where the listing belongs, which is the raw text
/// this routing exists to stop showing a person.
#[test]
fn domain_list_degrades_when_the_daemon_answers_with_an_error() {
    let env = Env::new("list-ctl-err");
    env.setup_domain("eng");
    let _fake = FakeDaemon::spawn(&env, Some(r#"{"ok":false,"error":"domain_stats failed"}"#));

    let (ok, stdout, stderr) = env.run_full(&["domain", "list"]);
    assert!(ok, "the listing still answers: {stdout}{stderr}");
    assert!(stdout.contains("eng\t"), "{stdout}");
    assert!(stdout.contains("(counts not read)"), "{stdout}");
    assert!(
        stderr.contains("engram counts were not read"),
        "the note says which half is missing: {stderr}"
    );
    assert!(
        stderr.contains("crystalline doctor --fix"),
        "and names a remedy: {stderr}"
    );
    assert!(
        stderr.contains("domain_stats failed"),
        "with the daemon's own words at the end: {stderr}"
    );

    let (ok, stdout, _) = env.run_full(&["--json", "domain", "list"]);
    assert!(ok);
    let value: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(value["counts"]["read"], json!(false), "{value}");
    assert_eq!(value["domains"][0]["name"], json!("eng"), "{value}");
}

/// A domain registered in this command's own environment but not by the
/// daemon still shows the count the daemon's index holds for it: the counts
/// are the index's rows, not the daemon's registrations.
#[test]
fn domain_list_over_a_daemon_counts_a_domain_only_this_command_registers() {
    let env = Env::new("list-envonly");
    env.setup_domain("eng");
    let team = env.dir.join("kb-team");
    std::fs::create_dir_all(&team).unwrap();
    std::fs::write(
        team.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# team\n",
    )
    .unwrap();
    let _fake = FakeDaemon::spawn(
        &env,
        Some(
            r#"{"v":1,"ok":true,"data":{"domains":[{"name":"eng","engrams":2},{"name":"team","engrams":5}]}}"#,
        ),
    );
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd
        .env("CRYSTALLINE_DOMAIN_TEAM", &team)
        .args(["--json", "domain", "list"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let team_row = value["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "team")
        .cloned()
        .unwrap_or_else(|| panic!("{value}"));
    assert_eq!(team_row["engrams"], json!(5), "{value}");
    assert_eq!(value["counts"]["read"], json!(true), "{value}");
}

/// A row without `engrams` is a reply in a shape this listing does not know
/// (a daemon of another version), said as such, not "(not indexed)".
#[test]
fn domain_list_says_a_row_without_counts_is_another_shape() {
    let env = Env::new("list-nocount");
    env.setup_domain("eng");
    let _fake = FakeDaemon::spawn(
        &env,
        Some(r#"{"v":1,"ok":true,"data":{"domains":[{"name":"eng"}]}}"#),
    );
    let (ok, stdout, stderr) = env.run_full(&["domain", "list"]);
    assert!(ok, "{stdout}{stderr}");
    assert!(stdout.contains("(counts not read)"), "{stdout}");
    assert!(
        stderr.contains("did not read back in the shape this listing expects"),
        "{stderr}"
    );
}

/// A daemon that takes its time still gets its answer read: the listing
/// waits for it and shows its counts.
#[test]
fn domain_list_waits_for_a_slow_daemon() {
    let env = Env::new("list-slow");
    env.setup_domain("eng");
    let _fake = FakeDaemon::spawn_slow(
        &env,
        r#"{"v":1,"ok":true,"data":{"domains":[{"name":"eng","engrams":7}]}}"#,
        Duration::from_millis(1500),
    );
    let (ok, stdout, stderr) = env.run_full(&["--json", "domain", "list"]);
    assert!(ok, "{stdout}{stderr}");
    let value: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(value["domains"][0]["engrams"], json!(7), "{value}");
    assert_eq!(value["counts"]["read"], json!(true), "{value}");
}

/// A domain registered by name into a folder whose MANIFEST declares no
/// name gets that name written into its MANIFEST.
#[test]
fn domain_add_writes_an_explicit_name_into_a_nameless_manifest() {
    let env = Env::new("addname");
    env.setup_domain("eng");
    let manifest = std::fs::read_to_string(env.dir.join("kb-eng/MANIFEST.md")).unwrap();
    assert!(manifest.contains("domain_name: eng"), "{manifest}");
}

/// The same, for a daemon that dies mid-exchange and leaves a truncated line.
/// A different failure inside `ctl_exchange`, the same thing to say about it.
#[test]
fn domain_list_degrades_when_the_daemon_answers_nothing_at_all() {
    let env = Env::new("list-ctl-cut");
    env.setup_domain("eng");
    let _fake = FakeDaemon::spawn(&env, None);

    let (ok, stdout, stderr) = env.run_full(&["domain", "list"]);
    assert!(ok, "the listing still answers: {stdout}{stderr}");
    assert!(stdout.contains("eng\t"), "{stdout}");
    assert!(stdout.contains("(counts not read)"), "{stdout}");
    assert!(stderr.contains("crystalline doctor --fix"), "{stderr}");
}

/// A data verb that cannot reach the index says who has it and what to do,
/// never the backend's lock text on its own.
///
/// The state: a daemon is alive, its record is published, it owns the index
/// and nothing answers on its socket. The data verbs (`search` and its five
/// siblings) do not open the index through the CLI's own helper - they reach
/// it in the service crate, whose standalone fallback used to hand the raw
/// error straight out. A person reading "Locking error: File is locked by
/// another process" has no way to know a daemon exists, let alone which
/// command ends it.
#[test]
fn search_names_the_daemon_and_the_remedy_when_the_index_cannot_be_opened() {
    let env = Env::new("search-locked");
    env.setup_domain("eng");
    let _fake = FakeDaemon::unreachable(&env);
    let pid = {
        let record: Value =
            serde_json::from_slice(&std::fs::read(env.lock_path()).unwrap()).unwrap();
        record["pid"].as_u64().unwrap()
    };
    // Make the open fail the way a held index does, without needing a real
    // daemon to hold it: a directory where the database file belongs cannot be
    // opened by any backend. What is under test is the wording of a failed
    // open, not which failure produced it.
    let db = env.state_dir().join("index.db");
    let _ = std::fs::remove_file(&db);
    std::fs::create_dir_all(&db).unwrap();

    let (ok, _stdout, stderr) = env.run_full(&["search", "seed"]);
    assert!(!ok, "search needs the index: {stderr}");
    assert!(
        stderr.contains(&format!("(pid {pid})")),
        "the holder is named: {stderr}"
    );
    assert!(
        stderr.contains("owns the index at"),
        "and what it holds: {stderr}"
    );
    assert!(
        stderr.contains("crystalline doctor --fix") && stderr.contains("crystalline ctl shutdown"),
        "with the two commands that do something about it: {stderr}"
    );
    let daemon_at = stderr.find("owns the index at").unwrap();
    let raw_at = stderr
        .find("The index reported: ")
        .unwrap_or_else(|| panic!("the backend's own words are kept: {stderr}"));
    assert!(
        daemon_at < raw_at,
        "and they come last, never first: {stderr}"
    );
}

/// `status` is the verb a person reaches for when something is broken, and a
/// configuration this binary cannot parse is one of the things that can be
/// broken. The daemon is asked before the file is read, so its report still
/// arrives, and the config problem travels as a note beside it.
#[test]
fn status_still_answers_over_a_config_it_cannot_parse() {
    let env = Env::new("status-bad-config");
    env.setup_domain("eng");

    let mut client = Mcp::spawn(&env);
    client.initialize();
    env.wait_ready();
    // The daemon's pass after its initial sync loads, changes and saves the
    // file. Broken between its load and its save, the file was replaced with
    // the good copy and `status` found nothing to report.
    env.wait_names_recorded();

    // The daemon read a good config on the way up; this breaks the copy on
    // disk underneath it, which is exactly the state a person is in when they
    // reach for `status`.
    std::fs::write(
        env.config_path(),
        "domains: [unclosed
",
    )
    .unwrap();

    let (ok, stdout, stderr) = env.run_full(&["status"]);
    assert!(ok, "status answers: {stdout}{stderr}");
    assert!(stdout.starts_with("Daemon: running (pid "), "{stdout}");
    assert!(
        stderr.contains("configuration did not load"),
        "the config problem is named, not swallowed: {stderr}"
    );

    let (ok, stdout, _) = env.run_full(&["status", "--json"]);
    assert!(ok);
    let value: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(value["pid"].as_u64().is_some(), "{value}");
    assert!(
        value["config_error"]
            .as_str()
            .is_some_and(|e| !e.is_empty()),
        "--json carries the same note as a field: {value}"
    );
    // Nothing the daemon did since put a good file back.
    assert_eq!(
        std::fs::read_to_string(env.config_path()).unwrap(),
        "domains: [unclosed\n",
        "the daemon left the broken file as it is"
    );
}

/// `--db`/`--config` overrides bypass the daemon on purpose; the first line
/// says so instead of pretending to be the daemon's view.
#[test]
fn status_with_an_override_says_bypassed() {
    let env = Env::new("status-bypass");
    let side = env.dir.join("side.db");

    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd.args(["status", "--db"]).arg(&side).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("Daemon: bypassed (--db/--config override)"),
        "{stdout}"
    );
}

/// A lock record naming a live pid that answers on no socket produces a
/// stderr note, so a fallback read never silently masquerades as the
/// daemon's view - the "status says nothing is indexed" confusion.
#[test]
fn status_notes_an_unreachable_daemon_on_stderr() {
    let env = Env::new("status-orphan");
    std::fs::create_dir_all(env.state_dir()).unwrap();
    // A disposable child stands in for the daemon: alive, but its socket
    // path holds nothing. A same-or-newer version sidesteps the takeover
    // path, and Env::drop's kill -9 of the lock pid only hits the stand-in.
    let mut stand_in = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::fs::write(
        env.lock_path(),
        serde_json::to_string(&json!({
            "pid": stand_in.id(),
            "socket_path": env.sock_path().display().to_string(),
            "version": "99.0.0",
            "started_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
    )
    .unwrap();

    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd.arg("status").output().unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("did not answer"),
        "expected the unreachable-daemon note, got: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("Daemon: not running"), "{stdout}");
    let _ = stand_in.kill();
    let _ = stand_in.wait();
}

// --- the unresponsive lock holder -------------------------------------------
//
// The 2026-07-28 incident: a daemon alive, its lock held, its socket answering
// nothing. No client could attach (nothing answered) and none could spawn (the
// lock was held), so every session failed until the wedged process died on its
// own, forty minutes later. The tests below drive that state with a real
// separate process rather than a fabricated lock file, because the whole point
// of the fix is what it does to a process.

/// A real, separate process holding the index lock with a published record and
/// no socket: the wedge. `crystalline hold-lock` is a hidden entry that exists
/// for exactly this, so the holder is a genuine crystalline binary and passes
/// the identity gate on the kill path, as the wedged daemon did.
struct Wedge {
    child: Child,
}

impl Wedge {
    fn spawn(env: &Env) -> Wedge {
        Wedge::spawn_with(env, &["hold-lock", "--secs", "60"])
    }

    /// A one-shot command holding the state directory for `secs` seconds:
    /// the lock plus a record naming `command`, as a standalone rename or
    /// sync publishes it, released when the time is up.
    fn spawn_standalone(env: &Env, command: &str, secs: u64) -> Wedge {
        let secs = secs.to_string();
        Wedge::spawn_with(
            env,
            &["hold-lock", "--secs", &secs, "--standalone", command],
        )
    }

    fn spawn_with(env: &Env, args: &[&str]) -> Wedge {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        let mut child = cmd
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // The readiness line means the lock is taken and the record is
        // published; no sleep-and-hope.
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "holding", "hold-lock did not take the lock");
        Wedge { child }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether the holder left within `budget`. `try_wait` rather than a
    /// signal probe: this holder is a child of the test process, so a killed
    /// one stays visible as an unreaped zombie until it is waited for.
    fn exited_within(&mut self, budget: Duration) -> bool {
        let start = Instant::now();
        loop {
            if self.child.try_wait().unwrap().is_some() {
                return true;
            }
            if start.elapsed() > budget {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn still_holding(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

impl Drop for Wedge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The incident, end to end: a connecting client finds the lock held and the
/// socket silent, verifies the holder, dislodges it and serves from a fresh
/// daemon. Before the fix this session failed outright.
#[test]
fn a_connecting_client_dislodges_a_wedged_lock_holder() {
    let env = Env::new("wedge-mcp");
    env.setup_domain("eng");

    let mut wedge = Wedge::spawn(&env);
    let wedge_pid = wedge.pid();
    assert_eq!(
        env.lock_pid(),
        Some(u64::from(wedge_pid)),
        "the wedge published its own record"
    );

    // The client attaches to nothing, cannot take the lock, dislodges the
    // holder and starts a daemon that serves this session.
    let mut client = Mcp::spawn(&env);
    client.initialize();
    env.wait_ready();

    assert!(
        wedge.exited_within(Duration::from_secs(10)),
        "the wedged holder was dislodged"
    );
    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "a fresh daemon answers: {out}");
    let status: Value = serde_json::from_str(&out).unwrap();
    let pid = status["pid"].as_u64().unwrap();
    assert_ne!(
        pid,
        u64::from(wedge_pid),
        "the daemon serving now is a new process"
    );
    assert_eq!(env.lock_pid(), Some(pid), "the record names the successor");

    // The session really works, which is what the incident cost.
    client.send_call("search_engrams", json!({ "query": "token" }));
    let found = client.read_tool_value();
    assert!(found["total"].as_u64().unwrap() >= 1, "search: {found}");
}

/// The holder is unidentifiable (its record names a pid that cannot be alive,
/// so nothing proves who owns the lock). Doubt refuses: no signal, and an
/// error naming the lock file and how to look at the holder.
#[test]
fn an_unidentified_lock_holder_is_never_signalled() {
    let env = Env::new("wedge-unknown");
    let mut wedge = Wedge::spawn(&env);

    // Rewrite the record to name a pid that cannot exist. The lock stays held
    // by the live holder, so from the outside something owns the index and
    // nothing says what.
    std::fs::write(
        env.info_path(),
        serde_json::to_string(&json!({
            "pid": 2147483647u32,
            "socket_path": env.sock_path().display().to_string(),
            "version": "99.0.0",
            "started_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap(),
    )
    .unwrap();

    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd.args(["doctor", "--fix"]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("service.lock"), "names the lock path: {text}");
    assert!(text.contains("lsof"), "suggests lsof: {text}");
    assert!(
        text.contains("nothing was signalled"),
        "says nothing was signalled: {text}"
    );
    assert!(
        wedge.still_holding(),
        "an unidentified holder is left strictly alone"
    );
}

/// `status` is a read-only diagnosis. It names the third state - neither
/// running nor absent - and never touches the holder.
#[test]
fn status_reports_an_unresponsive_daemon_without_killing_it() {
    let env = Env::new("wedge-status");
    let mut wedge = Wedge::spawn(&env);
    let pid = wedge.pid();

    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd.arg("status").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with(&format!("Daemon: unresponsive (pid {pid} per its record)")),
        "{stdout}"
    );
    assert!(
        stdout.contains("a connecting client will replace it, or run crystalline doctor --fix"),
        "{stdout}"
    );
    assert!(
        wedge.still_holding(),
        "status diagnoses, it never signals anything"
    );
}

/// `doctor` reports the wedge as a problem and `--fix` runs the same dislodge
/// a connecting client would.
#[test]
fn doctor_reports_the_wedge_and_fix_dislodges_it() {
    let env = Env::new("wedge-doctor");
    let mut wedge = Wedge::spawn(&env);

    let (ok, out) = env.run(&["--json", "doctor"]);
    assert!(
        !ok,
        "a wedged daemon is a problem, so doctor exits 1: {out}"
    );
    let report: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["service"]["daemon_unresponsive"], json!(true));
    assert_eq!(report["service"]["daemon_dislodged"], json!(false));
    assert!(
        wedge.still_holding(),
        "doctor without --fix never signals anything"
    );

    let (ok, out) = env.run(&["--json", "doctor", "--fix"]);
    assert!(ok, "the wedge is resolved, so doctor exits 0: {out}");
    let report: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["service"]["daemon_dislodged"], json!(true));
    assert!(
        wedge.exited_within(Duration::from_secs(10)),
        "doctor --fix dislodged the holder"
    );
    assert!(!env.info_path().exists(), "the stale record was cleaned up");
}

/// The safety property in the other direction: a daemon that answers its
/// socket is never dislodged, not even by `doctor --fix`.
#[test]
fn a_responsive_daemon_survives_doctor_fix() {
    let env = Env::new("wedge-live");
    env.setup_domain("eng");
    let mut client = Mcp::spawn(&env);
    client.initialize();
    env.wait_ready();
    let before = env.lock_pid().expect("the daemon published its record");

    // A scratch `--db` keeps doctor's own index read off the database the live
    // daemon holds open (opening it would fail with a plain locking error,
    // which has nothing to do with the service section under test here). The
    // service checks read the state directory, never the `--db` path, so the
    // holder diagnosis is the real one.
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let raw = cmd
        .args(["--json", "--db"])
        .arg(env.dir.join("side.db"))
        .args(["doctor", "--fix"])
        .output()
        .unwrap();
    let ok = raw.status.success();
    let out = String::from_utf8_lossy_owned(raw.stdout);
    assert!(
        ok,
        "a healthy service is not a problem: {out}{}",
        String::from_utf8_lossy(&raw.stderr)
    );
    let report: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["service"]["daemon_unresponsive"], json!(false));
    assert_eq!(report["service"]["daemon_dislodged"], json!(false));
    assert_eq!(report["service"]["holder_unknown"], Value::Null);

    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "the daemon is still serving: {out}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        status["pid"].as_u64(),
        Some(before),
        "the same daemon still owns the index"
    );
}

/// End to end: `crystalline evolve` against a running daemon routes over the
/// ctl `tool` command like every other data verb, so `--json` hands back the
/// engine's structured queue rather than a TOON string, and the human view
/// numbers each finding, marks its class and closes with the fixed guidance.
///
/// The finding is planted through the CLI itself: flipping the seed engram to
/// `superseded` without naming a successor is exactly the half-finished
/// retirement `V004` exists to catch, and the edit goes through the daemon's
/// engine so the index agrees before the sweep runs.
#[test]
fn evolve_reports_a_planted_finding_over_the_daemon() {
    let env = Env::new("evolve");
    env.setup_domain("eng");

    let mut c1 = Mcp::spawn(&env);
    c1.initialize();
    env.wait_ready();

    let (ok, out) = env.run(&[
        "edit",
        "seed",
        "eng",
        "set_frontmatter",
        "--key",
        "status",
        "--value",
        "superseded",
        "--json",
    ]);
    assert!(ok, "edit set_frontmatter: {out}");

    // `--json` keeps the raw engine value.
    let (ok, out) = env.run(&[
        "--json",
        "evolve",
        "--domain",
        "eng",
        "--today",
        "2026-08-02",
        "--limit",
        "50",
    ]);
    assert!(ok, "evolve --json: {out}");
    let value: Value = serde_json::from_str(out.trim())
        .expect("evolve --json returns structured JSON even over the daemon");
    assert_eq!(value["scope"]["today"], json!("2026-08-02"), "{value}");
    let rules: Vec<&str> = value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["rule"].as_str())
        .collect();
    assert!(
        rules.contains(&"V004"),
        "the planted retirement without a successor must be found: {value}"
    );

    // The human view: a numbered block with the class marked, the address, the
    // per-rule legend and the guidance the constant carries.
    let (ok, out) = env.run(&["evolve", "--domain", "eng", "--today", "2026-08-02"]);
    assert!(ok, "evolve: {out}");
    assert!(out.contains("Sweep of eng as of 2026-08-02"), "{out}");
    assert!(out.contains("V004"), "{out}");
    assert!(
        out.contains("JUDGMENT") || out.contains("MECHANICAL"),
        "the class must be marked on every finding: {out}"
    );
    assert!(out.contains("crystalline://eng/seed"), "{out}");
    assert!(out.contains("Actions:"), "{out}");
    assert!(
        out.contains("This queue changes nothing by itself."),
        "the fixed guidance must close the human view: {out}"
    );

    drop(c1);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// A second `serve` loses the index lock, and says so in a way an operator can
/// act on: exit code 3 (distinct from every other startup failure, so a unit
/// file can set RestartPreventExitStatus=3), and a message naming what this
/// invocation asked to bind, what the holder's record says it bound, and the
/// key that makes every daemon on the machine bind the same way.
///
/// The holder is alive for the whole test (the `Mcp` client keeps it up), so
/// there is no teardown window to race here: the lock is held on purpose.
#[test]
fn a_serve_that_loses_the_lock_exits_three_and_says_what_was_lost() {
    let env = Env::new("lockexit");
    env.setup_domain("eng");

    let client = Mcp::spawn(&env);
    env.wait_ready();
    let owner_pid = env
        .lock_pid()
        .expect("the autostarted daemon published a record");

    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut second = Command::new(bin());
    env.apply(&mut second);
    let out = second
        .args([
            "serve",
            "--http",
            &addr,
            "--allowed-host",
            "muthur.lan",
            "--config",
        ])
        .arg(env.config_path())
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(3),
        "lock loss has its own exit code, not the generic 1"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Scoped to the refusal's own line. These flags also contradict this env's
    // configuration, so the startup notice prints the address, the allow-list
    // and both keys to the same stderr before the lock is ever attempted -
    // asserting over the whole capture would pass even if the refusal said
    // none of it.
    let refusal = stderr
        .lines()
        .find(|l| l.contains("already owns it"))
        .unwrap_or_else(|| panic!("the refusal reaches stderr: {stderr}"));
    assert!(
        refusal.contains(&addr),
        "it names what this serve asked to bind: {refusal}"
    );
    assert!(
        refusal.contains("muthur.lan"),
        "and the allow-list it asked for: {refusal}"
    );
    assert!(
        refusal.contains(&owner_pid.to_string()),
        "it names the holder ({owner_pid}): {refusal}"
    );
    assert!(
        refusal.contains("autostart"),
        "and how the holder started: {refusal}"
    );
    assert!(
        refusal.contains("service.http"),
        "and the key that reconciles them: {refusal}"
    );

    drop(client);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// A daemon started while a one-shot command holds the state directory (a
/// standalone rename, or the name adoption after a sync) waits for it and
/// says in its log what it waits for, then starts once the command is done.
#[test]
fn a_daemon_start_waits_for_a_standalone_command() {
    let env = Env::new("waitone");
    env.setup_domain("eng");
    let holder = Wedge::spawn_standalone(&env, "crystalline domain rename", 60);
    let holder_pid = holder.pid();

    let log = env.dir.join("serve.log");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args(["serve", "--config"])
        .arg(env.config_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();

    // The daemon says what it waits for while the command still holds the
    // state directory.
    let start = Instant::now();
    loop {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("waiting up to") {
            assert!(
                text.contains("`crystalline domain rename`")
                    && text.contains(&format!("pid {holder_pid}")),
                "the log says what the daemon waits for: {text}"
            );
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the daemon gave up ({status}) instead of waiting: {text}");
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the daemon never said it was waiting: {text}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // The command finishes, and the daemon starts.
    drop(holder);
    let start = Instant::now();
    loop {
        let (ok, _) = env.run(&["ctl", "status", "--json"]);
        if ok {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "the daemon gave up ({status}): {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the daemon did not start once the command let go: {}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.wait();
}

/// A one-shot command that does not let go in time makes the daemon give
/// up with the lock-held exit code, naming the command and its pid instead
/// of a holder nobody can name.
#[test]
fn a_daemon_start_names_a_standalone_command_that_does_not_let_go() {
    let env = Env::new("waitlong");
    env.setup_domain("eng");
    let holder = Wedge::spawn_standalone(&env, "crystalline domain rename", 60);
    let holder_pid = holder.pid();

    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let out = serve
        .args(["serve", "--config"])
        .arg(env.config_path())
        .env("CRYSTALLINE_TEST_STANDALONE_WAIT_MS", "300")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "the lock-held exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the standalone command `crystalline domain rename`")
            && stderr.contains(&format!("pid {holder_pid}")),
        "the refusal names the command and its pid: {stderr}"
    );
    drop(holder);
}

/// Nothing signals a one-shot command: `doctor --fix`, which ends a wedged
/// daemon, leaves a standalone rename running.
#[test]
fn doctor_fix_never_signals_a_standalone_command() {
    let env = Env::new("waitdoc");
    env.setup_domain("eng");
    let mut holder = Wedge::spawn_standalone(&env, "crystalline domain rename", 60);

    let _ = env.run_full(&["--json", "doctor", "--fix"]);
    assert!(
        holder.still_holding(),
        "the standalone command is still running after doctor --fix"
    );
    assert!(env.info_path().is_file(), "and its record is still there");
}

/// The owner record says how its daemon was started. A daemon an agent's
/// `crystalline mcp` connection spawned and one an operator ran are
/// indistinguishable from the outside today, which is what let a managed unit
/// restart-loop 277 times behind a healthy-looking localhost probe.
#[test]
fn the_owner_record_says_how_the_daemon_was_started() {
    let env = Env::new("startedby");
    env.setup_domain("eng");

    // Autostart: the mcp client finds no daemon and spawns one.
    let client = Mcp::spawn(&env);
    env.wait_ready();
    let record = env.lock_record().expect("the daemon published a record");
    assert_eq!(
        record["started_by"], "autostart",
        "a daemon spawned by a connecting client records it: {record}"
    );
    // This Env turns the endpoint off, so the binding is recorded as off
    // rather than left unrecorded.
    assert_eq!(record["http"], "off", "{record}");
    drop(client);
    let _ = env.run(&["ctl", "shutdown"]);
    // The autostarted daemon still owns the lock until its teardown finishes;
    // a second serve started inside that window loses the lock and never binds.
    wait_lock_released(&env);

    // A deliberate serve on the same state directory records the other mode.
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args([
            "serve",
            "--http",
            &addr,
            "--allowed-host",
            "muthur.lan",
            "--config",
        ])
        .arg(env.config_path())
        // Inherited rather than discarded, and that is the point. This start
        // has failed on CI more than once with nothing to go on but "the
        // endpoint did not open", because the daemon's own account of why it
        // did not was thrown away here. nextest prints a failing test's output
        // and swallows a passing one's, so this costs nothing on a green run
        // and is the whole diagnosis on a red one.
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    // The autostarted daemon above only just released the lock (see
    // `wait_lock_released`), so this second start is racing a shared CI
    // runner's scheduler rather than a fresh process; give it real room.
    //
    // Watched through `wait_port_or_exit` rather than `wait_port_within`,
    // because the two failures it can have want different answers and this
    // test has so far reported neither. A daemon still running after the
    // budget was too slow. One that has already exited was never going to
    // bind, and waiting the rest of the budget for it says nothing.
    wait_port_or_exit(&addr, Duration::from_secs(30), &mut child);
    let record = env.lock_record().expect("the daemon published a record");
    assert_eq!(record["started_by"], "serve", "{record}");
    assert_eq!(
        record["http"], addr,
        "the record names the address it bound: {record}"
    );
    assert_eq!(
        record["allowed_hosts"],
        serde_json::json!(["muthur.lan"]),
        "the record carries the Host allow-list too: {record}"
    );

    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.kill();
    let _ = child.wait();
}

/// A localhost probe can tell a deliberately started daemon from one an agent's
/// connection spawned, and can see what it actually bound. Without this, a
/// health check passes while the endpoint an operator configured does not exist
/// - the exact shape that hid the 2026-09-10 outage.
///
/// One daemon, one state directory, no reuse: nothing here has to wait for a
/// departing holder to let the lock go.
#[test]
fn health_and_status_say_how_the_daemon_started_and_what_it_bound() {
    let env = Env::new("expose");
    env.setup_domain("eng");

    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut child = serve
        .args([
            "serve",
            "--http",
            &addr,
            "--allowed-host",
            "muthur.lan",
            "--config",
        ])
        .arg(env.config_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_port(&addr);
    std::thread::sleep(Duration::from_millis(300));

    let body: Value = reqwest::blocking::Client::new()
        .get(format!("http://{addr}/health"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(body["status"], "ok", "{body}");
    assert_eq!(body["started_by"], "serve", "{body}");
    assert_eq!(body["http"], addr, "{body}");
    // The allow-list is the one exposure fact this body must not carry: the
    // probe route is never Host-guarded, so anything on it is readable by any
    // unauthenticated caller that can reach the port, and these are internal
    // hostnames. It rides the local socket instead, asserted below.
    assert!(
        body.get("allowed_hosts").is_none(),
        "an unguarded probe does not publish the Host allow-list: {body}"
    );

    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "{out}");
    let ctl: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(ctl["started_by"], "serve", "{ctl}");
    assert_eq!(ctl["http"], addr, "{ctl}");
    assert_eq!(
        ctl["allowed_hosts"],
        serde_json::json!(["muthur.lan"]),
        "{ctl}"
    );

    let (ok, human) = env.run(&["status"]);
    assert!(ok, "{human}");
    assert!(
        human.contains("started by serve") && human.contains(&addr),
        "the human status names both: {human}"
    );

    let _ = env.run(&["ctl", "shutdown"]);
    let _ = child.kill();
    let _ = child.wait();
}

/// The other half: an autostarted daemon says so, and the endpoint this test
/// environment turns off reads as off rather than as missing.
#[test]
fn an_autostarted_daemon_reports_itself_as_autostarted() {
    let env = Env::new("autoexp");
    env.setup_domain("eng");
    let client = Mcp::spawn(&env);
    env.wait_ready();

    let (ok, out) = env.run(&["ctl", "status", "--json"]);
    assert!(ok, "{out}");
    let ctl: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(ctl["started_by"], "autostart", "{ctl}");
    assert!(
        ctl["http"].is_null(),
        "the endpoint is off in this env: {ctl}"
    );

    drop(client);
    let _ = env.run(&["ctl", "shutdown"]);
}

/// A daemon told on its command line that its spawner's breakaway was
/// refused says so twice: in its report, and as a warning in its log. On
/// Windows the spawner adds the flag; here it is given by hand, which checks
/// the plumbing on every platform.
#[test]
fn a_daemon_told_its_breakaway_was_refused_says_so() {
    let env = Env::new("refused");
    env.setup_domain("eng");

    let stderr_path = env.dir.join("serve.stderr");
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let mut daemon = cmd
        .args(["serve", "--daemon", "--breakaway-refused"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();
    env.wait_ready();

    let status = status_json(&env);
    assert_eq!(
        status["runs_in"]["breakaway_refused"],
        json!(true),
        "{status}"
    );
    let record = env.lock_record().unwrap();
    assert_eq!(
        record["runs_in"]["breakaway_refused"],
        json!(true),
        "service.json carries it too: {record}"
    );
    let log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    assert!(
        log.contains("could not leave the job of the program that started it"),
        "the warning reaches the daemon's log: {log}"
    );

    let (ok, out) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown: {out}");
    let _ = daemon.wait();
}

/// Issue #115: a daemon a client starts works in the state directory, never
/// in the directory the client was started in. Windows locks a running
/// process's working directory, so a daemon that kept its client's one
/// blocked any rename of that folder (an app update) for as long as it ran.
/// Unix never locks it, so here the proof is the directory the daemon
/// reports; the rename is checked all the same, and is the real test on
/// Windows (service_windows.rs).
///
/// `work` is a sibling of the state directory, never an ancestor: the daemon
/// does hold its state directory, on purpose.
#[test]
fn an_autostarted_daemon_works_in_the_state_directory() {
    let env = Env::new("cwd");
    env.setup_domain("eng");
    let work = env.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();

    // No daemon answers yet, so the one inspected below is the client's
    // autostart.
    let (running, out) = env.run(&["ctl", "status", "--json"]);
    assert!(!running, "no daemon before the client starts one: {out}");

    let client = Mcp::spawn_in(&env, &work, None);
    env.wait_ready();

    let status = status_json(&env);
    let runs_in = &status["runs_in"];
    let working_dir = runs_in["working_dir"]
        .as_str()
        .unwrap_or_else(|| panic!("no working_dir: {status}"));
    assert_eq!(
        dunce::canonicalize(working_dir).unwrap(),
        dunce::canonicalize(env.state_dir()).unwrap(),
        "the daemon works in the state directory, not the client's: {status}"
    );
    for key in ["in_job", "job_allows_breakaway", "package"] {
        assert!(
            runs_in.get(key).is_some_and(Value::is_null),
            "{key} is present and null off Windows: {status}"
        );
    }
    assert_eq!(runs_in["breakaway_refused"], json!(false), "{status}");
    let record = env.lock_record().unwrap();
    assert_eq!(
        record["runs_in"], *runs_in,
        "service.json carries the same facts: {record}"
    );

    let (ok, human) = env.run(&["status"]);
    assert!(ok, "{human}");
    let line = human
        .lines()
        .find(|l| l.starts_with("Runs in: "))
        .unwrap_or_else(|| panic!("status names where the daemon runs: {human}"));
    assert_eq!(
        line,
        format!("Runs in: {working_dir}"),
        "the same directory as the JSON report: {human}"
    );

    let (_, doctor) = env.run(&["doctor"]);
    assert!(
        doctor.contains("daemon working directory: "),
        "doctor names it in full: {doctor}"
    );
    assert!(
        !doctor.contains("daemon job:"),
        "and has no Windows lines off Windows: {doctor}"
    );

    // The client pins its own working directory: end it first.
    drop(client);
    std::fs::rename(&work, env.dir.join("work-moved"))
        .expect("nothing holds the client's directory any more");

    let (ok, out) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown: {out}");
    wait_lock_released(&env);
}

/// The other half of the move: a relative `--db` given to the client still
/// names the file in the client's directory, not one in the daemon's. Green
/// before the working directory moved too (the daemon then shared the
/// client's directory); it guards the pair.
#[test]
fn a_relative_db_still_reaches_the_file_the_client_meant() {
    let env = Env::new("reldb");
    env.setup_domain("eng");
    let work = env.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();

    let client = Mcp::spawn_in(&env, &work, Some("rel.db"));
    env.wait_ready();

    let status = status_json(&env);
    assert!(
        work.join("rel.db").is_file(),
        "the index opened where the client was: {status}"
    );
    assert!(
        !env.state_dir().join("rel.db").exists(),
        "and not in the daemon's own working directory: {status}"
    );

    drop(client);
    let (ok, out) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown: {out}");
    wait_lock_released(&env);
}

/// The daemon route for the rows of a domain nobody registers any more: they
/// are reported, and `--fix` collects them, through the daemon that owns the
/// index. Nothing is stopped to do it - which is the whole difference between
/// this and the orphan *file* rows doctor also reports, where a removal is a
/// write the daemon's read verb cannot make.
///
/// The 0.17.0 shape is reproduced by a config edit rather than by `domain
/// remove`, which clears the rows itself: a test built on the removal would
/// assert on an index with no orphan in it.
#[test]
fn doctor_collects_orphaned_rows_through_the_running_daemon() {
    let env = Env::new("orphanrows");
    env.setup_domain("eng");
    env.setup_domain("retired");
    let mut cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    cfg.domains.shift_remove("retired");
    config::save_yaml(&env.config_path(), &cfg).unwrap();

    // The daemon starts after the edit, so the removed domain is in no tier of
    // its registered set: not the startup snapshot, not the file.
    let client = Mcp::spawn(&env);
    env.wait_ready();

    let (_, out) = env.run(&["--json", "doctor"]);
    let report: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(
        report["index"]["source"], "daemon",
        "the daemon owns the index and answered: {report}"
    );
    let rows = &report["orphaned_rows"]["domains"];
    assert_eq!(rows[0]["name"], "retired", "the orphan is named: {report}");
    assert!(
        rows[0]["engrams"].as_i64().unwrap() >= 2,
        "with the rows at stake: {report}"
    );
    assert_eq!(
        rows[0]["collected"], false,
        "and a look removes nothing: {report}"
    );

    let (_, out) = env.run(&["--json", "doctor", "--fix"]);
    let report: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(
        report["orphaned_rows"]["domains"][0]["collected"], true,
        "the daemon that owns the index does the collecting: {report}"
    );

    let (_, out) = env.run(&["--json", "doctor"]);
    let report: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(
        report["orphaned_rows"]["domains"],
        json!([]),
        "and the next look has nothing left to report: {report}"
    );

    drop(client);
    let _ = env.run(&["ctl", "shutdown"]);
}
