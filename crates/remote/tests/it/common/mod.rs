//! Shared state for remote's integration suites.

/// Held by every test that points `HOME` or `XDG_STATE_HOME` somewhere else.
/// The suites used to be separate binaries, each sure it was alone in its
/// process; under the `cargo test` fallback they now share one.
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
