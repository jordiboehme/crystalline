//! Windows integration tests for the daemon: the record split, the named-pipe
//! attach and the second-instance refusal, driven against the real
//! `crystalline` binary. The unix twin lives in service.rs; this file is
//! windows-only because it exercises the named pipe and Windows env isolation
//! (USERPROFILE, APPDATA, LOCALAPPDATA, which etcetera's Windows strategy
//! reads). It compiles to an empty test binary on every other platform, so the
//! coverage rides the windows-latest CI leg without touching local runs.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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

    /// Isolate a child (and any daemon it spawns, which inherits this
    /// environment) into this test's directories, with the HTTP endpoint turned
    /// off: it is on at `127.0.0.1:7411` by default and a port is the one thing
    /// a scratch directory cannot isolate, so concurrent daemons here would race
    /// each other and the runner for one real port. No test in this file wants
    /// an endpoint. The task seam answers "no task", so a spawn whose
    /// breakaway is refused never asks the real Task Scheduler, which on a
    /// machine with the MSI installed would start the real daemon.
    fn apply(&self, cmd: &mut Command) {
        cmd.env("USERPROFILE", &self.dir)
            .env("APPDATA", self.dir.join("roaming"))
            .env("LOCALAPPDATA", self.dir.join("local"))
            .env("CRYSTALLINE_SERVICE_HTTP", "false")
            .env("CRYSTALLINE_TEST_DAEMON_TASK", "missing");
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

/// Run a one-shot command in this env, returning (success, stdout).
fn run(env: &Env, args: &[&str]) -> (bool, String) {
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let out = cmd.args(args).stdin(Stdio::null()).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy_owned(out.stdout),
    )
}

/// Poll `ctl status --json` for up to 60s and return the daemon's report.
fn wait_for_ctl_status(env: &Env) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (ok, out) = run(env, &["ctl", "status", "--json"]);
        if ok && let Ok(v) = serde_json::from_str::<Value>(out.trim()) {
            return v;
        }
        assert!(Instant::now() < deadline, "no daemon answered within 60s");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stop the daemon a bridge started and wait until its record is gone. The
