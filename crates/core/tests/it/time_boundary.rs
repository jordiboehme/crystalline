//! Only crystalline-core may name a date library: every other crate goes
//! through `crystalline_core::time`.
//!
//! The test scans every crate but core (`Cargo.toml`, `build.rs` and the
//! `.rs` files under `src`, `tests`, `benches` and `examples`) for `chrono`
//! and `jiff` as a whole word. A file that names jiff fails. A file that
//! names chrono fails unless it is on
//! `crates/core/tests/time_boundary_allow.txt`, the files that still used
//! chrono when the module arrived; a listed file that no longer names
//! chrono fails too, so the list shrinks as each crate moves over.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

const ALLOW_LIST: &str = include_str!("../time_boundary_allow.txt");

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// Whether `text` holds `word` with no identifier character on either side.
fn names(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Every file the boundary covers, as a workspace-relative path with
/// forward slashes.
fn scanned_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates folder");
    for entry in crates {
        let dir = entry.expect("crate entry").path();
        if !dir.is_dir() || dir.file_name().is_some_and(|n| n == "core") {
            continue;
        }
        for top in ["Cargo.toml", "build.rs"] {
            let file = dir.join(top);
            if file.is_file() {
                files.push(file);
            }
        }
        for sub in ["src", "tests", "benches", "examples"] {
            for file in WalkDir::new(dir.join(sub)).into_iter().flatten() {
                let path = file.path();
                if file.file_type().is_file() && path.extension().is_some_and(|e| e == "rs") {
                    files.push(path.to_path_buf());
                }
            }
        }
    }
    let mut relative: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    relative.sort();
    relative
}

#[test]
fn no_crate_but_core_names_chrono_or_jiff() {
    let root = workspace_root();
    let allowed: BTreeSet<&str> = ALLOW_LIST
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let mut names_jiff = Vec::new();
    let mut unlisted_chrono = Vec::new();
    let mut still_listed = Vec::new();
    let mut seen_chrono = BTreeSet::new();
    for rel in scanned_files(&root) {
        let text = std::fs::read_to_string(root.join(&rel)).expect("readable source");
        if names(&text, "jiff") {
            names_jiff.push(rel.clone());
        }
        if names(&text, "chrono") {
            seen_chrono.insert(rel.clone());
            if !allowed.contains(rel.as_str()) {
                unlisted_chrono.push(rel);
            }
        }
    }
    for listed in &allowed {
        if !seen_chrono.contains(*listed) {
            still_listed.push(*listed);
        }
    }

    let mut problems = String::new();
    if !names_jiff.is_empty() {
        problems.push_str("these files name jiff; only crystalline-core may:\n");
        problems.push_str(&names_jiff.join("\n"));
        problems.push('\n');
    }
    if !unlisted_chrono.is_empty() {
        problems.push_str(
            "these files name chrono; use crystalline_core::time instead \
             (the allow-list only shrinks):\n",
        );
        problems.push_str(&unlisted_chrono.join("\n"));
        problems.push('\n');
    }
    if !still_listed.is_empty() {
        problems.push_str(
            "these allow-list entries no longer name chrono; remove them from \
             crates/core/tests/time_boundary_allow.txt:\n",
        );
        problems.push_str(&still_listed.join("\n"));
        problems.push('\n');
    }
    assert!(problems.is_empty(), "{problems}");
}

#[test]
fn names_matches_whole_words_only() {
    assert!(names("use chrono::Utc;", "chrono"));
    assert!(names("chrono = { workspace = true }", "chrono"));
    assert!(names("(jiff)", "jiff"));
    assert!(!names("in chronological order", "chrono"));
    assert!(!names("my_chrono_helper", "chrono"));
    assert!(!names("jiffy", "jiff"));
}
