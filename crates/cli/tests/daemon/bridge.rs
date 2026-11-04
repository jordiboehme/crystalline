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

/// When Claude Desktop's package keeps a second state that registers
/// domains, the bridge says so in the onboarding instructions: the person
/// is in the chat, not in a terminal.
#[test]
fn a_packaged_bridge_names_a_split_state_in_the_instructions() {
    let home = short_home();
    let folder = home
        .path()
        .join("local")
        .join("Packages")
        .join("Claude_pzs8sxrjxfjjc")
        .join("LocalCache")
        .join("Roaming")
        .join("crystalline");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("config.yaml"),
        "domains:\n  notes:\n    path: /nonexistent/notes\n",
    )
    .unwrap();
    let mut bridge = Bridge::start(home.path(), "serve");
    let opened = bridge.open();
    bridge.finish();
    let mut stop = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
    for (name, value) in crate::common::isolation_env(home.path()) {
        stop.env(name, value);
    }
    let _ = stop.args(["ctl", "shutdown"]).output();
    let instructions = opened["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("Claude Desktop keeps a second Crystalline state"),
        "{instructions}"
    );
    assert!(
        instructions.contains("--merge-desktop-state"),
        "{instructions}"
    );
    assert!(
        instructions.contains("1 domain the Crystalline daemon does not serve (notes)"),
        "{instructions}"
    );
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
    let without_json = run(&[], &["status", "--json"]);
    bridge.finish();
    let _ = run(&[], &["ctl", "shutdown"]);

    let said = String::from_utf8_lossy(&with_variable.stderr);
    // The sign-in task is Windows' own; elsewhere the line names no task.
    let expected = if cfg!(windows) {
        "note: The daemon serves no HTTP endpoint, not 127.0.0.1:7499 from CRYSTALLINE_SERVICE_HTTP: a daemon the sign-in task started reads the system environment. Set CRYSTALLINE_SERVICE_HTTP as a user environment variable, or stop the daemon (`crystalline ctl shutdown`) and start it from this shell."
    } else {
        "note: The daemon serves no HTTP endpoint, not 127.0.0.1:7499 from CRYSTALLINE_SERVICE_HTTP: the daemon was started without this shell's environment. Stop it (`crystalline ctl shutdown`) and start it from this shell, or set CRYSTALLINE_SERVICE_HTTP where it starts."
    };
    assert!(said.contains(expected), "{said}");
    let json: Value = serde_json::from_slice(&as_json.stdout).unwrap();
    assert_eq!(
        json["config_mismatch"][0]["variable"], "CRYSTALLINE_SERVICE_HTTP",
        "{json}"
    );
    assert!(
        !String::from_utf8_lossy(&without.stderr).contains("The daemon serves"),
        "nothing set here, nothing said"
    );
    let quiet: Value = serde_json::from_slice(&without_json.stdout).unwrap();
    assert!(
        quiet.get("config_mismatch").is_none(),
        "no mismatch, no key, as doctor --json: {quiet}"
    );
}

/// The doctor half of the same line: `check_service` asks the holder and
/// compares it with what this shell would have started, so a variable set
/// here that the daemon did not get is a warning in the service section, and
/// with `--json` a `config_mismatch` row. A shell that sets none says
/// nothing.
#[test]
fn doctor_names_the_variable_a_task_started_daemon_did_not_get() {
    let home = short_home();
    // HTTP off in the file, as in the status twin above.
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
        for name in crystalline_service::shaping::shaping_set_here() {
            cmd.env_remove(name);
        }
        for (name, value) in extra {
            cmd.env(name, value);
        }
        cmd.args(args).output().unwrap()
    };
    let shell = [("CRYSTALLINE_SERVICE_HTTP", "127.0.0.1:7499")];
    let with_variable = run(&shell, &["doctor"]);
    let as_json = run(&shell, &["--json", "doctor"]);
    let without = run(&[], &["doctor"]);
    bridge.finish();
    let _ = run(&[], &["ctl", "shutdown"]);

    let said = String::from_utf8_lossy(&with_variable.stdout);
    let expected = if cfg!(windows) {
        "  [warning] The daemon serves no HTTP endpoint, not 127.0.0.1:7499 from CRYSTALLINE_SERVICE_HTTP: a daemon the sign-in task started reads the system environment. Set CRYSTALLINE_SERVICE_HTTP as a user environment variable, or stop the daemon (`crystalline ctl shutdown`) and start it from this shell."
    } else {
        "  [warning] The daemon serves no HTTP endpoint, not 127.0.0.1:7499 from CRYSTALLINE_SERVICE_HTTP: the daemon was started without this shell's environment. Stop it (`crystalline ctl shutdown`) and start it from this shell, or set CRYSTALLINE_SERVICE_HTTP where it starts."
    };
    assert!(said.contains(expected), "{said}");
    let json: Value = serde_json::from_slice(&as_json.stdout).unwrap();
    assert_eq!(
        json["service"]["config_mismatch"][0]["variable"], "CRYSTALLINE_SERVICE_HTTP",
        "{json}"
    );
    assert!(
        !String::from_utf8_lossy(&without.stdout).contains("The daemon serves"),
        "nothing set here, nothing said"
    );
}

