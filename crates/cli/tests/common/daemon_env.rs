//! One real daemon in an isolated, short-path environment, for the CLI's
//! tests that need `crystalline serve` running: the per-prompt recall and
//! session start through a daemon. Moved here from `setup/hook.rs` so more
//! than one module can use it; a module adds its own methods in an `impl`
//! block of its own.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// An isolated, short-path environment holding one daemon.
///
/// The base is `/tmp` rather than a `tempfile` directory for the reason
/// `crates/cli/tests/daemon/service.rs` gives: the daemon's unix socket path has to
/// stay inside the platform's 104-byte limit.
pub struct DaemonEnv {
    pub dir: PathBuf,
    pub serve: Option<std::process::Child>,
}

impl DaemonEnv {
    pub fn new(tag: &str) -> DaemonEnv {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = PathBuf::from("/tmp").join(format!("cqp-{tag}-{nanos}"));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        DaemonEnv { dir, serve: None }
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config/crystalline/config.yaml")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.dir.join("state/crystalline")
    }
    pub fn info_path(&self) -> PathBuf {
        self.state_dir().join("service.json")
    }
    pub fn lock_path(&self) -> PathBuf {
        self.state_dir().join("service.lock")
    }

    /// Isolate a child into this test's directories, with the HTTP endpoint
    /// off (a real port is the one thing a temp directory cannot isolate) and
    /// the real keychain refused.
    pub fn apply(&self, cmd: &mut std::process::Command) {
        cmd.env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_CACHE_HOME", self.dir.join("cache"))
            .env("CRYSTALLINE_SERVICE_HTTP", "false")
            .env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1");
    }

    /// Start the daemon against this environment's config and wait until it
    /// answers.
    pub fn serve(&mut self) {
        self.serve_with(true);
    }

    /// [`DaemonEnv::serve`] without the explicit `--config`: the daemon finds
    /// the same file through `XDG_CONFIG_HOME`, and because nothing bypasses
    /// it, it installs this machine's connected servers too.
    pub fn serve_with_sources(&mut self) {
        self.serve_with(false);
    }

    fn serve_with(&mut self, explicit_config: bool) {
        let mut cmd = crate::common::crystalline_std();
        self.apply(&mut cmd);
        cmd.arg("serve");
        if explicit_config {
            cmd.arg("--config").arg(self.config_path());
        }
        let child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        self.serve = Some(child);
        let start = Instant::now();
        while !self.run(&["ctl", "status", "--json"]).0 {
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "the daemon did not become ready"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        // What every assertion below rests on: there is a real daemon here,
        // this one, and not a command that answered by opening the index
        // itself. The hook only ever speaks to a daemon that published this
        // record.
        let text =
            std::fs::read_to_string(self.info_path()).expect("the daemon published a record");
        let record: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(record["started_by"], json!("serve"), "{record}");
        assert!(
            record["pid"].as_u64().is_some_and(|pid| pid > 0),
            "{record}"
        );
    }

    /// Run a one-shot command, returning (success, stdout).
    pub fn run(&self, args: &[&str]) -> (bool, String) {
        let mut cmd = crate::common::crystalline_std();
        self.apply(&mut cmd);
        let out = cmd.args(args).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy_owned(out.stdout),
        )
    }

    /// The daemon's status report.
    pub fn status(&self) -> Value {
        let (ok, out) = self.run(&["ctl", "status", "--json"]);
        assert!(ok, "ctl status failed: {out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("status json: {e}: {out}"))
    }

    /// Stop the daemon and wait until the lock is free, so the temp directory
    /// goes away without a live writer in it.
    pub fn shutdown(&mut self) {
        let _ = self.run(&["ctl", "shutdown"]);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(8) {
            if !self.lock_path().exists() && !self.info_path().exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Some(mut child) = self.serve.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for DaemonEnv {
    fn drop(&mut self) {
        self.shutdown();
        if let Ok(text) = std::fs::read_to_string(self.info_path())
            && let Ok(v) = serde_json::from_str::<Value>(&text)
            && let Some(pid) = v.get("pid").and_then(Value::as_u64)
        {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
