//! Shared helpers for the profile harness install tests: an isolated home,
//! a bin folder with a `crystalline` symlink to the binary under test, and
//! the fixtures from crates/core/tests/fixtures/harness.

// `write` and `receipt` have no caller until the first harness install lands.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

pub struct Sandbox {
    pub _dir: tempfile::TempDir,
    pub home: PathBuf,
    pub bin: PathBuf,
}

/// A fresh home and a bin folder holding only a `crystalline` symlink to the
/// binary under test, so the absolute spelling resolves to `<bin>/crystalline`
/// and the PATH holds nothing else.
pub fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(
        assert_cmd::cargo::cargo_bin("crystalline"),
        bin.join("crystalline"),
    )
    .unwrap();
    Sandbox {
        _dir: dir,
        home,
        bin,
    }
}

/// `crystalline` isolated under `home` (HOME, XDG, CRYSTALLINE_TEST_NO_KEYCHAIN)
/// with PATH set to `bin` alone.
pub fn cmd(b: &Sandbox) -> Command {
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, &b.home);
    cmd.env_remove("COPILOT_HOME").env("PATH", &b.bin);
    cmd
}

pub fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

pub fn receipt(b: &Sandbox) -> serde_json::Value {
    let p = crate::common::isolated_state_dir(&b.home).join("installs.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}
