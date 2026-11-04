//! A newer `crystalline mcp` started by an older Claude Desktop extension
//! rebuilds the extension at its own version and opens it in Claude
//! Desktop, once per version, without making the session wait.
//!
//! Every bridge here is `mcp --embedded` (no daemon is started) with the
//! launcher's environment (`CRYSTALLINE_CHANNEL=desktop`,
//! `CRYSTALLINE_LAUNCHER_VERSION`). The open goes through the debug build's
//! seam (`CRYSTALLINE_TEST_DESKTOP_OPEN`), so no test opens Claude Desktop.
//! The decision matrix (older, same and newer launcher, record or none,
//! offered already) is unit tested in `src/desktop_update.rs`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The launcher version the planted extension carries: older than any
/// release.
const OLD_LAUNCHER: &str = "0.1.0";

/// The line the bridge logs when the offer failed.
const FAILED: &str = "could not offer the Claude Desktop extension update to Crystalline";

/// The line an info log shows when this version was offered already.
const OFFERED_ALREADY: &str = "was offered already";

const BINARY_NAME: &str = if cfg!(windows) {
    "crystalline.exe"
} else {
    "crystalline"
};

/// A temp home with a record of an extension built by an older Crystalline,
/// pinning a fake crystalline under `<home>/pinned`.
struct Setup {
    home: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let s = Setup { home };
        let pinned = s.pinned();
        std::fs::create_dir_all(pinned.parent().unwrap()).unwrap();
        std::fs::write(&pinned, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&pinned, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let record = json!({
            "format": 1,
            "extension": {
                "version": OLD_LAUNCHER,
                "binary": pinned.display().to_string(),
                "shim_sha256": "ab".repeat(32),
                "file": s.bundle().display().to_string(),
                "built_at": "2026-01-01T00:00:00Z",
                "opened": true,
            },
            "offered": OLD_LAUNCHER,
        });
        std::fs::create_dir_all(s.state()).unwrap();
        std::fs::write(s.record_file(), format!("{record:#}\n")).unwrap();
        s
    }

    fn pinned(&self) -> PathBuf {
        self.home.path().join("pinned").join(BINARY_NAME)
    }

    fn state(&self) -> PathBuf {
        crate::common::isolated_state_dir(self.home.path())
    }

    fn bundle(&self) -> PathBuf {
        self.state().join("claude-desktop").join("crystalline.mcpb")
    }

    fn record_file(&self) -> PathBuf {
        self.state().join("claude-desktop.json")
    }

    fn record(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.record_file()).unwrap()).unwrap()
    }

    fn open_log(&self) -> PathBuf {
        self.home.path().join("open.log")
    }

    fn open_lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.open_log())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// `crystalline mcp --embedded` as the old extension's launcher starts
    /// it, with the open seam `seam` and `RUST_LOG` set to `log`.
    fn bridge(&self, seam: &str, log: Option<&str>) -> Bridge {
        let mut cmd = crate::common::crystalline_std();
        for (name, value) in crate::common::isolation_env(self.home.path()) {
            cmd.env(name, value);
        }
        cmd.env_remove("RUST_LOG");
        // An empty PATH: the rebuilt extension pins the recorded binary, not
        // whatever PATH or this test binary would give.
        let empty = self.home.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        cmd.env("PATH", &empty)
            .env("CRYSTALLINE_SERVICE_HTTP", "false")
            .env("CRYSTALLINE_CHANNEL", "desktop")
            .env("CRYSTALLINE_LAUNCHER_VERSION", OLD_LAUNCHER)
            .env("CRYSTALLINE_TEST_DESKTOP_OPEN", seam);
        if let Some(log) = log {
            cmd.env("RUST_LOG", log);
        }
        cmd.args(["mcp", "--embedded"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take();
        let out = BufReader::new(child.stdout.take().unwrap());
        let errors = Arc::new(Mutex::new(String::new()));
        let mut stderr = child.stderr.take().unwrap();
        let sink = Arc::clone(&errors);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&chunk[..n]));
            }
        });
        Bridge {
            child,
            stdin,
            out,
            errors,
        }
    }
}

struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
    errors: Arc<Mutex<String>>,
}

impl Bridge {
    /// Send `initialize` and read its answer.
    fn initialize(&mut self) -> Value {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": { "name": "claude-ai", "version": "1" } } })
        )
        .unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"))
    }

    /// The stderr lines that contain `text`, once at least one does.
    fn wait_for_log(&self, text: &str) -> Vec<String> {
        wait_until(&format!("a log line with '{text}'"), || {
            let errors = self.errors.lock().unwrap();
            let found: Vec<String> = errors
                .lines()
                .filter(|line| line.contains(text))
                .map(str::to_string)
                .collect();
            (!found.is_empty()).then_some(found)
        })
    }

    /// Close stdin and wait for the bridge to exit. Panics when it does not
    /// within ten seconds.
    fn finish(mut self) -> String {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                // Let the stderr reader take the last bytes.
                std::thread::sleep(Duration::from_millis(100));
                return self.errors.lock().unwrap().clone();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        panic!("the bridge did not exit after stdin closed");
    }
}

