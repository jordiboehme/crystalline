//! CLI commands run inside an app package (Claude Desktop's MSIX package on
//! Windows), through the two debug seams: `CRYSTALLINE_TEST_PACKAGE` says
//! "this process is inside an app package" and `CRYSTALLINE_TEST_DAEMON_TASK`
//! stands in for Task Scheduler. Windows sends every new file such a process
//! creates under AppData to the package's private copy, so a packaged
//! command sends its writes to the daemon or refuses (issue 153). No seam
//! can simulate the redirection itself; these tests pin that the client
//! process writes nothing.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

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

/// How long a packaged command may run: the task wait (15 s) and the
/// command's own work, with room to spare on a slow CI machine.
const COMMAND_LIMIT: Duration = Duration::from_secs(90);

/// How long the command's stdout and stderr may stay open after it exited.
const PIPE_LIMIT: Duration = Duration::from_secs(10);

/// Run `cmd` as `output()` does, but bounded, so a hang fails the test with
/// its cause instead of running into the harness timeout. The two hangs it
/// tells apart: the command itself did not finish, or it finished and a
/// process it started (the daemon the `serve` seam starts) still holds its
/// output pipes, which on Windows is what an inherited handle does. Either
/// way the daemon under `home` is stopped first, which also lets the readers
/// end.
fn bounded(cmd: &mut Command, home: &Path) -> Output {
    fn reader(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            let _ = send.send(bytes);
        });
        receive
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = reader(child.stdout.take().unwrap());
    let stderr = reader(child.stderr.take().unwrap());
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > COMMAND_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            stop_daemon(home);
            panic!("the packaged command did not finish within {COMMAND_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let collect = |pipe: mpsc::Receiver<Vec<u8>>, name: &str| {
        pipe.recv_timeout(PIPE_LIMIT).unwrap_or_else(|_| {
            stop_daemon(home);
            panic!(
                "the packaged command exited ({status}), but its {name} stayed open for \
                 {PIPE_LIMIT:?}: a process it started holds the pipe"
            )
        })
    };
    let stdout = collect(stdout, "stdout");
    let stderr = collect(stderr, "stderr");
    Output {
        status,
        stdout,
        stderr,
    }
}

/// Stops the daemon under a home when a test ends, failed or not, so a
/// failing test leaves no daemon running.
struct StopsDaemon<'a>(&'a Path);

impl Drop for StopsDaemon<'_> {
    fn drop(&mut self) {
        stop_daemon(self.0);
    }
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
    let _stops = StopsDaemon(home.path());
    let out = bounded(
        crystalline(home.path(), true, "serve").args(["config", "set", "github.poll_secs", "120"]),
        home.path(),
    );
    let answered = daemon_answers(home.path());
    let search = bounded(
        crystalline(home.path(), true, "serve").args(["search", "anything", "--json"]),
        home.path(),
    );
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

/// A folder with a MANIFEST under `parent`, ready for `domain add`.
fn domain_folder(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntitle: kb\ntype: manifest\nstatus: stable\n---\n# kb\n",
    )
    .unwrap();
    dir
}

/// A packaged `domain add` registers through the daemon: the file domain
/// with a relative `--path` (sent as an absolute one) and a virtual domain,
/// both served by the daemon afterwards. The client runs fenced, so it
/// could not have written the config itself.
#[test]
fn a_packaged_domain_add_registers_through_the_daemon() {
    let home = short_home();
    // Declared before the guard, so the daemon stops before the folder
    // it serves is removed.
    let work = tempfile::tempdir().unwrap();
    let _stops = StopsDaemon(home.path());
    domain_folder(work.path(), "kb");
    let file = bounded(
        crystalline(home.path(), true, "serve")
            .current_dir(work.path())
            .args(["domain", "add", "kb", "--path", "kb", "--json"]),
        home.path(),
    );
    let virtual_add = bounded(
        crystalline(home.path(), true, "serve").args([
            "domain",
            "add",
            "notes",
            "--virtual",
            "--json",
        ]),
        home.path(),
    );
    // A later setting through the daemon keeps the registration: the
    // daemon writes the file it reads again, never a copy from its start.
    let setting = bounded(
        crystalline(home.path(), true, "serve").args(["config", "set", "github.poll_secs", "120"]),
        home.path(),
    );
    let status = crystalline(home.path(), false, "missing")
        .args(["ctl", "status", "--json"])
        .output()
        .unwrap();
    stop_daemon(home.path());
    assert!(file.status.success(), "{}", text(&file));
    assert!(setting.status.success(), "{}", text(&setting));
    assert!(virtual_add.status.success(), "{}", text(&virtual_add));
    let config: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&config_path(home.path())).unwrap();
    let canonical = std::fs::canonicalize(work.path().join("kb")).unwrap();
    let kb = &config.domains["kb"];
    assert_eq!(
        std::fs::canonicalize(kb.file_path().unwrap()).unwrap(),
        canonical,
        "the absolute folder was registered: {config:?}"
    );
    assert!(config.domains["notes"].is_virtual(), "{config:?}");
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let registered = status["registered"].to_string();
    assert!(registered.contains("\"kb\""), "{status}");
    assert!(registered.contains("\"notes\""), "{status}");
}