/// The note a too old Claude Desktop extension launcher gets in the
/// onboarding instructions (`LAUNCHER_NOTE` in the service crate's bridge).
const LAUNCHER_NOTE: &str = "The Claude Desktop extension that starts Crystalline is older than this Crystalline supports. Ask the user to run `crystalline install claude-desktop` in a terminal, then confirm the install dialog in Claude Desktop.";

/// Stop the daemon the `serve` task seam started under `home`.
fn stop_daemon(home: &Path) {
    let mut stop = Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
    for (name, value) in crate::common::isolation_env(home) {
        stop.env(name, value);
    }
    let _ = stop.args(["ctl", "shutdown"]).output();
}

/// The `server/discover` opener of a 2026-07-28 client, with both keys rmcp
/// requires in `_meta`.
fn discover() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": { "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {} } } })
}

/// The instructions of the answer to `opener` from a bridge started by a
/// launcher of `version` on the `desktop` channel.
fn launcher_instructions(version: &str, opener: Value) -> String {
    let home = short_home();
    let mut bridge = Bridge::start_with(
        home.path(),
        "serve",
        &[("CRYSTALLINE_LAUNCHER_VERSION", version)],
    );
    let answer = bridge.ask(opener);
    let instructions = answer["result"]["instructions"]
        .as_str()
        .unwrap_or_else(|| panic!("no instructions: {answer}"))
        .to_string();
    bridge.finish();
    stop_daemon(home.path());
    instructions
}

#[test]
fn an_old_launcher_is_named_in_the_initialize_instructions() {
    let old = launcher_instructions("0.23.0", initialize());
    assert!(old.ends_with(&format!("\n\n{LAUNCHER_NOTE}")), "{old}");
    let current = launcher_instructions("0.25.1", initialize());
    assert!(!current.contains(LAUNCHER_NOTE), "{current}");
}

#[test]
fn an_old_launcher_is_named_in_the_discover_instructions() {
    let old = launcher_instructions("0.23.0", discover());
    assert!(old.ends_with(&format!("\n\n{LAUNCHER_NOTE}")), "{old}");
    let current = launcher_instructions("0.25.1", discover());
    assert!(!current.contains(LAUNCHER_NOTE), "{current}");
}

/// The Claude Desktop extension update for a bridge inside Claude Desktop's
/// app package: the bridge writes nothing and asks the daemon (started
/// through the `serve` task seam, outside the package) to offer it. The
/// open goes through the debug build's seam, so these run in debug builds
/// on macOS and Windows only.
#[cfg(all(debug_assertions, any(target_os = "macos", windows)))]
mod desktop_offer {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use super::{Bridge, LAUNCHER_NOTE, initialize, short_home, stop_daemon};

    const VERSION: &str = env!("CARGO_PKG_VERSION");

    /// The launcher note once the update to this version was offered.
    const OFFERED_NOTE: &str = "The Claude Desktop extension that starts Crystalline is older than this Crystalline supports. Crystalline offered the extension update in Claude Desktop. Ask the user to confirm the update dialog there. If no dialog shows, ask them to run `crystalline install claude-desktop` in a terminal.";

    const OLD_BUNDLE: &str = "the extension an older Crystalline built";

