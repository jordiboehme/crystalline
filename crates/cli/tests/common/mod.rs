//! Shared helpers for the CLI integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The repo-root `tests/fixtures` directory, shared across milestones.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}