/// daemon is detached, so no `Reap` guard owns it and `Env::drop` cannot
/// remove a directory it still holds.
fn shutdown_daemon(env: &Env) {
    let _ = run(env, &["ctl", "shutdown"]);
    let deadline = Instant::now() + Duration::from_secs(15);
    while env.info_path().exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Two spellings of one directory compare equal: canonicalized, so an 8.3
/// short temp path and a long one meet.
fn same_dir(a: &str, b: &Path) -> bool {
    dunce::canonicalize(a).ok() == dunce::canonicalize(b).ok()
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
        cmd.arg("mcp");
        Bridge::start(cmd)
    }

    /// As [`Bridge::attach`], started in `dir` and with an optional `--db`
    /// passed exactly as written (a relative one stays relative).
    fn attach_in(env: &Env, dir: &Path, db: Option<&str>) -> Bridge {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.current_dir(dir);
        if let Some(db) = db {
            cmd.arg("--db").arg(db);
        }
        cmd.arg("mcp");
        Bridge::start(cmd)
    }

    /// As [`Bridge::attach`], with the client's stderr (its log, the one a
    /// harness keeps) written to `log` at the default `warn` level.
    fn attach_logging_to(env: &Env, log: &Path) -> Bridge {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.env("RUST_LOG", "warn").arg("mcp");
        Bridge::start_with(cmd, std::fs::File::create(log).unwrap().into())
    }

    fn start(cmd: Command) -> Bridge {
        Bridge::start_with(cmd, Stdio::null())
    }

    fn start_with(mut cmd: Command, stderr: Stdio) -> Bridge {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr);
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
/// split it could not even read who owned the lock, so it reported pid 0. It
/// must also exit on the lock's own code (3, so a unit file can set
/// RestartPreventExitStatus) and name the keys that reconcile the two
/// exposures. The unix leg of the same contract lives in service.rs, as
/// `a_serve_that_loses_the_lock_exits_three_and_says_what_was_lost`.
///
/// The second serve asks for an allow-list this env's daemon never had, which
/// is what makes the exposure advice apply: a refusal only names the keys when
/// the two sides actually differ, and both daemons here take the same
/// `CRYSTALLINE_SERVICE_HTTP=false` endpoint.
#[test]
fn a_second_serve_fails_fast_naming_the_owner() {
    let env = Env::new("win-second");
    let daemon = spawn_daemon(&env);
    let record = wait_for_record(&env);
    let owner_pid = record["pid"].as_u64().unwrap();

    let mut second = Command::new(bin());
    env.apply(&mut second);
    let out = second
        .args(["serve", "--allowed-host", "muthur.lan"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "lock loss has its own exit code, not the generic 1"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Scoped to the refusal's own line: the allow-list flag also contradicts
    // this env's configuration, so the startup notice prints the same key to
    // the same stderr before the lock is attempted.
    let refusal = stderr
        .lines()
        .find(|l| l.contains("already owns it"))
        .unwrap_or_else(|| panic!("the refusal reaches stderr: {stderr}"));
    assert!(
        refusal.contains(&owner_pid.to_string()),
        "the refusal names the live owner (pid {owner_pid}): {refusal}"
    );
    assert!(
        refusal.contains("service.http"),
        "and the key that makes every daemon here bind the same way: {refusal}"
    );
    assert!(
        refusal.contains("service.allowed_hosts muthur.lan"),
        "and the allow-list key, in the spelling the setting takes: {refusal}"
    );

    drop(daemon);
}

/// The Windows leg of the force-takeover path: a real process holds the lock
/// and publishes a record without ever binding the pipe (the wedge of the
/// 2026-07-28 incident), and `doctor --fix` verifies its identity through
/// `QueryFullProcessImageNameW` and ends it through `TerminateProcess`. The
/// unix twin of this scenario lives in service.rs; this is the only place the
/// Windows signalling path runs at all.
#[test]
fn doctor_fix_dislodges_an_unresponsive_holder() {
    let env = Env::new("win-wedge");

    let mut hold = Command::new(bin());
    env.apply(&mut hold);
    let mut holder = hold
        .args(["hold-lock", "--secs", "60"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The readiness line means the lock is taken and the record is published.
    let mut out = BufReader::new(holder.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "holding", "hold-lock did not take the lock");
    let holder_pid = u64::from(holder.id());
    let record = wait_for_record(&env);
    assert_eq!(record["pid"].as_u64(), Some(holder_pid));

    let mut doctor = Command::new(bin());
    env.apply(&mut doctor);
    let report = doctor
        .args(["--json", "doctor", "--fix"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&report.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "doctor did not print a report ({e}): {stdout}{}",
            String::from_utf8_lossy(&report.stderr)
        )
    });
    assert_eq!(
        parsed["service"]["daemon_dislodged"],
        json!(true),
        "{stdout}"
    );

    // `try_wait` rather than a liveness probe: the holder is this test's own
    // child, so it must be reaped to be seen as gone.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if holder.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the wedged holder was not dislodged"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !env.info_path().exists(),
        "the dislodged holder's stale record was cleaned up"
    );
}

/// Issue #115, the real regression test: a daemon a client started keeps no
/// hold on the client's directory. Windows locks a running process's working
/// directory, so before 0.21.1 this rename failed with a sharing violation
/// for as long as the daemon ran.
///
/// The runner itself may run this test inside a job object, so `in_job` is
/// only checked to be a bool, and the breakaway permission to be known
/// whenever there is a job. A runner process has no package identity.
#[test]
fn an_autostarted_daemon_leaves_its_clients_directory_free() {
    let env = Env::new("win-cwd");
    let work = env.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();

    let bridge = Bridge::attach_in(&env, &work, None);
    let status = wait_for_ctl_status(&env);
    let runs_in = &status["runs_in"];
    assert!(
        same_dir(
            runs_in["working_dir"].as_str().unwrap_or(""),
            &env.state_dir()
        ),
        "the daemon works in the state directory: {status}"
    );
    assert!(runs_in["in_job"].is_boolean(), "{status}");
    if runs_in["in_job"] == json!(true) {
        assert!(runs_in["job_allows_breakaway"].is_boolean(), "{status}");
    }
    assert!(runs_in["package"].is_null(), "{status}");

    // The client pins its own working directory: end it first.
    drop(bridge);
    let renamed = std::fs::rename(&work, env.dir.join("work-moved"));
    shutdown_daemon(&env);
    renamed.expect("the daemon holds no handle on the client's directory");
}

/// A relative `--db` still names the file in the client's directory.
#[test]
fn a_relative_db_still_reaches_the_file_the_client_meant_on_windows() {
    let env = Env::new("win-reldb");
    let work = env.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();

    let bridge = Bridge::attach_in(&env, &work, Some("rel.db"));
    let status = wait_for_ctl_status(&env);
    let in_work = work.join("rel.db").is_file();
    let in_state = env.state_dir().join("rel.db").exists();
    drop(bridge);
    shutdown_daemon(&env);
    assert!(in_work, "the index opened where the client was: {status}");
    assert!(
        !in_state,
        "and not in the daemon's own working directory: {status}"
    );
}

/// The refusal path end to end: this test process joins a fresh job with no
/// limit flags, so no breakaway. The client it starts is in that job, its
/// breakaway spawn is refused and the daemon starts inside the job, knows it
/// and says so. nextest runs every test in its own process, so the job ends
/// with this test (under plain `cargo test` the other tests of this binary
/// would share it; none of them asserts on jobs). Nested jobs need Windows 8
/// or later, which every runner is.
#[test]
fn a_daemon_that_cannot_leave_the_job_says_so() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let env = Env::new("win-job");
    // SAFETY: both arguments may be null (no security attributes, no name),
    // which makes a private job with default limits; the result is checked.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(
        !job.is_null(),
        "CreateJobObjectW: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: `job` is the valid handle just created and `GetCurrentProcess`
    // returns this process's pseudo handle, which never needs closing.
    let joined = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
    assert!(
        joined != 0,
        "AssignProcessToJobObject: {}",
        std::io::Error::last_os_error()
    );

    let client_log_path = env.dir.join("client.stderr");
    let bridge = Bridge::attach_logging_to(&env, &client_log_path);
    let status = wait_for_ctl_status(&env);
    let log = std::fs::read_to_string(env.state_dir().join("daemon.log")).unwrap_or_default();
    let (_, doctor) = run(&env, &["doctor"]);
    drop(bridge);
    let client_log = std::fs::read_to_string(&client_log_path).unwrap_or_default();
    shutdown_daemon(&env);
    // The job handle goes last; the job has no kill-on-close limit.
    // SAFETY: `job` is a valid handle owned by this test and closed once.
    unsafe { CloseHandle(job) };

    let runs_in = &status["runs_in"];
    assert_eq!(runs_in["in_job"], json!(true), "{status}");
    assert_eq!(runs_in["job_allows_breakaway"], json!(false), "{status}");
    assert_eq!(runs_in["breakaway_refused"], json!(true), "{status}");
    assert!(
        log.contains("could not leave the job of the program that started it"),
        "the daemon's startup warning is in daemon.log: {log}"
    );
    assert!(
        client_log.contains("Windows refused the breakaway"),
        "the client says it too, in the log its harness keeps: {client_log}"
    );
    assert!(
        doctor.contains("[warning] the daemon runs inside a job it cannot leave"),
        "doctor warns: {doctor}"
    );
}

/// The clean stop at the session end, end to end: Windows ends a windowless
/// process at sign-out without a word, so a daemon started the way the task
/// starts it has the hidden window, and WM_ENDSESSION through that window
/// runs the graceful stop to its end. What this cannot show is that Windows
/// itself sends the message at a real sign-out; that stays a check on a real
/// machine.
#[test]
fn a_task_started_daemon_stops_cleanly_when_the_session_ends() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ENDSESSION_LOGOFF, FindWindowW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_ENDSESSION,
    };

    let env = Env::new("win-session-end");
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    serve
        .env_remove("RUST_LOG")
        .env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1")
        .args(["serve", "--from-task"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut daemon = Reap(serve.spawn().unwrap());
    let state = env.state_dir();

    let deadline = Instant::now() + Duration::from_secs(60);
    let class: Vec<u16> = "CrystallineSessionEnd".encode_utf16().chain([0]).collect();
    let title: Vec<u16> = format!("Crystalline daemon {}", daemon.0.id())
        .encode_utf16()
        .chain([0])
        .collect();
    let hwnd = loop {
        // SAFETY: two NUL-terminated wide strings that live for the call.
        let found = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
        if !found.is_null() && env.info_path().is_file() {
            break found;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "the daemon left before its window appeared"
        );
        assert!(
            Instant::now() < deadline,
            "the daemon's window never appeared"
        );
        std::thread::sleep(Duration::from_millis(100));
    };

    let mut result = 0usize;
    let started = Instant::now();
    // SAFETY: a window handle FindWindowW just returned; the call waits at
    // most 8 s and returns early if the daemon's thread is gone.
    unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_ENDSESSION,
            1,
            ENDSESSION_LOGOFF as isize,
            SMTO_ABORTIFHUNG,
            8000,
            &mut result,
        )
    };
    let exit = loop {
        if let Some(status) = daemon.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the daemon did not leave"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "inside the session-end budget: {:?}",
        started.elapsed()
    );
    assert!(exit.success(), "a clean exit: {exit:?}");
    let log = std::fs::read_to_string(state.join("daemon.log")).unwrap();
    assert!(log.contains("stopping (Windows sign-out)"), "{log}");
    assert!(
        log.contains("shutdown: removing the record, the socket and the lock file"),
        "{log}"
    );
    assert!(
        !log.contains("shutdown did not finish"),
        "the watchdog was not needed: {log}"
    );
    assert!(!env.info_path().exists(), "the record is gone");
    let lock = state.join("service.lock");
    if lock.exists() {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock)
            .unwrap();
        fs4::FileExt::try_lock(&file).expect("the lock was released");
    }
}
