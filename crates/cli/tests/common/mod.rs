//! Shared helpers for the CLI integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// The repo-root `tests/fixtures` directory, shared across milestones.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// The base-directory variables that redirect every child this crate can
/// spawn into an isolated `home`, so its accounts, config, index and state
/// never land in the developer's own directories - plus one credential
/// variable that redirects a different kind of shared state entirely.
///
/// Both base-directory families are needed, because base-directory
/// resolution (via etcetera's `choose_base_strategy`) is a different
/// strategy per platform: the XDG one on unix and macOS, which reads `HOME`
/// and `XDG_*_HOME`, and the Windows one, which reads `USERPROFILE`,
/// `APPDATA` and `LOCALAPPDATA` and ignores the XDG variables entirely. On
/// Windows it also has no state directory of its own, so `state_dir` falls
/// back to the data directory, `APPDATA`. Setting only the XDG variables
/// would leave a Windows run resolving one real `%APPDATA%\crystalline` and
/// colliding with every other test doing the same (Windows byte-range locks
/// are mandatory). Setting the Windows names on unix as well is harmless
/// there, so one array covers both platforms without a `cfg`.
///
/// `CRYSTALLINE_TEST_NO_KEYCHAIN` is not a base directory at all: it is
/// `crystalline_remote::token`'s test seam
/// (`crystalline_remote::token::refuse_real_keychain`), checked inside
/// `TokenStore::resolve_and_load_for`/`save_resolving_for` - the choke point
/// every credential touch in the CLI and the engine funnels through - and it
/// is a pure kill switch: set, it refuses the real OS keychain and falls
/// back to the file store at whatever directory the caller already resolved
/// (`origins_state_dir()` under this test's own isolated `XDG_STATE_HOME`,
/// so a fixture a test wrote there is still where the CLI looks). It is
/// deliberately NOT `CRYSTALLINE_TEST_TOKEN_STORE_DIR`
/// (`crystalline::cmd::test_token_store_dir`), which redirects `connect
/// github` to a directory of ITS OWN choosing: setting that one here too
/// once broke `disconnecting_a_personal_identity_forgets_its_credential`,
/// whose fixture lives at the isolated state dir, not at a second, unrelated
/// one this array would have invented.
///
/// None of `HOME`/`XDG_STATE_HOME`/etc. reach the keychain on their own -
/// the OS keychain service name is a hardcoded constant, not derived from
/// any base directory - so a child that only got the directories above
/// still asks the real login keychain for a `github` credential the moment
/// it reaches `origin update`/`status`, `doctor`'s GitHub section, `connect
/// github --disconnect` or `users disable`/`remove`'s personal-credential
/// sweep. Every test in this crate that calls [`isolate`] or iterates this
/// array now gets the credential seam for free, which is the only way any
/// of the four call sites above stop touching the real keychain from a
/// test: none of them takes a `--token-store-dir` flag of its own.
///
/// `CRYSTALLINE_TEST_DAEMON_TASK=missing` is the debug build's stand-in for
/// Task Scheduler (`crystalline_service::daemon_task::for_this_process`).
/// Without it a Windows child whose daemon spawn is refused its breakaway
/// asks the real Task Scheduler, and on a machine with the MSI installed it
/// starts the real daemon for the real user. A test that wants another
/// answer sets the variable after [`isolate`], which wins.
///
/// `HF_ENDPOINT` points the model downloader at [`NO_HUB`], so a child that
/// would fetch the embedding or contradiction model (about 220 MB and 550 MB,
/// into this test's empty cache) fails the fetch at once and goes on
/// text-only, as it does offline. See [`NO_HUB`].
pub fn isolation_env(home: &Path) -> [(&'static str, PathBuf); 10] {
    [
        ("HOME", home.to_path_buf()),
        ("XDG_CONFIG_HOME", home.join("config")),
        ("XDG_STATE_HOME", home.join("state")),
        ("XDG_CACHE_HOME", home.join("cache")),
        ("USERPROFILE", home.to_path_buf()),
        ("APPDATA", home.join("roaming")),
        ("LOCALAPPDATA", home.join("local")),
        ("CRYSTALLINE_TEST_NO_KEYCHAIN", PathBuf::from("1")),
        ("CRYSTALLINE_TEST_DAEMON_TASK", PathBuf::from("missing")),
        (HUB_ENDPOINT_VAR, PathBuf::from(NO_HUB)),
    ]
}

/// The variable the model downloader (hf-hub) reads its hub address from.
pub const HUB_ENDPOINT_VAR: &str = "HF_ENDPOINT";

/// A hub address nothing listens on: port 1 on loopback refuses the connection
/// at once. Every test daemon used to download the embedding model into its own
/// fresh cache, which added network and CPU load to every daemon test. A refused
/// fetch is what an offline machine sees: the daemon warns, keeps serving and
/// searches text-only. A test that wants a download in flight names the real
/// hub itself (`Env::downloading` in `daemon/service.rs`).
pub const NO_HUB: &str = "http://127.0.0.1:1";

/// Apply [`isolation_env`] to a child `Command`.
pub fn isolate(cmd: &mut Command, home: &Path) {
    for (name, value) in isolation_env(home) {
        cmd.env(name, value);
    }
    // A child logs by the filter the test gives it, never by the runner's.
    cmd.env_remove("RUST_LOG");
    // A models cache named in the developer's shell would win over the
    // scratch cache above, and `doctor --fix` removes lock folders there.
    cmd.env_remove("CRYSTALLINE_MODELS_DIR");
}

/// The name in [`isolation_env`] that governs where a child isolated by
/// [`isolate`] keeps its state directory, and the folder that variable points
/// at.
///
/// There is one per platform because base-directory resolution is one strategy
/// per platform (see [`isolation_env`]): the XDG strategy reads
/// `XDG_STATE_HOME`, and the Windows strategy ignores it, has no state
/// directory of its own and falls back to the data directory, `APPDATA`.
/// Naming the pair together is what keeps [`isolated_state_dir`] honest: a
/// fixture is placed under the same variable the binary will resolve through,
/// so the two can never be edited apart.
pub const STATE_HOME_VAR: &str = if cfg!(windows) {
    "APPDATA"
} else {
    "XDG_STATE_HOME"
};

/// Where a child isolated by [`isolate`] keeps its state, the directory
/// `crystalline_core::config::state_dir` resolves to under that environment.
///
/// A test that plants a fixture the binary is meant to find - an overlay
/// journal draft, a receipt - has to write it here rather than spell
/// `<home>/state/crystalline` by hand, which is the unix answer on every
/// platform and silently the wrong folder on Windows.
pub fn isolated_state_dir(home: &Path) -> PathBuf {
    let base = isolation_env(home)
        .into_iter()
        .find(|(name, _)| *name == STATE_HOME_VAR)
        .map(|(_, dir)| dir)
        .expect("the isolation environment sets this platform's state-home variable");
    base.join("crystalline")
}

/// The default home every command built by [`crystalline`] and
/// [`crystalline_std`] runs under, one per test process.
///
/// A test that does not isolate its child itself (most of them pass explicit
/// `--db` and `--config` paths and never think about it) would otherwise fall
/// through to the developer's real `~/.local/state/crystalline`, opening and
/// migrating the real index. The directory is created lazily under the system
/// temp dir and lives for the whole process; a test that sets its own
/// variables afterwards simply overrides these.
pub fn default_home() -> &'static Path {
    static HOME: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        // Statics are never dropped, so the directory is kept explicitly; it
        // is empty until a child writes into it and the OS temp cleaner owns
        // it from here.
        tempfile::Builder::new()
            .prefix("crystalline-cli-test-home-")
            .tempdir()
            .expect("create the default test home")
            .keep()
    });
    &HOME
}

