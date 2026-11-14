//! Hand a file to Claude Desktop, or show it in the file manager.
//!
//! `install claude-desktop` opens the extension it built here, so Claude
//! Desktop shows its install dialog, and a newer Crystalline opens its
//! update the same way (`desktop_offer`). Nothing in this module prints: a caller
//! gets the error text and says what to do instead.
//!
//! Debug builds read a test seam, [`TEST_DESKTOP_OPEN_ENV`], so tests never
//! open the real Claude Desktop. Release builds never read it, so a test that
//! runs `install claude-desktop` without `--no-open` must be compiled only
//! into debug builds (`#[cfg(debug_assertions)]`, as `tests/setup/main.rs`
//! does for its `install_desktop` module), or `cargo test --release` would
//! open the real app. The same holds for a test that starts a bridge or a
//! daemon which may offer the extension update. The unit tests below never
//! reach the real open: they run `/bin/sh` in its place.

use std::path::Path;

/// The debug build's stand-in for the real open. `fail` refuses every call,
/// `log:<file>` appends one line per call to `<file>` and succeeds, and
/// `hold:<file>` writes `<file>.held` and then does the same once
/// `<file>.go` exists (it gives up after a minute), so a test can
/// hold an open and watch what else goes on. `refuse:<file>` appends the
/// line too and then refuses, so a test sees when a refused open happened.
/// Empty or unset is the real open.
///
/// Only a debug build reads it. A test that relies on it must be compiled
/// only into debug builds, or a release test run opens Claude Desktop.
pub const TEST_DESKTOP_OPEN_ENV: &str = "CRYSTALLINE_TEST_DESKTOP_OPEN";

/// Opens files the way this machine does.
pub trait DesktopOpener {
    /// Open `file` with Claude Desktop.
    fn open_extension(&self, file: &Path) -> Result<(), String>;
    /// Show `file` in Finder or File Explorer.
    fn reveal(&self, file: &Path) -> Result<(), String>;
}

/// The opener this process uses: in a debug build the test seam when it is
/// set, else the real one.
pub fn for_this_process() -> Box<dyn DesktopOpener> {
    #[cfg(debug_assertions)]
    if let Some(seam) = std::env::var(TEST_DESKTOP_OPEN_ENV)
        .ok()
        .and_then(|value| Seam::parse(&value))
    {
        return Box::new(seam);
    }
    Box::new(System)
}

/// What the test seam does.
#[cfg(debug_assertions)]
#[derive(Debug, PartialEq, Eq)]
enum Seam {
    Fail,
    Log(std::path::PathBuf),
    Hold(std::path::PathBuf),
    Refuse(std::path::PathBuf),
    Unknown(String),
}

/// How long the `hold:` seam waits for its release file.
#[cfg(debug_assertions)]
const HOLD_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(debug_assertions)]
impl Seam {
    /// The seam a value names, or `None` for the real opener.
    fn parse(value: &str) -> Option<Seam> {
        if value.is_empty() {
            return None;
        }
        Some(if value == "fail" {
            Seam::Fail
        } else if let Some(file) = value.strip_prefix("log:") {
            Seam::Log(file.into())
        } else if let Some(file) = value.strip_prefix("hold:") {
            Seam::Hold(file.into())
        } else if let Some(file) = value.strip_prefix("refuse:") {
            Seam::Refuse(file.into())
        } else {
            Seam::Unknown(value.to_string())
        })
    }

