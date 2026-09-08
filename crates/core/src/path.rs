//! Rules about domain-relative paths that more than one crate has to answer
//! the same way.
//!
//! A path rule that two crates each implement in their own words drifts
//! silently: nothing compares the two, so the day one of them changes, the
//! surfaces that depend on them agreeing simply start disagreeing. Anything
//! in that shape belongs here, where there is one implementation and both
//! sides call it.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