/// A `crystalline` command already isolated under [`default_home`].
pub fn crystalline() -> Command {
    let mut cmd = Command::cargo_bin("crystalline").expect("the crystalline binary is built");
    for (name, value) in isolation_env(default_home()) {
        cmd.env(name, value);
    }
    // A models cache named in the developer's shell would win over the
    // scratch cache above, and `doctor --fix` removes lock folders there.
    cmd.env_remove("CRYSTALLINE_MODELS_DIR");
    cmd
}

/// The same as [`crystalline`], as a plain `std::process::Command`.
pub fn crystalline_std() -> std::process::Command {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
    for (name, value) in isolation_env(default_home()) {
        cmd.env(name, value);
    }
    // A models cache named in the developer's shell would win over the
    // scratch cache above, and `doctor --fix` removes lock folders there.
    cmd.env_remove("CRYSTALLINE_MODELS_DIR");
    cmd
}

/// A process found by [`processes_under`]: its pid and its command line.
pub type Process = (u32, String);

/// Every live process whose environment or command line names `dir`, as
/// `(pid, command line)`: on macOS read from `ps -E` (which shows the
/// environment of this user's own processes), on Linux from `/proc`. A zombie
/// is not counted, since it is already gone and only waits to be reaped, and
/// neither is this test process.
///
/// A test daemon is found this way rather than by the pid in its owner record
/// (`service.json`): the record names only the daemon that published it, and
/// a successor a bridge spawned has not published one yet, or published it
/// after the test read it (see [`Leftovers`]). Every child a test isolates
/// carries `HOME` (or `XDG_STATE_HOME`) under its own folder, and so does
/// everything those children start.
///
/// `None` when the scan could not run at all (on macOS `ps` could not be
/// started, as when the per-user process limit is reached under a loaded
/// run), so a caller never reads a failed scan as "nothing left". One retry
/// every 100 ms for up to 5 s comes first.
#[cfg(unix)]
pub fn processes_under(dir: &Path) -> Option<Vec<Process>> {
    let start = std::time::Instant::now();
    loop {
        if let Some(found) = scan_under(dir) {
            return Some(found);
        }
        if start.elapsed() > std::time::Duration::from_secs(5) {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// One attempt of [`processes_under`].
#[cfg(unix)]
fn scan_under(dir: &Path) -> Option<Vec<Process>> {
    // The folder as given and as the kernel spells it (`/tmp` is
    // `/private/tmp` on macOS, and a daemon is handed absolute paths).
    let mut needles = vec![dir.to_string_lossy().into_owned()];
    if let Ok(real) = std::fs::canonicalize(dir) {
        let real = real.to_string_lossy().into_owned();
        if real != needles[0] {
            needles.push(real);
        }
    }
    let names_it = |text: &str| {
        needles.iter().any(|needle| {
            text.match_indices(needle.as_str()).any(|(at, _)| {
                matches!(
                    text[at + needle.len()..].chars().next(),
                    None | Some('/') | Some(' ') | Some('\0')
                )
            })
        })
    };
    let me = std::process::id();
    let mut found = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return None;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == me {
                continue;
            }
            let base = entry.path();
            let zombie = std::fs::read_to_string(base.join("stat"))
                .ok()
                .and_then(|stat| {
                    stat.rsplit_once(')')
                        .map(|(_, rest)| rest.trim_start().starts_with('Z'))
                })
                .unwrap_or(true);
            if zombie {
                continue;
            }
            let read = |name: &str| {
                std::fs::read(base.join(name))
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default()
            };
            let cmdline = read("cmdline");
            if names_it(&read("environ")) || names_it(&cmdline) {
                found.push((pid, cmdline.replace('\0', " ").trim_end().to_string()));
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-axww", "-E", "-o", "pid=,stat=,command="])
            .output()
        else {
            return None;
        };
        if !out.status.success() {
            return None;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut fields = line.split_whitespace();
            let (Some(pid), Some(stat)) = (fields.next(), fields.next()) else {
                continue;
            };
            let Ok(pid) = pid.parse::<u32>() else {
                continue;
            };
            if pid == me || stat.starts_with('Z') || !names_it(line) {
                continue;
            }
            // `-E` appends the environment to the command line; the command
            // ends at the first `NAME=value` word.
            let command: Vec<&str> = fields
                .take_while(|word| {
                    !word.split_once('=').is_some_and(|(name, _)| {
                        !name.is_empty()
                            && name
                                .chars()
                                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                    })
                })
                .collect();
            found.push((pid, command.join(" ")));
        }
    }
    Some(found)
}

/// What [`stop_processes_under`] met: what the first scan found, what was
/// still alive at the end, and whether every scan ran (`false` when one could
/// not, see [`processes_under`]).
#[cfg(unix)]
pub struct Stopped {
    pub found: Vec<Process>,
    pub left: Vec<Process>,
    pub scanned: bool,
}

/// Stop every process [`processes_under`] finds for `dir` with `SIGKILL`, and
/// wait until none is left, for at most `within`. Each lap kills again what it
/// finds, so a successor that was being spawned on the first lap is caught on
/// a later one.
///
/// When a scan cannot run, the pid in the owner record under `dir` (the old
/// way, `state/crystalline/service.json`) is killed instead, so this is never
/// weaker than that, and the result says the scan failed.
#[cfg(unix)]
pub fn stop_processes_under(dir: &Path, within: std::time::Duration) -> Stopped {
    let start = std::time::Instant::now();
    let Some(first) = processes_under(dir) else {
        kill_record_pid(dir);
        return Stopped {
            found: Vec::new(),
            left: Vec::new(),
            scanned: false,
        };
    };
    let mut left = first.clone();
    let mut quiet_laps = 0;
    loop {
        if left.is_empty() {
            quiet_laps += 1;
            // Two empty scans 100 ms apart: one is enough for a process that
            // exists, the second catches one a dying parent forked just now.
            if quiet_laps >= 2 {
                break;
            }
        } else {
            quiet_laps = 0;
            for (pid, _) in &left {
                kill_hard(*pid);
            }
        }
        if start.elapsed() > within {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(if left.is_empty() {
            100
        } else {
            50
        }));
        match processes_under(dir) {
            Some(now) => left = now,
            None => {
                kill_record_pid(dir);
                return Stopped {
                    found: first,
                    left,
                    scanned: false,
                };
            }
        }
    }
    Stopped {
        found: first,
        left,
        scanned: true,
    }
}

/// `SIGKILL` to `pid`, never to 0 or 1 (which `kill(2)` reads as a group or
/// init) and never to this test process.
#[cfg(unix)]
fn kill_hard(pid: u32) {
    if pid > 1 && pid != std::process::id() {
        // SAFETY: `kill` only sends a signal. The pid was found under a
        // test's own folder or in its owner record a moment ago, and 0, 1 and
        // this process are refused above.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// Kill the pid the owner record under `dir` names, the fallback when no scan
/// can run. Some tests write a fake record (another process's pid, or this
/// test's own), which [`kill_hard`] refuses for this process.
#[cfg(unix)]
fn kill_record_pid(dir: &Path) {
    let record = dir.join("state/crystalline/service.json");
    if let Some(pid) = std::fs::read_to_string(record)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("pid").and_then(serde_json::Value::as_u64))
        .and_then(|pid| u32::try_from(pid).ok())
    {
        kill_hard(pid);
    }
}

/// The guard every test folder that a daemon may run in holds: dropped, it
/// stops every process still running under the folder (a daemon a test left
/// behind, a bridge's successor, a background command), says in the test's
/// output which ones it had to stop, and fails the test when one is still
/// alive afterwards, unless the test is already failing (a panic inside a
/// drop that runs during unwinding would abort the run).
///
/// Hold it next to the folder and drop it before the folder is removed: a
/// daemon whose folder is deleted under it re-creates its state directory and
/// runs on with no test to stop it, which is how the leaked
/// `serve --daemon --autostarted` processes of 2026-11-04 came about.
#[cfg(unix)]
pub struct Leftovers(pub PathBuf);

#[cfg(unix)]
impl Leftovers {
    /// Stop now rather than at the end of the scope. Consumes the guard, so
    /// its drop does not run the stop a second time.
    pub fn stop_now(self) {
        let mut this = std::mem::ManuallyDrop::new(self);
        let dir = std::mem::take(&mut this.0);
        stop_leftovers(&dir);
    }
}

#[cfg(unix)]
impl Drop for Leftovers {
    fn drop(&mut self) {
        stop_leftovers(&self.0);
    }
}

/// The work of [`Leftovers`]: stop, report and fail as its doc says.
#[cfg(unix)]
fn stop_leftovers(dir: &Path) {
    let Stopped {
        found,
        left,
        scanned,
    } = stop_processes_under(dir, std::time::Duration::from_secs(10));
    for (pid, command) in &found {
        eprintln!(
            "note: stopped pid {pid} still running under {}: {command}",
            dir.display()
        );
    }
    if !scanned && !std::thread::panicking() {
        panic!(
            "could not look for processes under {} (the scan did not run), so only the \
             owner record's pid was stopped",
            dir.display()
        );
    }
    if !left.is_empty() && !std::thread::panicking() {
        panic!(
            "processes still running under {} after the test: {left:?}",
            dir.display()
        );
    }
}

/// A temp home whose drop first stops every process still running under it
/// ([`Leftovers`], on unix), then removes it. For a test that starts a daemon
/// in a `tempfile` home and stops it with `ctl shutdown` at its end: a failing
/// assert before that line would otherwise leave the daemon running.
pub struct GuardedHome(tempfile::TempDir);

impl GuardedHome {
    pub fn new(dir: tempfile::TempDir) -> GuardedHome {
        GuardedHome(dir)
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for GuardedHome {
    fn drop(&mut self) {
        #[cfg(unix)]
        Leftovers(self.0.path().to_path_buf()).stop_now();
    }
}
