//! Rules about domain-relative paths that more than one crate has to answer
//! the same way.
//!
//! A path rule that two crates each implement in their own words drifts
//! silently: nothing compares the two, so the day one of them changes, the
//! surfaces that depend on them agreeing simply start disagreeing. Anything
//! in that shape belongs here, where there is one implementation and both
//! sides call it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes one temp file from another within this process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The sibling a write goes through before it is renamed onto `target`:
/// `.<file name>.<purpose>.<pid>.<seq>`, hidden so that neither a sync, the
/// daemon's watcher nor remote change detection ever takes one a crash left
/// behind for an engram or a change to share. The whole file name is kept
/// (`shot.png`, not `shot`), so a leftover still says whose it was, and the
/// counter keeps two writes in one process off each other's temp file.
pub fn hidden_temp_path(target: &Path, purpose: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = std::ffi::OsString::from(".");
    if let Some(own) = target.file_name() {
        name.push(own);
    }
    name.push(format!(".{purpose}.{}.{seq}", std::process::id()));
    target.with_file_name(name)
}

/// Two paths' case-insensitive identity: what macOS and Windows treat as one
/// path. Full Unicode lowercase rather than ASCII, since both filesystems
/// fold well beyond ASCII, and locale-independent, so the answer is the same
/// on every machine looking at the same domain.
///
/// This is not any one filesystem's folding table and does not try to be. It
/// only has to be coarse enough to catch the pairs a real checkout would
/// collapse. It over-folds in places (`U+212A` KELVIN SIGN lowercases to `k`,
/// where a real checkout keeps the two apart), which both callers absorb in
/// the safe direction rather than compensate for.
///
/// Two callers ask this question, and they must get the same answer or they
/// contradict each other about the same domain:
///
/// - verify rule `E009` ([`crate::verify`], `format::check_domain`) reports a
///   domain holding two paths that fold together, because no macOS or Windows
///   checkout can hold them all.
/// - `crystalline_remote::changes::detect_local_changes` folds a walked path
///   onto a base-snapshot entry that differs only in case, and declines to
///   guess whenever more than one entry folds that way - which is exactly the
///   state `E009` asks the domain to fix.
pub fn fold_path_case(path: &str) -> String {
    path.to_lowercase()
}

/// Whether `path` lies at or under `root`, compared by components, never on
/// disk: both separators split, `.` dropped and `..` applied, a Windows
/// verbatim prefix (`\\?\`, `\\?\UNC\`) removed and, with `fold_case`,
/// without regard to case ([`fold_path_case`]). So `AppData-old` never
/// matches `AppData`. Nothing is opened or canonicalized, so a path in a
/// folder the OS guards (macOS privacy protection) is compared without
/// touching it.
pub fn lies_within(path: &Path, root: &Path, fold_case: bool) -> bool {
    let path = parts(path, fold_case);
    let root = parts(root, fold_case);
    !root.is_empty() && path.len() >= root.len() && path[..root.len()] == root[..]
}

/// The components [`lies_within`] compares.
fn parts(path: &Path, fold_case: bool) -> Vec<String> {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/UNC/") {
        text = format!("//{rest}");
    } else if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_string();
    }
    if fold_case {
        text = fold_path_case(&text);
    }
    let mut out: Vec<String> = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_temp_path_is_a_hidden_sibling_that_keeps_the_whole_name() {
        let first = hidden_temp_path(Path::new("/kb/assets/shot.png"), "tmp");
        let second = hidden_temp_path(Path::new("/kb/assets/shot.png"), "tmp");
        assert_eq!(first.parent(), Some(Path::new("/kb/assets")));
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(&format!(".shot.png.tmp.{}.", std::process::id())),
            "{name}"
        );
        assert_ne!(first, second, "two calls never share a temp file");
    }

    #[test]
    fn folding_is_case_insensitive_and_leaves_the_shape_alone() {
        assert_eq!(fold_path_case("Notes/Alpha.md"), "notes/alpha.md");
        assert_eq!(fold_path_case("notes/alpha.md"), "notes/alpha.md");
        assert_eq!(
            fold_path_case("Notes/Alpha.md"),
            fold_path_case("NOTES/ALPHA.MD"),
            "every spelling of one path folds together"
        );
        assert_eq!(
            fold_path_case("Notes/Beta.md"),
            "notes/beta.md",
            "separators and extensions are untouched"
        );
    }

    #[test]
    fn folding_reaches_beyond_ascii() {
        assert_eq!(fold_path_case("Ordner/Größe.md"), "ordner/größe.md");
        assert_eq!(fold_path_case("notes/ÉCOLE.md"), "notes/école.md");
    }

    #[test]
    fn lies_within_compares_components_by_text() {
        use super::lies_within;
        let p = |s: &str| std::path::PathBuf::from(s);
        assert!(lies_within(&p("/a/b/c"), &p("/a/b"), false));
        assert!(lies_within(&p("/a/b"), &p("/a/b"), false));
        assert!(!lies_within(&p("/a/b-old/c"), &p("/a/b"), false));
        assert!(lies_within(&p("/a/./x/../b/c"), &p("/a/b"), false));
        assert!(lies_within(
            &p(r"\\?\C:\Users\ada\kb"),
            &p(r"C:\Users\ada"),
            false
        ));
        assert!(lies_within(
            &p(r"\\?\UNC\srv\share\kb"),
            &p(r"\\srv\share"),
            false
        ));
        assert!(!lies_within(&p("/A/B/c"), &p("/a/b"), false));
        assert!(lies_within(&p("/A/B/c"), &p("/a/b"), true));
        assert!(
            !lies_within(&p("/a"), &p(""), true),
            "an empty root holds nothing"
        );
    }
}