/// The daemon keeps the CLI's rule: a folder without a MANIFEST is refused
/// naming `domain init`, never scaffolded.
#[test]
fn a_packaged_domain_add_still_needs_a_manifest() {
    let home = short_home();
    // Declared before the guard, so the daemon stops before the folder
    // it serves is removed.
    let work = tempfile::tempdir().unwrap();
    let _stops = StopsDaemon(home.path());
    std::fs::create_dir_all(work.path().join("bare")).unwrap();
    let out = bounded(
        crystalline(home.path(), true, "serve")
            .current_dir(work.path())
            .args(["domain", "add", "bare", "--path", "bare"]),
        home.path(),
    );
    stop_daemon(home.path());
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no MANIFEST.md"),
        "{}",
        text(&out)
    );
    assert!(!work.path().join("bare").join("MANIFEST.md").exists());
}

/// Without a daemon a packaged `domain add` refuses and registers nothing.
#[test]
fn a_packaged_domain_add_without_a_daemon_registers_nothing() {
    let home = short_home();
    let work = tempfile::tempdir().unwrap();
    let dir = domain_folder(work.path(), "kb");
    let before = snapshot_state(home.path());
    for args in [
        vec!["domain", "add", "kb", "--path", dir.to_str().unwrap()],
        vec!["domain", "add", "notes", "--virtual"],
    ] {
        let out = crystalline(home.path(), true, "missing")
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("outside Claude Desktop"),
            "{args:?}: {}",
            text(&out)
        );
    }
    assert_eq!(before, snapshot_state(home.path()));
}

/// The terminal setup steps refuse inside a package and name the terminal:
/// each writes Crystalline's state on this machine.
#[test]
fn setup_steps_refuse_inside_a_package() {
    let work = tempfile::tempdir().unwrap();
    let key = work.path().join("app.pem");
    std::fs::write(
        &key,
        "-----BEGIN RSA PRIVATE KEY-----\n-----END RSA PRIVATE KEY-----\n",
    )
    .unwrap();
    let key = key.display().to_string();
    for args in [
        &[
            "github",
            "app",
            "set",
            "--app-id",
            "1",
            "--client-id",
            "Iv23test",
            "--private-key-file",
            key.as_str(),
        ][..],
        &["github", "app", "remove"][..],
        &["connect", "https://kb.example.com"][..],
        &["connect", "github"][..],
        &["disconnect", "kb"][..],
        &["install", "claude-code"][..],
        &["uninstall", "claude-code"][..],
        &["daemon-task", "register"][..],
        &["doctor", "--fix"][..],
        &["doctor", "--discard-rename"][..],
        &["users", "list"][..],
        &["kept", "drop", "k1", "--yes"][..],
        &["model", "download"][..],
    ] {
        let home = short_home();
        let before = snapshot_state(home.path());
        let out = crystalline(home.path(), true, "missing")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", text(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("in a terminal outside Claude Desktop"),
            "{args:?}: {}",
            text(&out)
        );
        assert_eq!(before, snapshot_state(home.path()), "{args:?}");
    }
}

/// A second state in Claude Desktop's package, the way a packaged writer
/// left it before 0.30.0: a config that registers `domains`.
fn plant_private_state(home: &Path, domains: &[&str]) -> PathBuf {
    let folder = home
        .join("local")
        .join("Packages")
        .join("Claude_pzs8sxrjxfjjc")
        .join("LocalCache")
        .join("Roaming")
        .join("crystalline");
    std::fs::create_dir_all(&folder).unwrap();
    let mut yaml = String::from("domains:\n");
    for name in domains {
        yaml.push_str(&format!("  {name}:\n    path: /nonexistent/{name}\n"));
    }
    std::fs::write(folder.join("config.yaml"), yaml).unwrap();
    folder
}

