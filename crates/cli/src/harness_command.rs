//! The program spelling in a profile harness command and its ownership rule.
//!
//! Cursor and Kiro are GUI apps whose hooks may run with a PATH that lacks
//! `crystalline`, so their commands name the binary by an absolute path
//! (`CommandSpelling::AbsolutePathEntry`); the terminal harnesses keep the
//! bare `crystalline`. The ownership rule here accepts both spellings, but
//! only for the profile harnesses: the legacy three keep `extends_command`
//! and `is_own_spelling` in install.rs unchanged, so a hand-written
//! `/opt/homebrew/bin/crystalline prompt system` in a Claude Code file stays
//! somebody else's (spec decision 18).

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crystalline_core::{CommandSpelling, HarnessKind};

use crate::install::{SESSION_START_COMMAND, extends_command};

/// The program to write into a command, and a notice for the report.
pub(crate) struct ResolvedProgram {
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
/// `harness`. `path_env: None` means no PATH.
pub(crate) fn resolve_program(
    harness: HarnessKind,
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
    let program = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "crystalline".to_string());
    let warning = Some(format!(
        "crystalline is not on your PATH, so {} gets this binary's own path ({program})",
        harness.display_name()
    ));
    ResolvedProgram { program, warning }
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
fn is_executable(path: &Path) -> bool {
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

/// Split a command into its program and the rest. The program is a
/// double-quoted first word or the text up to the first space; the rest is
/// empty or starts with a space.
pub(crate) fn split_program(command: &str) -> Option<(&str, &str)> {
    if let Some(quoted) = command.strip_prefix('"') {
        let close = quoted.find('"')?;
        let (program, rest) = (&quoted[..close], &quoted[close + 1..]);
        return (!program.is_empty() && (rest.is_empty() || rest.starts_with(' ')))
            .then_some((program, rest));
    }
    let split = command.find(' ').unwrap_or(command.len());
    let (program, rest) = command.split_at(split);
    (!program.is_empty()).then_some((program, rest))
}

/// `crystalline`, or an absolute path whose file name is `crystalline`
/// (on Windows also `crystalline.exe`, in any case).
pub(crate) fn is_crystalline_program(word: &str) -> bool {
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

/// The command line for `program` and `rest`, with the program in double
/// quotes when it holds whitespace (spec decision 11).
pub(crate) fn command_line(program: &str, rest: &str) -> String {
    if program.chars().any(char::is_whitespace) {
        format!("\"{program}\"{rest}")
    } else {
        format!("{program}{rest}")
    }
}

/// Whether a profile harness command runs our routing hook: a crystalline
/// program, then words that extend `crystalline prompt system` on a word
/// boundary. Such a command counts as present and comes out on uninstall.
pub(crate) fn profile_command_kind(command: &str) -> bool {
    split_program(command).is_some_and(|(program, rest)| {
        is_crystalline_program(program)
            && extends_command(&format!("crystalline{rest}"), SESSION_START_COMMAND)
    })
}

/// Whether a profile harness command is a spelling install wrote, and so
/// one it may rewrite in place: a crystalline program, ` prompt system`, an
/// optional ` --format cursor` or ` --format hook-specific`, an optional
/// ` --harness <known id>`, and nothing else. A hand-written variant with
/// other flags is counted present by [`profile_command_kind`] and left alone.
pub(crate) fn is_own_profile_spelling(command: &str) -> bool {
    let Some((program, rest)) = split_program(command) else {
        return false;
    };
    if !is_crystalline_program(program) {
        return false;
    }
    let words = SESSION_START_COMMAND
        .strip_prefix("crystalline")
        .expect("the routing command starts with the program");
    let Some(rest) = rest.strip_prefix(words) else {
        return false;
    };
    let rest = rest
        .strip_prefix(" --format cursor")
        .or_else(|| rest.strip_prefix(" --format hook-specific"))
        .unwrap_or(rest);
    match rest.strip_prefix(" --harness ") {
        Some(id) => HarnessKind::from_id(id).is_some(),
        None => rest.is_empty(),
    }
}

/// An absolute program path for tests that holds on the platform they run
/// on: a Unix path is not absolute on Windows, where a drive is needed.
#[cfg(test)]
pub(crate) const TEST_PROGRAM: &str = if cfg!(windows) {
    r"C:\tools\crystalline.exe"
} else {
    "/opt/homebrew/bin/crystalline"
};

/// A second absolute program path for tests, in another folder.
#[cfg(test)]
pub(crate) const TEST_OTHER_PROGRAM: &str = if cfg!(windows) {
    r"C:\Users\u\.cargo\bin\crystalline.exe"
} else {
    "/home/u/.cargo/bin/crystalline"
};

#[cfg(test)]
mod tests {
    use super::*;

    const ABS: &str = TEST_PROGRAM;

    /// An absolute program path with a space in it, on this platform.
    const SPACED: &str = if cfg!(windows) {
        r"C:\Users\Jo Doe\bin\crystalline.exe"
    } else {
        "/Users/Jo Doe/.cargo/bin/crystalline"
    };

    #[test]
    fn ownership_accepts_the_bare_and_the_absolute_spelling_and_nothing_else() {
        for ours in [
            "crystalline prompt system --harness cursor".to_string(),
            format!("{ABS} prompt system --format cursor --harness cursor"),
            format!("\"{SPACED}\" prompt system --harness kiro"),
            format!("{ABS} prompt system"),
        ] {
            assert!(profile_command_kind(&ours), "{ours}");
        }
        for foreign in [
            "crystalline-foo prompt system",
            "/x/crystalline2 prompt system",
            "/x/crystalline/bin/other prompt system",
            "relative/crystalline prompt system",
            "/Users/someone/.othertool/hooks/cursor-hook.sh SessionStart",
            "crystalline prompt systemd",
            "crystalline hook stop --harness cursor",
            r"C:\x\crystalline2.exe prompt system",
        ] {
            assert!(!profile_command_kind(foreign), "{foreign}");
        }
    }

    #[test]
    fn only_spellings_we_wrote_are_rewritten() {
        assert!(is_own_profile_spelling(
            "crystalline prompt system --format cursor --harness cursor"
        ));
        assert!(is_own_profile_spelling(&format!(
            "{ABS} prompt system --format hook-specific --harness gemini"
        )));
        assert!(is_own_profile_spelling(&format!(
            "\"{SPACED}\" prompt system --harness kiro"
        )));
        assert!(is_own_profile_spelling("crystalline prompt system"));
        assert!(!is_own_profile_spelling(
            "crystalline prompt system --format cursor --workspace /repo"
        ));
        assert!(!is_own_profile_spelling(
            "crystalline prompt system --format hook-specific --harness nextgen"
        ));
    }

    /// The Windows spellings of the binary: a drive path, quoted when it
    /// holds a space, with the `.exe` suffix in any case.
    #[cfg(windows)]
    #[test]
    fn a_windows_spelling_is_ours() {
        for ours in [
            r"C:\tools\crystalline.exe prompt system --format cursor --harness cursor",
            r"C:\tools\CRYSTALLINE.EXE prompt system",
            r"C:\tools\crystalline prompt system --harness kiro",
            r#""C:\Program Files\Crystalline\crystalline.exe" prompt system --harness cursor"#,
        ] {
            assert!(profile_command_kind(ours), "{ours}");
            assert!(is_own_profile_spelling(ours), "{ours}");
        }
        assert!(!profile_command_kind(
            r"C:\tools\crystalline2.exe prompt system"
        ));
        assert!(!profile_command_kind(
            r"tools\crystalline.exe prompt system"
        ));
        assert_eq!(
            split_program(r#""C:\Program Files\crystalline.exe" prompt system"#),
            Some((r"C:\Program Files\crystalline.exe", " prompt system"))
        );
        assert_eq!(
            command_line(r"C:\Program Files\crystalline.exe", " prompt system"),
            r#""C:\Program Files\crystalline.exe" prompt system"#
        );
    }

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
            HarnessKind::Cursor,
            CommandSpelling::AbsolutePathEntry,
            Some(&path),
            None,
        );
        assert_eq!(got.program, bin.join("crystalline").display().to_string());
        assert!(got.warning.is_none());
        let cellar = real.parent().unwrap().as_os_str().to_owned();
        let got = resolve_program(
            HarnessKind::Cursor,
            CommandSpelling::AbsolutePathEntry,
            Some(&cellar),
            None,
        );
        assert_eq!(got.program, real.display().to_string());
        assert!(got.warning.unwrap().contains("versioned folder"));
        assert_eq!(
            resolve_program(
                HarnessKind::Cursor,
                CommandSpelling::Bare,
                Some(&path),
                None
            )
            .program,
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
            HarnessKind::Kiro,
            CommandSpelling::AbsolutePathEntry,
            Some(std::ffi::OsStr::new("")),
            Some(kept.to_str().unwrap()),
        );
        assert_eq!(got.program, kept.display().to_string());
        let gone = tmp.path().join("gone/crystalline");
        let got = resolve_program(
            HarnessKind::Kiro,
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

    #[test]
    fn a_program_with_a_space_is_quoted_and_read_back() {
        let line = command_line(
            "/Users/Jo Doe/bin/crystalline",
            " prompt system --harness kiro",
        );
        assert_eq!(
            line,
            "\"/Users/Jo Doe/bin/crystalline\" prompt system --harness kiro"
        );
        assert_eq!(
            split_program(&line),
            Some((
                "/Users/Jo Doe/bin/crystalline",
                " prompt system --harness kiro"
            ))
        );
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
            HarnessKind::Cursor,
            CommandSpelling::AbsolutePathEntry,
            Some(&path),
            None,
        );
        assert_eq!(got.program, bin.join("crystalline").display().to_string());
    }
}
