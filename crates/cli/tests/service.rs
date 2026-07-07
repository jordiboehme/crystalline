//! Integration tests for the daemon, spawning the real `crystalline` binary.
//!
//! Each test runs in an isolated, deliberately short temp HOME (so the unix
//! socket path stays under the platform limit). etcetera uses the XDG strategy
//! on Linux and macOS, so setting the XDG_*_HOME variables redirects the state,
//! config and cache directories. These tests are unix-only: they use unix domain
//! sockets and `kill -9` for the stale-lock scenario.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crystalline_core::config::{self, GlobalConfig};
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

    fn apply(&self, cmd: &mut Command) {
        cmd.env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_CACHE_HOME", self.dir.join("cache"));
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.join("state/crystalline")
    }
    fn lock_path(&self) -> PathBuf {
        self.state_dir().join("service.lock")
    }
    fn sock_path(&self) -> PathBuf {
        self.state_dir().join("service.sock")
    }
    fn config_path(&self) -> PathBuf {
        self.dir.join("config/crystalline/config.yaml")
    }

    /// Create a domain directory with a MANIFEST and a seed engram, then register
    /// it in the config. Config selection rides on the isolated `XDG_CONFIG_HOME`
    /// (`apply` below), never an explicit `--config`: the default config path
    /// already resolves to `config_path()`, and an explicit override would mean
    /// "bypass the daemon" (see `crystalline_service::use_daemon`), which would
    /// wrongly force the direct index path and collide with a running daemon's
    /// lock. Leaving it off lets `domain add` route its sync through a live
    /// daemon exactly as a plain invocation does.
    fn setup_domain(&self, name: &str) {
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
            String::from_utf8_lossy(&out.stdout).into_owned(),
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

    fn lock_pid(&self) -> Option<u64> {
        let text = std::fs::read_to_string(self.lock_path()).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        v.get("pid").and_then(Value::as_u64)
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
        Mcp::spawn_inner(env, false, &[])
    }

    /// Spawn an `mcp` client that starts a read-only daemon when none is running.
    fn spawn_read_only(env: &Env) -> Mcp {
        Mcp::spawn_inner(env, true, &[])
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

    /// Spawn an `mcp` client with `--domain` folders: the Claude Desktop MCPB
    /// bundle entry point. Each folder is registered as a domain (created and
    /// scaffolded with a MANIFEST.md when it does not exist yet) before the
    /// daemon attach-or-spawn decision.
    fn spawn_with_domains(env: &Env, domains: &[PathBuf]) -> Mcp {
        Mcp::spawn_inner(env, false, domains)
    }

    fn spawn_inner(env: &Env, read_only: bool, domains: &[PathBuf]) -> Mcp {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.arg("mcp");
        if read_only {
            cmd.arg("--read-only");
        }
        for d in domains {
            cmd.arg("--domain").arg(d);
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

/// End to end: a daemon started read-only reports it over ctl status, hides
/// the four content-mutating tools from tools/list and refuses a write call by
/// name with the read-only error.
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

    // tools/list hides the four content-mutating tools and keeps the eight reads.
    let names = c1.list_tools();
    assert_eq!(names.len(), 8, "read-only exposes 8 tools: {names:?}");
    for hidden in [
        "write_engram",
        "edit_engram",
        "move_engram",
        "delete_engram",
    ] {
        assert!(
            !names.contains(&hidden.to_string()),
            "{hidden} hidden: {names:?}"
        );
    }

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

/// `crystalline mcp --domain <path>` is the entry point Claude Desktop MCPB
/// bundles use: a user-picked folder is registered as a domain (creating it
/// and scaffolding a MANIFEST.md when it does not exist yet) before the
/// daemon attach-or-spawn decision, so it is watched and listed from the very
/// first launch. A restart with the same folder must be a cheap no-op: no
/// duplicate registration, and the server still answers `list_domains`.
#[test]
fn mcp_domain_flag_registers_folder_and_is_idempotent_across_restarts() {
    let env = Env::new("mcpdom");
    let knowledge = env.dir.join("knowledge");
    assert!(!knowledge.exists(), "the folder must not pre-exist");
    let domains = vec![knowledge.clone()];

    // First launch: the folder does not exist on disk yet.
    let mut c1 = Mcp::spawn_with_domains(&env, &domains);
    c1.initialize();
    env.wait_ready();

    c1.send_call("list_domains", json!({}));
    let r = c1.read_tool_value();
    let names = domain_names(&r);
    assert!(
        names.contains(&"knowledge".to_string()),
        "domain 'knowledge' listed: {r}"
    );

    assert!(
        knowledge.join("MANIFEST.md").exists(),
        "MANIFEST.md scaffolded for the bundle's folder"
    );
    let cfg: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key("knowledge"),
        "config.yaml registers 'knowledge': {cfg:?}"
    );
    assert_eq!(cfg.domains.len(), 1, "exactly one domain registered");

    // Shut the daemon down cleanly and wait for the lock and socket to clear
    // before the second launch, so it does not race a stale-lock takeover.
    drop(c1);
    let (ok, _) = env.run(&["ctl", "shutdown"]);
    assert!(ok, "ctl shutdown");
    let start = Instant::now();
    while (env.sock_path().exists() || env.lock_path().exists())
        && start.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!env.sock_path().exists(), "socket removed on shutdown");
    assert!(!env.lock_path().exists(), "lock removed on shutdown");

    // Second launch with the same flag: no duplicate registration, and the
    // server still answers list_domains.
    let mut c2 = Mcp::spawn_with_domains(&env, &domains);
    c2.initialize();
    env.wait_ready();

    c2.send_call("list_domains", json!({}));
    let r2 = c2.read_tool_value();
    let names2 = domain_names(&r2);
    assert!(
        names2.contains(&"knowledge".to_string()),
        "domain 'knowledge' still listed after restart: {r2}"
    );

    let cfg2: GlobalConfig = config::load_yaml(&env.config_path()).unwrap();
    assert_eq!(
        cfg2.domains.len(),
        1,
        "restart with the same --domain flag did not duplicate the registration: {cfg2:?}"
    );

    drop(c2);
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
    // The 12 core tools plus `configure`: GitHub collaboration is off by
    // default, so the other five collaboration tools stay hidden (see
    // crystalline-service's mcp_collab test suite for the full gating matrix).
    assert_eq!(tools.len(), 13, "13 tools over HTTP");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"configure"), "{names:?}");
    assert!(!names.contains(&"add_domain"), "{names:?}");

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

fn wait_port(addr: &str) {
    let start = Instant::now();
    loop {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        if start.elapsed() > Duration::from_secs(8) {
            panic!("HTTP endpoint did not open on {addr}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `crystalline healthcheck` against a port nothing is listening on: the
/// connection is refused immediately, so this needs no daemon spawn and no
/// wait, unlike the success path piggybacked on the HTTP smoke test above.
#[test]
fn healthcheck_against_nothing_exits_nonzero() {
    let port = free_port();
    let status = Command::new(bin())
        .args(["healthcheck", &format!("127.0.0.1:{port}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "healthcheck against a closed port exits nonzero"
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

    let (ok, out) = env.run(&["status", "--json"]);
    assert!(ok, "{out}");
    let value: Value = serde_json::from_str(out.trim()).expect("one JSON object");
    assert!(value["pid"].as_u64().is_some(), "{value}");
    assert!(value["domains"].is_array(), "{value}");
}

/// Without a daemon, `status` says so in its first line and renders the same
/// human shape from a direct index read; a registered domain that was never
/// synced shows as not indexed yet rather than disappearing.
#[test]
fn status_without_a_daemon_says_not_running() {
    let env = Env::new("status-down");

    let (ok, out) = env.run(&["status"]);
    assert!(ok, "{out}");
    assert!(
        out.starts_with("Daemon: not running; reading the index directly"),
        "{out}"
    );
    assert!(out.contains("No index at "), "{out}");
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
