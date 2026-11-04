//! Comparing Crystalline versions.

/// Whether a `major.minor.patch` version is at least `min`. A pre-release or
/// build suffix after the patch is ignored, and a version that does not parse
/// is never enough: a guard is not handed to a server on a guess.
pub fn version_at_least(version: &str, min: &str) -> bool {
    fn parts(v: &str) -> Option<(u64, u64, u64)> {
        let core = v.trim().trim_start_matches('v');
        let core = core.split(['-', '+']).next()?;
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        it.next().is_none().then_some((major, minor, patch))
    }
    match (parts(version), parts(min)) {
        (Some(have), Some(need)) => have >= need,
        _ => false,
    }
}