    fn state(home: &Path) -> PathBuf {
        crate::common::isolated_state_dir(home)
    }

    fn bundle(home: &Path) -> PathBuf {
        state(home).join("claude-desktop").join("crystalline.mcpb")
    }

    fn record_file(home: &Path) -> PathBuf {
        state(home).join("claude-desktop.json")
    }

    /// A record of an extension an older Crystalline built, and its file.
    fn plant(home: &Path) {
        let bundle = bundle(home);
        std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
        std::fs::write(&bundle, OLD_BUNDLE).unwrap();
        let record = json!({
            "format": 1,
            "extension": {
                "version": "0.1.0",
                "binary": "/nowhere/crystalline",
                "shim_sha256": "ab".repeat(32),
                "file": bundle.display().to_string(),
                "built_at": "2026-01-01T00:00:00Z",
                "opened": true,
            },
            "offered": "0.1.0",
        });
        std::fs::write(record_file(home), format!("{record:#}\n")).unwrap();
    }

    fn record(home: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(record_file(home)).unwrap()).unwrap()
    }

    /// Stops the daemon the `serve` task seam started under its home when
    /// it drops, so a failed assert leaves nothing running either.
    struct StopsDaemon<'a>(&'a Path);

    impl Drop for StopsDaemon<'_> {
        fn drop(&mut self) {
            stop_daemon(self.0);
        }
    }

    fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The instructions a packaged bridge with an old launcher and the open
    /// seam `seam` answers `initialize` with.
    fn instructions(home: &Path, seam: &str) -> String {
        let mut bridge = Bridge::start_with(
            home,
            "serve",
            &[
                ("CRYSTALLINE_LAUNCHER_VERSION", "0.1.0"),
                ("CRYSTALLINE_TEST_DESKTOP_OPEN", seam),
            ],
        );
        let answer = bridge.ask(initialize());
        bridge.finish();
        answer["result"]["instructions"]
            .as_str()
            .unwrap_or_else(|| panic!("no instructions: {answer}"))
            .to_string()
    }

    #[test]
    fn a_packaged_bridge_has_the_daemon_offer_the_update_once() {
        let home = short_home();
        plant(home.path());
        let _stops = StopsDaemon(home.path());
        let log = home.path().join("open.log");
        let seam = format!("log:{}", log.display());

        let first = instructions(home.path(), &seam);
        assert!(
            first.ends_with(&format!("\n\n{LAUNCHER_NOTE}")),
            "an offer still in flight is not called offered: {first}"
        );
        wait_until("the offered version in the record", || {
            record(home.path())["offered"] == VERSION
        });
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("open {}\n", bundle(home.path()).display()),
            "the daemon opened the rebuilt extension"
        );
        assert_eq!(record(home.path())["extension"]["version"], VERSION);

        let second = instructions(home.path(), &seam);
        assert!(second.ends_with(&format!("\n\n{OFFERED_NOTE}")), "{second}");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().lines().count(),
            1,
            "the second start offers nothing"
        );
    }

    #[test]
    fn a_failed_open_in_the_daemon_keeps_the_record_and_the_note() {
        let home = short_home();
        plant(home.path());
        let _stops = StopsDaemon(home.path());
        let before = std::fs::read(record_file(home.path())).unwrap();

        // The daemon logs to nothing here, so the seam says when it refused.
        let refused = home.path().join("refused.log");
        let seam = format!("refuse:{}", refused.display());
        let first = instructions(home.path(), &seam);
        assert!(first.ends_with(&format!("\n\n{LAUNCHER_NOTE}")), "{first}");
        wait_until("the refused open", || refused.exists());
        wait_until("the old bundle back in its place", || {
            std::fs::read_to_string(bundle(home.path())).is_ok_and(|text| text == OLD_BUNDLE)
        });
        let aside = bundle(home.path()).with_extension("mcpb.previous");
        assert!(!aside.exists(), "nothing is left aside");
        assert_eq!(std::fs::read(record_file(home.path())).unwrap(), before);

        let second = instructions(home.path(), &seam);
        assert!(
            second.ends_with(&format!("\n\n{LAUNCHER_NOTE}")) && !second.contains(OFFERED_NOTE),
            "a failed open is never called offered: {second}"
        );
    }
}
