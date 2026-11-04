//! Which crystalline binary a command or an extension names.
//!
//! GUI apps (Cursor, Kiro, Claude Desktop) may start a command with a PATH
//! that lacks `crystalline`, so what they run is named by an absolute path.
//! [`resolve_program`] picks that path; `install` uses it for the profile
//! harnesses and for the Claude Desktop extension, and the extension update
//! (the service crate's `desktop_offer`) uses it with the binary the record pins.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::CommandSpelling;

/// The program to write into a command, and a notice for the report.
pub struct ResolvedProgram {
    pub program: String,
    pub warning: Option<String>,
}

/// The program for a profile's spelling. `Bare` is `crystalline`.
/// `AbsolutePathEntry` keeps `keep` (the spelling already in the file) while
/// it names an executable file called `crystalline` (spec decision 6), else
/// takes the first PATH entry that holds one, as the entry joined with
/// `crystalline` and never canonicalized, so a link such as
/// `/opt/homebrew/bin/crystalline` survives an upgrade. With nothing on PATH
/// it falls back to this binary's own path, with a warning that names
/// `who`, the display name of what gets the path. `path_env: None` means no
/// PATH.
pub fn resolve_program(
    who: &str,
    spelling: CommandSpelling,
    path_env: Option<&OsStr>,
    keep: Option<&str>,
) -> ResolvedProgram {
    if spelling == CommandSpelling::Bare {
        return ResolvedProgram {
            program: "crystalline".to_string(),
            warning: None,
        };
    }
    if let Some(kept) = keep
        && Path::new(kept).is_absolute()
        && is_crystalline_program(kept)
        && is_executable(Path::new(kept))
    {
        return ResolvedProgram {
            program: kept.to_string(),
            warning: None,
        };
    }
    if let Some(found) = path_env.and_then(first_on_path) {
        let program = found.display().to_string();
        let warning = in_versioned_folder(&found).then(|| {
            format!(
                "the crystalline on your PATH sits in a versioned folder ({program}); an upgrade may move it. Point PATH at a stable link such as /opt/homebrew/bin."
            )
        });
        return ResolvedProgram { program, warning };
    }
    fallback(who, std::env::current_exe().ok())
}

/// This binary's own path (`exe`), for when PATH holds no crystalline. The
/// warning names `who`, and also says so when the path sits in a versioned
/// folder an upgrade may move.
fn fallback(who: &str, exe: Option<PathBuf>) -> ResolvedProgram {
    let program = exe
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "crystalline".to_string());
    let mut warning = format!(
        "crystalline is not on your PATH, so {who} gets this binary's own path ({program})"
    );
    if exe.as_deref().is_some_and(in_versioned_folder) {
        warning.push_str(
            ". That path sits in a versioned folder, so an upgrade may move it. Put a stable link such as /opt/homebrew/bin on your PATH.",
        );
    }
    ResolvedProgram {
        program,
        warning: Some(warning),
    }
}

/// The file name the binary has on this platform.
const BINARY_NAME: &str = if cfg!(windows) {
    "crystalline.exe"
} else {
    "crystalline"
};

fn first_on_path(path_env: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_env)
        .filter(|entry| entry.is_absolute())
        .map(|entry| entry.join(BINARY_NAME))
        .find(|candidate| is_executable(candidate))
}

/// An existing file (links followed) that may be run.
pub fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// A path with a `Cellar` segment or a segment that reads as a version
/// (`0.22.1`, `1.2`), which a package upgrade replaces.
fn in_versioned_folder(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(seg) => {
            let seg = seg.to_string_lossy();
            let mut parts = seg.split('.');
            let numeric = |p: Option<&str>| {
                p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            };
            seg == "Cellar" || (numeric(parts.next()) && numeric(parts.next()))
        }
        _ => false,
    })
}