/// A packaged `status` says it reads the package's private copy first and
/// names the private state's domains the daemon does not serve.
#[test]
fn a_packaged_status_names_what_the_daemon_does_not_serve() {
    let home = short_home();
    let _stops = StopsDaemon(home.path());
    plant_private_state(home.path(), &["notes", "work"]);
    let out = bounded(
        crystalline(home.path(), true, "serve").args(["status"]),
        home.path(),
    );
    let json = bounded(
        crystalline(home.path(), true, "serve").args(["status", "--json"]),
        home.path(),
    );
    stop_daemon(home.path());
    assert!(out.status.success(), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("runs inside Claude Desktop's app package"),
        "{stdout}"
    );
    assert!(
        stdout.contains("registers 2 domains the daemon does not serve (notes, work)"),
        "{stdout}"
    );
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        json["desktop_states"][0]["unserved"],
        serde_json::json!(["notes", "work"]),
        "{json}"
    );
    assert!(json["inside_package"].is_string(), "{json}");
}

/// A packaged `doctor` says the same, as a warning first and the
/// comparison as a problem.
#[test]
fn a_packaged_doctor_says_it_reads_the_private_copy() {
    let home = short_home();
    let _stops = StopsDaemon(home.path());
    plant_private_state(home.path(), &["notes"]);
    let out = bounded(
        crystalline(home.path(), true, "serve").args(["doctor"]),
        home.path(),
    );
    stop_daemon(home.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[warning] This command runs inside Claude Desktop's app package"),
        "{}",
        text(&out)
    );
    assert!(
        stdout.contains("[problem] Claude Desktop's private state registers 1 domain the daemon does not serve (notes)"),
        "{}",
        text(&out)
    );
}

/// `prompt system` inside a package reads the daemon's configuration; with
/// no daemon it falls back to its own read and says where that came from.
#[test]
fn a_packaged_routing_block_comes_from_the_daemon_or_says_it_does_not() {
    let home = short_home();
    // Declared before the guard, so the daemon stops before the folder
    // it serves is removed.
    let work = tempfile::tempdir().unwrap();
    let _stops = StopsDaemon(home.path());
    domain_folder(work.path(), "kb");
    let config = config_path(home.path());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!(
            "domains:\n  kb:\n    path: {}\n",
            work.path()
                .join("kb")
                .display()
                .to_string()
                .replace('\\', "/")
        ),
    )
    .unwrap();
    let alone = crystalline(home.path(), true, "missing")
        .args(["prompt", "system"])
        .output()
        .unwrap();
    let served = bounded(
        crystalline(home.path(), true, "serve").args(["prompt", "system"]),
        home.path(),
    );
    stop_daemon(home.path());
    assert!(alone.status.success(), "{}", text(&alone));
    let alone_text = String::from_utf8_lossy(&alone.stdout);
    assert!(alone_text.contains("kb"), "{alone_text}");
    assert!(
        alone_text.contains("did not give its configuration"),
        "{alone_text}"
    );
    assert!(served.status.success(), "{}", text(&served));
    let served_text = String::from_utf8_lossy(&served.stdout);
    assert!(served_text.contains("kb"), "{served_text}");
    assert!(
        !served_text.contains("did not give its configuration"),
        "{served_text}"
    );
}

/// The trigger is the process's own package identity, never the launcher's
/// variables: the extension's launcher sets them on the process it starts,
/// also on macOS, where nothing is redirected. With only them set and no
/// package, `domain add` takes the ordinary local path and `serve` is not
/// refused.
#[test]
fn the_launcher_variables_alone_do_not_make_a_process_packaged() {
    let home = short_home();
    let work = tempfile::tempdir().unwrap();
    let dir = domain_folder(work.path(), "kb");
    let out = crystalline(home.path(), false, "missing")
        .env("CRYSTALLINE_CHANNEL", "desktop")
        .env("CRYSTALLINE_LAUNCHER_VERSION", "0.30.0")
        .args(["domain", "add", "kb", "--path"])
        .arg(&dir)
        .arg("--no-sync")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let config = std::fs::read_to_string(config_path(home.path())).unwrap();
    assert!(config.contains("kb:"), "registered locally: {config}");
    assert!(!daemon_answers(home.path()), "no daemon was asked to start");
}

