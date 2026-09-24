//! Shared helpers for the CLI integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// The repo-root `tests/fixtures` directory, shared across milestones.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// The base-directory variables that redirect every child this crate can
/// spawn into an isolated `home`, so its accounts, config, index and state
/// never land in the developer's own directories - plus one credential
/// variable that redirects a different kind of shared state entirely.
///
/// Both base-directory families are needed, because base-directory
/// resolution (via etcetera's `choose_base_strategy`) is a different
/// strategy per platform: the XDG one on unix and macOS, which reads `HOME`
/// and `XDG_*_HOME`, and the Windows one, which reads `USERPROFILE`,
/// `APPDATA` and `LOCALAPPDATA` and ignores the XDG variables entirely. On
/// Windows it also has no state directory of its own, so `state_dir` falls
/// back to the data directory, `APPDATA`. Setting only the XDG variables
/// would leave a Windows run resolving one real `%APPDATA%\crystalline` and
/// colliding with every other test doing the same (Windows byte-range locks
/// are mandatory). Setting the Windows names on unix as well is harmless
/// there, so one array covers both platforms without a `cfg`.
///
/// `CRYSTALLINE_TEST_NO_KEYCHAIN` is not a base directory at all: it is
/// `crystalline_remote::token`'s test seam
/// (`crystalline_remote::token::refuse_real_keychain`), checked inside
/// `TokenStore::resolve_and_load_for`/`save_resolving_for` - the choke point
/// every credential touch in the CLI and the engine funnels through - and it
/// is a pure kill switch: set, it refuses the real OS keychain and falls
/// back to the file store at whatever directory the caller already resolved
/// (`origins_state_dir()` under this test's own isolated `XDG_STATE_HOME`,
/// so a fixture a test wrote there is still where the CLI looks). It is
/// deliberately NOT `CRYSTALLINE_TEST_TOKEN_STORE_DIR`
/// (`crystalline::cmd::test_token_store_dir`), which redirects `connect
/// github` to a directory of ITS OWN choosing: setting that one here too
/// once broke `disconnecting_a_personal_identity_forgets_its_credential`,
/// whose fixture lives at the isolated state dir, not at a second, unrelated
/// one this array would have invented.
///
/// None of `HOME`/`XDG_STATE_HOME`/etc. reach the keychain on their own -
/// the OS keychain service name is a hardcoded constant, not derived from
/// any base directory - so a child that only got the directories above
/// still asks the real login keychain for a `github` credential the moment
/// it reaches `origin update`/`status`, `doctor`'s GitHub section, `connect
/// github --disconnect` or `users disable`/`remove`'s personal-credential
/// sweep. Every test in this crate that calls [`isolate`] or iterates this
/// array now gets the credential seam for free, which is the only way any
/// of the four call sites above stop touching the real keychain from a
/// test: none of them takes a `--token-store-dir` flag of its own.
pub fn isolation_env(home: &Path) -> [(&'static str, PathBuf); 8] {
    [
        ("HOME", home.to_path_buf()),
        ("XDG_CONFIG_HOME", home.join("config")),
        ("XDG_STATE_HOME", home.join("state")),
        ("XDG_CACHE_HOME", home.join("cache")),
        ("USERPROFILE", home.to_path_buf()),
        ("APPDATA", home.join("roaming")),
        ("LOCALAPPDATA", home.join("local")),
        ("CRYSTALLINE_TEST_NO_KEYCHAIN", PathBuf::from("1")),
    ]
}

/// Apply [`isolation_env`] to a child `Command`.
pub fn isolate(cmd: &mut Command, home: &Path) {
    for (name, value) in isolation_env(home) {
        cmd.env(name, value);
    }
    // A child logs by the filter the test gives it, never by the runner's.
    cmd.env_remove("RUST_LOG");
}

/// The name in [`isolation_env`] that governs where a child isolated by
/// [`isolate`] keeps its state directory, and the folder that variable points
/// at.
///
/// There is one per platform because base-directory resolution is one strategy
/// per platform (see [`isolation_env`]): the XDG strategy reads
/// `XDG_STATE_HOME`, and the Windows strategy ignores it, has no state
/// directory of its own and falls back to the data directory, `APPDATA`.
/// Naming the pair together is what keeps [`isolated_state_dir`] honest: a
/// fixture is placed under the same variable the binary will resolve through,
/// so the two can never be edited apart.
pub const STATE_HOME_VAR: &str = if cfg!(windows) {
    "APPDATA"
} else {
    "XDG_STATE_HOME"
};

/// Where a child isolated by [`isolate`] keeps its state, the directory
/// `crystalline_core::config::state_dir` resolves to under that environment.
///
/// A test that plants a fixture the binary is meant to find - an overlay
/// journal draft, a receipt - has to write it here rather than spell
/// `<home>/state/crystalline` by hand, which is the unix answer on every
/// platform and silently the wrong folder on Windows.
pub fn isolated_state_dir(home: &Path) -> PathBuf {
    let base = isolation_env(home)
        .into_iter()
        .find(|(name, _)| *name == STATE_HOME_VAR)
        .map(|(_, dir)| dir)
        .expect("the isolation environment sets this platform's state-home variable");
    base.join("crystalline")
}