/// Poll `check` until it answers, for up to sixty seconds.
fn wait_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn manifest(zip_file: &Path) -> Value {
    let bytes = std::fs::read(zip_file).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut file = archive.by_name("manifest.json").unwrap();
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    serde_json::from_slice(&out).unwrap()
}

fn log_seam(s: &Setup) -> String {
    format!("log:{}", s.open_log().display())
}

#[test]
fn a_newer_bridge_offers_the_extension_update_once_per_version() {
    let s = Setup::new();
    let mut first = s.bridge(&log_seam(&s), None);
    assert!(first.initialize()["result"].is_object());
    let record = wait_until("the offered version in the record", || {
        let record = s.record();
        (record["offered"] == VERSION).then_some(record)
    });
    first.finish();

    let bundle = s.bundle();
    assert_eq!(
        s.open_lines(),
        vec![format!("open {}", bundle.display())],
        "Claude Desktop was asked to open the rebuilt extension"
    );
    let built = manifest(&bundle);
    assert_eq!(built["version"], VERSION, "the update carries this version");
    assert_eq!(
        built["server"]["mcp_config"]["env"]["CRYSTALLINE_BINARY"],
        s.pinned().display().to_string(),
        "the update keeps the recorded binary"
    );
    let ext = &record["extension"];
    assert_eq!(ext["version"], VERSION);
    assert_eq!(ext["opened"], true);
    assert_eq!(ext["binary"], s.pinned().display().to_string());

    // A second start at the same version offers nothing.
    let mut second = s.bridge(&log_seam(&s), Some("crystalline=info"));
    assert!(second.initialize()["result"].is_object());
    second.wait_for_log(OFFERED_ALREADY);
    second.finish();
    assert_eq!(s.open_lines().len(), 1, "{:?}", s.open_lines());
    assert_eq!(
        s.record(),
        record,
        "the record is as the first start left it"
    );
}

#[test]
fn the_session_answers_while_the_offer_is_still_held() {
    let s = Setup::new();
    let before = std::fs::read(s.record_file()).unwrap();
    let mut bridge = s.bridge(&format!("hold:{}", s.open_log().display()), None);
    // The offer is built and its open is held: the release file is never
    // written, so the answer below comes while the offer is in flight.
    let mut held = s.open_log().into_os_string();
    held.push(".held");
    let held = PathBuf::from(held);
    wait_until("the held open", || held.exists().then_some(()));
    let opened = bridge.initialize();
    assert!(opened["result"].is_object(), "{opened}");
    assert!(!s.open_log().exists(), "the open is still held");
    // A held offer does not keep the process alive once the client left.
    bridge.finish();
    assert_eq!(
        std::fs::read(s.record_file()).unwrap(),
        before,
        "an offer that never finished changes no record"
    );
}

#[test]
fn a_failed_open_logs_one_line_and_changes_no_record() {
    let s = Setup::new();
    let before = std::fs::read(s.record_file()).unwrap();
    let mut bridge = s.bridge("fail", None);
    assert!(bridge.initialize()["result"].is_object());
    let lines = bridge.wait_for_log(FAILED);
    let expected =
        format!("{FAILED} {VERSION}: Claude Desktop did not open it: the test seam refused it");
    assert!(lines[0].ends_with(&expected), "{lines:?}");
    let errors = bridge.finish();
    assert_eq!(
        errors.lines().filter(|line| line.contains(FAILED)).count(),
        1,
        "{errors}"
    );
    assert_eq!(std::fs::read(s.record_file()).unwrap(), before);
}

#[test]
fn a_failed_build_logs_one_line_and_changes_no_record() {
    let s = Setup::new();
    let before = std::fs::read(s.record_file()).unwrap();
    // A file where the extension's folder goes: the build cannot write.
    std::fs::write(s.state().join("claude-desktop"), "in the way").unwrap();
    let mut bridge = s.bridge(&log_seam(&s), None);
    assert!(bridge.initialize()["result"].is_object());
    bridge.wait_for_log(FAILED);
    let errors = bridge.finish();
    assert_eq!(
        errors.lines().filter(|line| line.contains(FAILED)).count(),
        1,
        "{errors}"
    );
    assert_eq!(std::fs::read(s.record_file()).unwrap(), before);
    assert!(!s.open_log().exists(), "a failed build opens nothing");
}