/// `doctor` inside a package with no daemon is still a report, the command
/// a person runs to find out why: it says it runs inside the package and
/// why the index was not read, and never stops with an error.
#[test]
fn a_packaged_doctor_without_a_daemon_still_reports() {
    let home = short_home();
    let before = snapshot_state(home.path());
    let out = crystalline(home.path(), true, "missing")
        .args(["doctor"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("Error:"), "{}", text(&out));
    assert!(stdout.contains("inside Claude Desktop:"), "{}", text(&out));
    assert_eq!(before, snapshot_state(home.path()));
}

/// The daemon's machine-wide `hooks/maintenance.json` is not a per-session
/// hook file: a packaged Stop hook would create a private copy that shadows
/// it. So the hook leaves it as it is (the stamps go to the daemon when one
/// answers), while its own session file is still kept.
#[test]
fn a_packaged_stop_hook_leaves_the_maintenance_file_alone() {
    let home = short_home();
    let work = tempfile::tempdir().unwrap();
    let config = config_path(home.path());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "domains:\n  test:\n    path: /nonexistent/test\n").unwrap();
    let hooks = crate::common::isolated_state_dir(home.path()).join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let maintenance = hooks.join("maintenance.json");
    std::fs::write(&maintenance, b"{\"v\":1}").unwrap();
    let before = std::fs::read(&maintenance).unwrap();
    let transcript = work.path().join("t.jsonl");
    std::fs::write(&transcript, "{}\n".repeat(25)).unwrap();
    let payload = serde_json::json!({
        "session_id": "packaged-maintenance",
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
    assert!(
        hooks.join("packaged-maintenance.json").is_file(),
        "{}",
        text(&out)
    );
    assert_eq!(
        std::fs::read(&maintenance).unwrap(),
        before,
        "the maintenance file is untouched"
    );
}

/// `domain add` with a `--config` outside AppData inside a package would
/// register and then fail its sync (no local engine): refused up front,
/// unless `--no-sync` asks for the registration alone.
#[test]
fn a_packaged_domain_add_with_an_override_and_a_sync_refuses_up_front() {
    let home = short_home();
    let work = tempfile::tempdir().unwrap();
    let dir = domain_folder(work.path(), "kb");
    let other = work.path().join("other.yaml");
    let out = crystalline(home.path(), true, "missing")
        .args(["domain", "add", "kb", "--path"])
        .arg(&dir)
        .arg("--config")
        .arg(&other)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--no-sync"),
        "{}",
        text(&out)
    );
    assert!(!other.exists(), "nothing was registered");
    let alone = crystalline(home.path(), true, "missing")
        .args(["domain", "add", "kb", "--no-sync", "--path"])
        .arg(&dir)
        .arg("--config")
        .arg(&other)
        .output()
        .unwrap();
    assert!(alone.status.success(), "{}", text(&alone));
    assert!(other.is_file());
}

/// A daemon that accepts the hook's connection and never answers must not
/// hold the Stop hook: the stamp is given up after a short wait and the
/// hook finishes well inside its budget.
#[cfg(unix)]
#[test]
fn a_packaged_stop_hook_does_not_wait_on_a_silent_daemon() {
    let home = short_home();
    let work = tempfile::tempdir().unwrap();
    let config = config_path(home.path());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "domains:\n  test:\n    path: /nonexistent/test\n").unwrap();
    let state = crate::common::isolated_state_dir(home.path());
    std::fs::create_dir_all(&state).unwrap();
    // Accepts every connection and keeps it open without a word.
    let listener = std::os::unix::net::UnixListener::bind(state.join("service.sock")).unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    let transcript = work.path().join("t.jsonl");
    std::fs::write(&transcript, "{}\n".repeat(25)).unwrap();
    let payload = serde_json::json!({
        "session_id": "packaged-silent",
        "transcript_path": transcript.display().to_string(),
        "hook_event_name": "Stop",
    });
    let started = std::time::Instant::now();
    let mut child = crystalline(home.path(), true, "missing")
        .args(["hook", "stop"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload.to_string().as_bytes()).unwrap();
    }
    let deadline = started + std::time::Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the Stop hook still waits on a daemon that never answers");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
}
