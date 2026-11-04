//! Hand a file to Claude Desktop, or show it in the file manager.
//!
//! `install claude-desktop` opens the extension it built here, so Claude
//! Desktop shows its install dialog. Nothing in this module prints: a caller
//! gets the error text and says what to do instead.
//!
//! Debug builds read a test seam, [`TEST_DESKTOP_OPEN_ENV`], so tests never
//! open the real Claude Desktop. Release builds never read it.

use std::path::Path;

/// The debug build's stand-in for the real open. `fail` refuses every call,
/// `log:<file>` appends one line per call to `<file>` and succeeds. Empty or
/// unset is the real open.
#[cfg_attr(
    not(any(debug_assertions, test)),
    expect(dead_code, reason = "only a debug build reads the seam")
)]
pub(crate) const TEST_DESKTOP_OPEN_ENV: &str = "CRYSTALLINE_TEST_DESKTOP_OPEN";

/// Opens files the way this machine does.
pub(crate) trait DesktopOpener {
    /// Open `file` with Claude Desktop.
    fn open_extension(&self, file: &Path) -> Result<(), String>;
    /// Show `file` in Finder or File Explorer.
    #[cfg_attr(
        not(all(test, debug_assertions)),
        expect(
            dead_code,
            reason = "the skill step that reveals its zip is not written yet"
        )
    )]
    fn reveal(&self, file: &Path) -> Result<(), String>;
}

/// The opener this process uses: in a debug build the test seam when it is
/// set, else the real one.
pub(crate) fn for_this_process() -> Box<dyn DesktopOpener> {
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
    Unknown(String),
}

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

#[cfg(target_os = "macos")]
impl DesktopOpener for System {
    fn open_extension(&self, file: &Path) -> Result<(), String> {
        run_open(&["-b", "com.anthropic.claudefordesktop"], file)
    }
    fn reveal(&self, file: &Path) -> Result<(), String> {
        run_open(&["-R"], file)
    }
}

/// Run `open <args> <file>` and wait for it.
#[cfg(target_os = "macos")]
fn run_open(args: &[&str], file: &Path) -> Result<(), String> {
    let status = std::process::Command::new("open")
        .args(args)
        .arg(file)
        .status()
        .map_err(|e| format!("could not run open ({e})"))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(format!("open exited with {code}")),
        None => Err("open was stopped by a signal".to_string()),
    }
}

#[cfg(windows)]
impl DesktopOpener for System {
    fn open_extension(&self, file: &Path) -> Result<(), String> {
        windows::shell_open(file.as_os_str(), None)
    }
    fn reveal(&self, file: &Path) -> Result<(), String> {
        let mut select = std::ffi::OsString::from("/select,");
        select.push(file);
        windows::shell_open(std::ffi::OsStr::new("explorer.exe"), Some(&select))
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
    pub(super) fn shell_open(file: &OsStr, parameters: Option<&OsStr>) -> Result<(), String> {
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
        if code > 32 {
            Ok(())
        } else {
            Err(format!("Windows could not open the file ({code})"))
        }
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
        assert_eq!(Seam::parse(""), None, "empty is the real opener");
        assert_eq!(Seam::parse("yes"), Some(Seam::Unknown("yes".to_string())));
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
        assert_eq!(
            Seam::Unknown("yes".to_string()).open_extension(file),
            Err(format!("unknown {TEST_DESKTOP_OPEN_ENV} value 'yes'"))
        );
    }

    /// A release build must never read the seam, or a set variable would
    /// stop a real install from opening Claude Desktop. Every place this
    /// crate names the variable is in this file, and every read of it sits
    /// under `#[cfg(debug_assertions)]`.
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
                    assert!(
                        !text.contains(TEST_DESKTOP_OPEN_ENV)
                            && !text.contains("TEST_DESKTOP_OPEN_ENV"),
                        "{} names the seam; read it only in desktop_open.rs",
                        path.display()
                    );
                }
            }
        }
    }
}
