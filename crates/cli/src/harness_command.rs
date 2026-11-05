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

use crystalline_core::HarnessKind;

pub(crate) use crystalline_core::program::{is_crystalline_program, resolve_program};

use crate::install::{SESSION_START_COMMAND, extends_command};

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
}