    fn call(&self, verb: &str, file: &Path) -> Result<(), String> {
        use std::io::Write;
        match self {
            Seam::Fail => Err("the test seam refused it".to_string()),
            Seam::Log(log) => std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .and_then(|mut out| writeln!(out, "{verb} {}", file.display()))
                .map_err(|e| e.to_string()),
            Seam::Hold(log) => {
                let beside = |suffix: &str| {
                    let mut path = log.as_os_str().to_owned();
                    path.push(suffix);
                    std::path::PathBuf::from(path)
                };
                std::fs::write(beside(".held"), "").map_err(|e| e.to_string())?;
                let release = beside(".go");
                let deadline = std::time::Instant::now() + HOLD_LIMIT;
                while !release.exists() {
                    if std::time::Instant::now() > deadline {
                        return Err("the test seam was never released".to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Seam::Log(log.clone()).call(verb, file)
            }
            Seam::Refuse(log) => {
                Seam::Log(log.clone()).call(verb, file)?;
                Seam::Fail.call(verb, file)
            }
            Seam::Unknown(value) => Err(format!("unknown {TEST_DESKTOP_OPEN_ENV} value '{value}'")),
        }
    }
}

#[cfg(debug_assertions)]
impl DesktopOpener for Seam {
    fn open_extension(&self, file: &Path) -> Result<(), String> {
        self.call("open", file)
    }
    fn reveal(&self, file: &Path) -> Result<(), String> {
        self.call("reveal", file)
    }
}

/// The real opener.
struct System;

/// The error when the open says Claude Desktop is not there: `open -b`
/// exits 1 and `ShellExecuteW` answers `SE_ERR_NOASSOC`.
#[cfg(any(target_os = "macos", windows, test))]
const NOT_INSTALLED: &str = "Claude Desktop does not seem to be installed";

#[cfg(target_os = "macos")]
impl DesktopOpener for System {
    fn open_extension(&self, file: &Path) -> Result<(), String> {
        run_open(
            std::ffi::OsStr::new("open"),
            &["-b", "com.anthropic.claudefordesktop"],
            file,
            true,
        )
    }
    fn reveal(&self, file: &Path) -> Result<(), String> {
        run_open(std::ffi::OsStr::new("open"), &["-R"], file, false)
    }
}

/// Run `<program> <args> <file>` and wait for it. Its stdout goes nowhere
/// and its stderr into the error, so nothing reaches this process's own
/// streams: in the MCP bridge stdout is the JSON-RPC channel. With
/// `exit_1_is_missing`, exit code 1 means Claude Desktop is not there.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn run_open(
    program: &std::ffi::OsStr,
    args: &[&str],
    file: &Path,
    exit_1_is_missing: bool,
) -> Result<(), String> {
    use std::process::Stdio;
    let output = crystalline_core::thread_env::strip_from(&mut std::process::Command::new(program))
        .args(args)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("could not run open ({e})"))?;
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.lines().map(str::trim).find(|l| !l.is_empty());
    match output.status.code() {
        Some(0) => Ok(()),
        Some(1) if exit_1_is_missing => Err(NOT_INSTALLED.to_string()),
        Some(code) => Err(match said {
            Some(line) => format!("open exited with {code} ({line})"),
            None => format!("open exited with {code}"),
        }),
        None => Err("open was stopped by a signal".to_string()),
    }
}

#[cfg(windows)]
impl DesktopOpener for System {
    fn open_extension(&self, file: &Path) -> Result<(), String> {
        windows::shell_open(file.as_os_str(), None).map_err(|code| shell_error(code, true))
    }
    fn reveal(&self, file: &Path) -> Result<(), String> {
        windows::shell_open(
            std::ffi::OsStr::new("explorer.exe"),
            Some(&select_parameter(file)),
        )
        .map_err(|code| shell_error(code, false))
    }
}

/// Explorer's `/select,"<file>"`. The quotes keep a path with spaces or
/// commas in one piece; without them Explorer opens its default folder.
#[cfg(any(windows, test))]
fn select_parameter(file: &Path) -> std::ffi::OsString {
    let mut select = std::ffi::OsString::from("/select,\"");
    select.push(file);
    select.push("\"");
    select
}

/// The error for a `ShellExecuteW` result of 32 or less. When the file is
/// the extension, `SE_ERR_NOASSOC` (31) means no app opens `.mcpb` files.
#[cfg(any(windows, test))]
fn shell_error(code: isize, opening_extension: bool) -> String {
    const SE_ERR_NOASSOC: isize = 31;
    if opening_extension && code == SE_ERR_NOASSOC {
        NOT_INSTALLED.to_string()
    } else {
        format!("Windows could not open the file ({code})")
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{S_FALSE, S_OK};
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// `ShellExecuteW(null, "open", file, parameters, null, SW_SHOWNORMAL)`.
    /// COM is set up first, since the handler may be a shell extension.
    /// The error is the result code, 32 or less. A GUI call: it writes
    /// nothing to this process's stdout or stderr.
    pub(super) fn shell_open(file: &OsStr, parameters: Option<&OsStr>) -> Result<(), isize> {
        let verb = wide(OsStr::new("open"));
        let file = wide(file);
        let parameters = parameters.map(wide);
        // SAFETY: every pointer is either null or a NUL-terminated UTF-16
        // buffer that lives until the call returns.
        let code = unsafe {
            let com = CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            );
            let result = ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                parameters.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
            if com == S_OK || com == S_FALSE {
                CoUninitialize();
            }
            result as isize
        };
        // A value above 32 is success; 32 or less is an error code.
        if code > 32 { Ok(()) } else { Err(code) }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
impl DesktopOpener for System {
    fn open_extension(&self, _file: &Path) -> Result<(), String> {
        Err("Claude Desktop is not available on this system".to_string())
    }
    fn reveal(&self, _file: &Path) -> Result<(), String> {
        Err("there is no file manager to show it in on this system".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(debug_assertions)]
    #[test]
    fn the_seam_value_names_what_it_does() {
        assert_eq!(Seam::parse("fail"), Some(Seam::Fail));
        assert_eq!(
            Seam::parse("log:/tmp/open.log"),
            Some(Seam::Log("/tmp/open.log".into()))
        );
        assert_eq!(
            Seam::parse(r"log:C:\Users\me\open.log"),
            Some(Seam::Log(r"C:\Users\me\open.log".into())),
            "only the first colon separates"
        );
        assert_eq!(
            Seam::parse("hold:/tmp/open.log"),
            Some(Seam::Hold("/tmp/open.log".into()))
        );
        assert_eq!(
            Seam::parse("refuse:/tmp/open.log"),
            Some(Seam::Refuse("/tmp/open.log".into()))
        );
        assert_eq!(Seam::parse(""), None, "empty is the real opener");
        assert_eq!(Seam::parse("yes"), Some(Seam::Unknown("yes".to_string())));
    }

    /// `hold:<log>` waits for `<log>.go` before it logs, so a test can show
    /// that something else went on while the open was still held.
    #[cfg(debug_assertions)]
    #[test]
    fn the_hold_seam_logs_only_once_released() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("open.log");
        let seam = Seam::Hold(log.clone());
        let file = Path::new("/x/crystalline.mcpb");
        let held = std::thread::spawn(move || seam.open_extension(file));
        let marker = dir.path().join("open.log.held");
        while !marker.exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!held.is_finished(), "the open waits for the release file");
        assert!(!log.exists());
        std::fs::write(dir.path().join("open.log.go"), "").unwrap();
        held.join().unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("open {}\n", file.display())
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn the_seam_refuses_or_logs() {
        let file = Path::new("/x/crystalline.mcpb");
        assert_eq!(
            Seam::Fail.open_extension(file),
            Err("the test seam refused it".to_string())
        );
        assert_eq!(
            Seam::Fail.reveal(file),
            Err("the test seam refused it".to_string())
        );
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("open.log");
        let seam = Seam::Log(log.clone());
        seam.open_extension(file).unwrap();
        seam.reveal(file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("open {0}\nreveal {0}\n", file.display())
        );
        let refused = dir.path().join("refused.log");
        assert_eq!(
            Seam::Refuse(refused.clone()).open_extension(file),
            Err("the test seam refused it".to_string())
        );
        assert_eq!(
            std::fs::read_to_string(&refused).unwrap(),
            format!("open {}\n", file.display())
        );
        assert_eq!(
            Seam::Unknown("yes".to_string()).open_extension(file),
            Err(format!("unknown {TEST_DESKTOP_OPEN_ENV} value 'yes'"))
        );
    }

    /// The real opener's branches, with `/bin/sh -c <script>` in place of
    /// `open`, so no app is ever opened.
    #[cfg(unix)]
    fn sh(script: &str, exit_1_is_missing: bool) -> Result<(), String> {
        run_open(
            std::ffi::OsStr::new("/bin/sh"),
            &["-c", script, "stub"],
            Path::new("/x/crystalline.mcpb"),
            exit_1_is_missing,
        )
    }

    #[cfg(unix)]
    #[test]
    fn the_real_opener_reads_the_exit_status() {
        assert_eq!(sh("exit 0", true), Ok(()));
        assert_eq!(
            sh(r#"test "$1" = /x/crystalline.mcpb"#, false),
            Ok(()),
            "the file is the last argument"
        );
        assert_eq!(
            sh("echo 'no such app' >&2; exit 1", true),
            Err("Claude Desktop does not seem to be installed".to_string())
        );
        assert_eq!(
            sh("exit 1", false),
            Err("open exited with 1".to_string()),
            "exit 1 of a reveal keeps its code"
        );
        assert_eq!(
            sh("printf '\\n  it went wrong  \\nmore\\n' >&2; exit 2", true),
            Err("open exited with 2 (it went wrong)".to_string())
        );
        assert_eq!(
            sh("kill -9 $$", true),
            Err("open was stopped by a signal".to_string())
        );
        let missing = run_open(
            std::ffi::OsStr::new("/nonexistent/open"),
            &[],
            Path::new("/x"),
            true,
        )
        .unwrap_err();
        assert!(missing.starts_with("could not run open ("), "{missing}");
    }

    /// The env variable naming the script [`noisy_open_in_a_child`] runs.
    #[cfg(unix)]
    const NOISY_SCRIPT_ENV: &str = "DESKTOP_OPEN_NOISY_SCRIPT";

    /// Run by [`the_real_opener_writes_nothing_to_our_streams`] in a child
    /// test process, whose streams that test reads.
    #[cfg(unix)]
    #[test]
    #[ignore = "run in a child process by the_real_opener_writes_nothing_to_our_streams"]
    fn noisy_open_in_a_child() {
        let script = std::env::var(NOISY_SCRIPT_ENV).expect("set by the parent test");
        println!("result: {:?}", sh(&script, true));
    }

    #[cfg(unix)]
    #[test]
    fn the_real_opener_writes_nothing_to_our_streams() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "desktop_open::tests::noisy_open_in_a_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(NOISY_SCRIPT_ENV, "echo STUB-OUT; echo STUB-ERR >&2; exit 3")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(
            stdout.contains(r#"result: Err("open exited with 3 (STUB-ERR)")"#),
            "the child ran the stub: {stdout}"
        );
        assert!(!stdout.contains("STUB-OUT"), "{stdout}");
        assert!(!stderr.contains("STUB-OUT"), "{stderr}");
        assert!(!stderr.contains("STUB-ERR"), "{stderr}");
        assert_eq!(
            stdout.matches("STUB-ERR").count(),
            1,
            "stderr reached us only inside the error: {stdout}"
        );
    }

    #[test]
    fn explorer_gets_the_file_in_quotes() {
        assert_eq!(
            select_parameter(Path::new(r"C:\Users\Ana Lopez\a,b.zip")),
            std::ffi::OsString::from(r#"/select,"C:\Users\Ana Lopez\a,b.zip""#)
        );
    }

    #[test]
    fn a_missing_association_says_claude_desktop_is_not_there() {
        assert_eq!(
            shell_error(31, true),
            "Claude Desktop does not seem to be installed"
        );
        assert_eq!(
            shell_error(31, false),
            "Windows could not open the file (31)"
        );
        assert_eq!(shell_error(2, true), "Windows could not open the file (2)");
    }

    /// A release build must never read the seam, or a set variable would
    /// stop a real install from opening Claude Desktop. Every place this
    /// crate's production code names the variable is in this file, and
    /// every read of it sits under `#[cfg(debug_assertions)]`.
    #[test]
    fn only_a_debug_build_reads_the_seam() {
        let source = include_str!("desktop_open.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .expect("the file has production code");
        let reads: Vec<usize> = production
            .match_indices("std::env::var")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(reads.len(), 1, "one read of the environment");
        let before = &production[..reads[0]];
        let gate = before
            .rfind("#[cfg(debug_assertions)]")
            .expect("the read is gated");
        let between = &before[gate..];
        assert!(
            between.lines().count() <= 3 && !between.contains('}'),
            "the gate is right above the read: {between}"
        );
        assert!(
            production[reads[0]..].starts_with("std::env::var(TEST_DESKTOP_OPEN_ENV)"),
            "the read is the seam's"
        );

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_some_and(|n| n != "desktop_open.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    // A test module may name it (shaping.rs lists it as a
                    // variable that is not a setting); production code not.
                    let text = text
                        .split("#[cfg(test)]\nmod tests {")
                        .next()
                        .unwrap_or_default();
                    assert!(
                        !text.contains(TEST_DESKTOP_OPEN_ENV)
                            && !text.contains("TEST_DESKTOP_OPEN_ENV"),
                        "{} names the seam outside its tests; read it only in desktop_open.rs",
                        path.display()
                    );
                }
            }
        }
    }
}
