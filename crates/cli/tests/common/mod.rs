//! Shared helpers for the CLI integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// The repo-root `tests/fixtures` directory, shared across milestones.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// The seven base-directory variables that redirect every child this crate
/// can spawn into an isolated `home`, so its accounts, config, index and
/// state never land in the developer's own directories.
///
/// Both families are needed, because base-directory resolution (via
/// etcetera's `choose_base_strategy`) is a different strategy per platform:
/// the XDG one on unix and macOS, which reads `HOME` and `XDG_*_HOME`, and
/// the Windows one, which reads `USERPROFILE`, `APPDATA` and `LOCALAPPDATA`
/// and ignores the XDG variables entirely. On Windows it also has no state
/// directory of its own, so `state_dir` falls back to the data directory,
/// `APPDATA`. Setting only the XDG variables would leave a Windows run
/// resolving one real `%APPDATA%\crystalline` and colliding with every other
/// test doing the same (Windows byte-range locks are mandatory). Setting the
/// Windows names on unix as well is harmless there, so one array covers both
/// platforms without a `cfg`.
pub fn isolation_env(home: &Path) -> [(&'static str, PathBuf); 7] {
    [
        ("HOME", home.to_path_buf()),
        ("XDG_CONFIG_HOME", home.join("config")),
        ("XDG_STATE_HOME", home.join("state")),
        ("XDG_CACHE_HOME", home.join("cache")),
        ("USERPROFILE", home.to_path_buf()),
        ("APPDATA", home.join("roaming")),
        ("LOCALAPPDATA", home.join("local")),
    ]
}

/// Apply [`isolation_env`] to a child `Command`.
pub fn isolate(cmd: &mut Command, home: &Path) {
    for (name, value) in isolation_env(home) {
        cmd.env(name, value);
    }
}
