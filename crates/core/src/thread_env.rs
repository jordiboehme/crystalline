//! The embedding thread variables this binary sets for itself, and how a
//! program it spawns is kept clear of them.
//!
//! `main` sets `RAYON_NUM_THREADS` when it chooses the embedding thread
//! count, and leaves a marker `<source>:<value>` beside it. A child that is
//! Crystalline itself inherits both. Any other program has no use for them,
//! so [`strip_from`] removes them, but only when the marker shows this
//! binary set the value: a count the user chose stays.

use std::process::Command;

/// The variable candle reads for its thread count.
pub const RAYON_VAR: &str = "RAYON_NUM_THREADS";

/// The marker variable, `<source>:<value>`.
pub const EMBED_THREADS_FROM_VAR: &str = "CRYSTALLINE_EMBED_THREADS_FROM";

/// Marker source: the count came from `CRYSTALLINE_EMBED_THREADS`.
pub const SOURCE_EMBED_THREADS: &str = "embed-threads";

/// Marker source: the count came from the CPU limit.
pub const SOURCE_CPU_LIMIT: &str = "cpu-limit";

/// The source named by `marker`, when it says this binary set `rayon`: the
/// marker is `<source>:<value>`, the source is one this binary writes and
/// the value equals the current `RAYON_NUM_THREADS`.
pub fn marker_source(marker: Option<&str>, rayon: Option<&str>) -> Option<&'static str> {
    let (source, value) = marker?.split_once(':')?;
    let source = match source {
        SOURCE_EMBED_THREADS => SOURCE_EMBED_THREADS,
        SOURCE_CPU_LIMIT => SOURCE_CPU_LIMIT,
        _ => return None,
    };
    (!value.is_empty() && value == rayon?.trim()).then_some(source)
}

/// Whether this process's `RAYON_NUM_THREADS` was set by this binary.
pub fn set_by_this_binary() -> bool {
    marker_source(
        std::env::var(EMBED_THREADS_FROM_VAR).ok().as_deref(),
        std::env::var(RAYON_VAR).ok().as_deref(),
    )
    .is_some()
}

/// Removes both variables from a command for a program that is not
/// Crystalline, when this binary set them. Returns the command for chaining.
/// Call it on every spawn of a foreign program. A spawn of Crystalline itself
/// keeps them.
pub fn strip_from(command: &mut Command) -> &mut Command {
    if set_by_this_binary() {
        command.env_remove(RAYON_VAR);
        command.env_remove(EMBED_THREADS_FROM_VAR);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_is_trusted_only_when_its_value_is_the_current_count() {
        assert_eq!(
            marker_source(Some("cpu-limit:2"), Some("2")),
            Some("cpu-limit")
        );
        assert_eq!(
            marker_source(Some("embed-threads:4"), Some("4")),
            Some("embed-threads")
        );
        assert_eq!(marker_source(Some("cpu-limit:2"), Some("3")), None);
        assert_eq!(marker_source(Some("cpu-limit:2"), None), None);
        assert_eq!(marker_source(None, Some("2")), None);
        assert_eq!(marker_source(Some("cpu-limit"), Some("2")), None);
        assert_eq!(marker_source(Some("default:2"), Some("2")), None);
        assert_eq!(marker_source(Some("cpu-limit:"), Some("")), None);
    }

    fn removed(command: &Command) -> Vec<String> {
        command
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect()
    }

    // One test touches the process environment, so none races it.
    #[test]
    fn strip_from_removes_both_variables_only_for_a_count_this_binary_set() {
        // SAFETY: this is the only test in the crate that touches these two
        // variables, and it restores them.
        unsafe {
            std::env::set_var(RAYON_VAR, "2");
            std::env::set_var(EMBED_THREADS_FROM_VAR, "cpu-limit:2");
        }
        let mut command = Command::new("true");
        strip_from(&mut command);
        let mut names = removed(&command);
        names.sort();
        assert_eq!(names, [EMBED_THREADS_FROM_VAR, RAYON_VAR]);

        // A count the user chose, with a stale marker, stays.
        unsafe { std::env::set_var(RAYON_VAR, "5") };
        let mut command = Command::new("true");
        strip_from(&mut command);
        assert!(removed(&command).is_empty());

        // No marker at all stays too.
        unsafe { std::env::remove_var(EMBED_THREADS_FROM_VAR) };
        let mut command = Command::new("true");
        strip_from(&mut command);
        assert!(removed(&command).is_empty());
        unsafe { std::env::remove_var(RAYON_VAR) };
    }
}
