//! Shared helpers for the golden fixture tests.
//!
//! This module is compiled into several integration test binaries; not every
//! binary uses every helper, so unused-warnings are allowed here.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The repo-root `tests/fixtures` directory, shared across milestones.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Collect `.md` files directly inside a fixtures subdirectory, sorted by name.
pub fn md_files(subdir: &str) -> Vec<PathBuf> {
    let dir = fixtures_dir().join(subdir);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    files.sort();
    files
}

/// Read a fixture file as raw bytes-as-string, preserving exact content.
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The file stem, for snapshot naming and messages.
pub fn stem(path: &Path) -> String {
    path.file_stem().unwrap().to_string_lossy().into_owned()
}