/// `crystalline`, or an absolute path whose file name is `crystalline`
/// (on Windows also `crystalline.exe`, in any case).
pub fn is_crystalline_program(word: &str) -> bool {
    if word == "crystalline" {
        return true;
    }
    let path = Path::new(word);
    if !path.is_absolute() {
        return false;
    }
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if cfg!(windows) {
        name.eq_ignore_ascii_case("crystalline") || name.eq_ignore_ascii_case("crystalline.exe")
    } else {
        name == "crystalline"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn exe(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_absolute_spelling_is_the_path_entry_never_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("Cellar/crystalline/0.22.1/bin/crystalline");
        exe(&real);
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&real, bin.join("crystalline")).unwrap();
        let path = std::env::join_paths([tmp.path().join("empty"), bin.clone()]).unwrap();
        let got = resolve_program(
            "Cursor",
            CommandSpelling::AbsolutePathEntry,
            Some(&path),
            None,
        );
        assert_eq!(got.program, bin.join("crystalline").display().to_string());
        assert!(got.warning.is_none());
        let cellar = real.parent().unwrap().as_os_str().to_owned();
        let got = resolve_program(
            "Cursor",
            CommandSpelling::AbsolutePathEntry,
            Some(&cellar),
            None,
        );
        assert_eq!(got.program, real.display().to_string());
        assert!(got.warning.unwrap().contains("versioned folder"));
        assert_eq!(
            resolve_program("Cursor", CommandSpelling::Bare, Some(&path), None).program,
            "crystalline"
        );
    }

    /// Decision 6: a GUI app's PATH at session start may lack crystalline.
    #[cfg(unix)]
    #[test]
    fn a_reconcile_keeps_an_absolute_spelling_that_still_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let kept = tmp.path().join("bin/crystalline");
        exe(&kept);
        let got = resolve_program(
            "Kiro",
            CommandSpelling::AbsolutePathEntry,
            Some(std::ffi::OsStr::new("")),
            Some(kept.to_str().unwrap()),
        );
        assert_eq!(got.program, kept.display().to_string());
        let gone = tmp.path().join("gone/crystalline");
        let got = resolve_program(
            "Kiro",
            CommandSpelling::AbsolutePathEntry,
            Some(std::ffi::OsStr::new("")),
            Some(gone.to_str().unwrap()),
        );
        assert_ne!(
            got.program,
            gone.display().to_string(),
            "a dead path is re-resolved"
        );
        let warning = got.warning.expect("the fallback is named");
        assert!(
            warning.contains("so Kiro gets this binary's own path"),
            "{warning}"
        );
    }

    /// Every target that falls back to this binary's own path hears when
    /// that path is a versioned one.
    #[test]
    fn the_own_path_fallback_warns_about_a_versioned_folder() {
        for who in ["Cursor", "Kiro", "Claude Desktop"] {
            let cellar = PathBuf::from("/opt/homebrew/Cellar/crystalline/0.31.0/bin/crystalline");
            let got = fallback(who, Some(cellar.clone()));
            assert_eq!(got.program, cellar.display().to_string());
            assert_eq!(
                got.warning.unwrap(),
                format!(
                    "crystalline is not on your PATH, so {who} gets this binary's own path ({}). That path sits in a versioned folder, so an upgrade may move it. Put a stable link such as /opt/homebrew/bin on your PATH.",
                    cellar.display()
                )
            );
            let plain = PathBuf::from("/Users/jo/.cargo/bin/crystalline");
            let got = fallback(who, Some(plain.clone()));
            assert_eq!(
                got.warning.unwrap(),
                format!(
                    "crystalline is not on your PATH, so {who} gets this binary's own path ({})",
                    plain.display()
                )
            );
        }
    }

    /// A file named crystalline that is not executable is not a binary to
    /// point a harness at.
    #[cfg(unix)]
    #[test]
    fn a_path_entry_without_an_executable_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("crystalline"), "not a program").unwrap();
        let bin = tmp.path().join("bin");
        exe(&bin.join("crystalline"));
        let path = std::env::join_paths([plain, bin.clone()]).unwrap();
        let got = resolve_program(
            "Cursor",
            CommandSpelling::AbsolutePathEntry,
            Some(&path),
            None,
        );
        assert_eq!(got.program, bin.join("crystalline").display().to_string());
    }
}
