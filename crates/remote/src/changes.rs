//! Local change detection: what a domain's working tree looks like relative
//! to the base snapshot recorded in [`crate::state::OriginState::files`].
//!
//! [`detect_local_changes`] walks the domain root exactly the way
//! `crystalline_index::sync` walks it for indexing (the same hidden-file
//! filter, the same SHA-256 hex encoding) except that every non-hidden file
//! is included regardless of extension: assets, `.crystalline.yaml` and any
//! other file that lives alongside the engrams travels with the domain, not
//! just markdown. This is pure detection with no side effects; a later task
//! decides what to do with the result (open a share proposal, warn about
//! files too large to share).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use walkdir::WalkDir;

use crate::error::RemoteError;
use crate::state::{BaseStamp, sha256_hex};

/// Files larger than this are never hashed or shared: they are reported in
/// [`LocalChanges::skipped_large`] instead of being treated as a change.
pub const MAX_SHARED_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// One local change relative to the base snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalChange {
    /// A file present on disk with no entry in the base snapshot.
    Added {
        /// The file's path, relative to the domain root, forward-slash
        /// normalized.
        path: String,
        /// The SHA-256 hex digest of the file's current content.
        sha256: String,
    },
    /// A file present on disk whose content no longer matches its base
    /// snapshot entry.
    Modified {
        /// The file's path, relative to the domain root, forward-slash
        /// normalized.
        path: String,
        /// The SHA-256 hex digest of the file's current content.
        sha256: String,
    },
    /// A file recorded in the base snapshot that is no longer on disk.
    Deleted {
        /// The file's path, relative to the domain root, forward-slash
        /// normalized.
        path: String,
    },
}

impl LocalChange {
    /// The path this change concerns, whichever variant it is.
    pub fn path(&self) -> &str {
        match self {
            LocalChange::Added { path, .. } => path,
            LocalChange::Modified { path, .. } => path,
            LocalChange::Deleted { path } => path,
        }
    }
}

/// The result of comparing a domain's working tree against its base
/// snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalChanges {
    /// Every detected change, in the order the walk encountered them (added
    /// and modified files first by walk order, then deletions).
    pub changes: Vec<LocalChange>,
    /// Files skipped for exceeding [`MAX_SHARED_FILE_BYTES`], with their
    /// sizes in bytes.
    pub skipped_large: Vec<(String, u64)>,
}

/// Detects local changes in `domain_root` relative to `base`, the base
/// snapshot manifest from [`crate::state::OriginState::files`].
///
/// Walk rules, mirroring `crystalline_index::sync`'s conventions:
///
/// - dot-files and dot-directories are skipped at any depth; the domain root
///   itself is never pruned, even if its own name starts with a dot.
/// - every non-hidden file is included regardless of extension.
/// - a file larger than [`MAX_SHARED_FILE_BYTES`] is reported in
///   `skipped_large` instead of being hashed or classified as a change.
/// - a file with no base entry is [`LocalChange::Added`].
/// - a file with a base entry is hashed and compared against the recorded
///   size and digest; content that still matches (a file touched or rewritten
///   with identical bytes, whatever its new mtime) is not a change at all,
///   and anything else is [`LocalChange::Modified`].
/// - a base entry with no file on disk is [`LocalChange::Deleted`].
///
/// Relative paths are always forward-slash normalized, regardless of
/// platform.
pub fn detect_local_changes(
    domain_root: &Path,
    base: &BTreeMap<String, BaseStamp>,
) -> Result<LocalChanges, RemoteError> {
    let mut changes = Vec::new();
    let mut skipped_large = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for entry in WalkDir::new(domain_root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e.file_name().to_string_lossy().as_ref()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy();
        if is_hidden(&fname) {
            continue;
        }

        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let rel = rel_path(domain_root, entry.path());
        let size = meta.len();
        seen.insert(rel.clone());

        if size > MAX_SHARED_FILE_BYTES {
            skipped_large.push((rel, size));
            continue;
        }

        match base.get(&rel) {
            Some(stamp) => {
                let bytes = std::fs::read(entry.path())?;
                let sha256 = sha256_hex(&bytes);
                if stamp.size != size || stamp.sha256 != sha256 {
                    changes.push(LocalChange::Modified { path: rel, sha256 });
                }
            }
            None => {
                let bytes = std::fs::read(entry.path())?;
                let sha256 = sha256_hex(&bytes);
                changes.push(LocalChange::Added { path: rel, sha256 });
            }
        }
    }

    for rel in base.keys() {
        if !seen.contains(rel) {
            changes.push(LocalChange::Deleted { path: rel.clone() });
        }
    }

    Ok(LocalChanges {
        changes,
        skipped_large,
    })
}

/// Pairs an `Added` change with a `Deleted` change of identical content hash:
/// the same bytes showing up under a new path and vanishing from an old one
/// almost always means the file was renamed or moved rather than genuinely
/// replaced. Returned as `(deleted_path, added_path)`. For nicer summaries
/// only: nothing downstream depends on the pairing being complete or correct,
/// it never changes what gets shared.
///
/// Matches added hashes against the base snapshot's recorded hash for each
/// deleted path, so it needs no re-reading of file content: `detect_local_changes`
/// already carries the hash on every `Added` change, and `base` already
/// carries the hash for every path that could be `Deleted`.
pub fn pair_renames(
    changes: &LocalChanges,
    base: &BTreeMap<String, BaseStamp>,
) -> Vec<(String, String)> {
    let mut deleted_by_hash: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for change in &changes.changes {
        if let LocalChange::Deleted { path } = change
            && let Some(stamp) = base.get(path)
        {
            deleted_by_hash
                .entry(stamp.sha256.as_str())
                .or_default()
                .push(path.as_str());
        }
    }

    let mut used: BTreeSet<&str> = BTreeSet::new();
    let mut pairs = Vec::new();
    for change in &changes.changes {
        if let LocalChange::Added { path, sha256 } = change
            && let Some(candidates) = deleted_by_hash.get(sha256.as_str())
            && let Some(from) = candidates.iter().find(|c| !used.contains(*c))
        {
            used.insert(from);
            pairs.push((from.to_string(), path.clone()));
        }
    }
    pairs
}

/// The per-domain config file name. Starts with a dot like every other
/// dot-file, but it is a real, meaningful part of the domain (verify
/// overrides, required files) rather than tooling clutter, so it travels
/// with the domain like any other tracked file instead of being filtered out
/// as hidden.
const DOMAIN_CONFIG_FILE_NAME: &str = ".crystalline.yaml";

/// True for any name starting with `.`, except the special `.` and `..`
/// directory entries and [`DOMAIN_CONFIG_FILE_NAME`].
fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".." && name != DOMAIN_CONFIG_FILE_NAME
}

/// The forward-slash relative path of `path` under `root`.
fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::*;
    use crate::state::BaseStamp;

    fn stamp_for(bytes: &[u8]) -> BaseStamp {
        BaseStamp {
            sha256: crate::state::sha256_hex(bytes),
            size: bytes.len() as u64,
        }
    }

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn fresh_directory_against_empty_base_reports_every_file_as_added_with_a_hash() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes/one.md", b"one");
        write(dir.path(), "notes/two.md", b"two");

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();

        let mut changes = result.changes;
        changes.sort_by(|a, b| a.path().cmp(b.path()));
        assert_eq!(changes.len(), 2);
        match &changes[0] {
            LocalChange::Added { path, sha256 } => {
                assert_eq!(path, "notes/one.md");
                assert_eq!(sha256, &crate::state::sha256_hex(b"one"));
            }
            other => panic!("expected Added, got {other:?}"),
        }
        match &changes[1] {
            LocalChange::Added { path, sha256 } => {
                assert_eq!(path, "notes/two.md");
                assert_eq!(sha256, &crate::state::sha256_hex(b"two"));
            }
            other => panic!("expected Added, got {other:?}"),
        }
        assert!(result.skipped_large.is_empty());
    }

    #[test]
    fn unchanged_file_same_size_and_content_reports_no_change() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes/one.md", b"stable content");
        let mut base = BTreeMap::new();
        base.insert("notes/one.md".to_string(), stamp_for(b"stable content"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert!(result.changes.is_empty(), "{:?}", result.changes);
    }

    #[test]
    fn rewritten_identical_content_reports_no_change_regardless_of_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let rel = "notes/one.md";
        write(dir.path(), rel, b"stable content");
        let mut base = BTreeMap::new();
        base.insert(rel.to_string(), stamp_for(b"stable content"));

        // Rewrite with identical bytes; this changes mtime but not content.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write(dir.path(), rel, b"stable content");

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert!(result.changes.is_empty(), "{:?}", result.changes);
    }

    #[test]
    fn same_size_different_content_reports_modified() {
        let dir = tempfile::tempdir().unwrap();
        let rel = "notes/one.md";
        write(dir.path(), rel, b"aaaaaaaa");
        let mut base = BTreeMap::new();
        base.insert(rel.to_string(), stamp_for(b"bbbbbbbb"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert_eq!(result.changes.len(), 1);
        match &result.changes[0] {
            LocalChange::Modified { path, sha256 } => {
                assert_eq!(path, rel);
                assert_eq!(sha256, &crate::state::sha256_hex(b"aaaaaaaa"));
            }
            other => panic!("expected Modified, got {other:?}"),
        }
    }

    #[test]
    fn different_size_content_reports_modified() {
        let dir = tempfile::tempdir().unwrap();
        let rel = "notes/one.md";
        write(dir.path(), rel, b"a longer rewritten body");
        let mut base = BTreeMap::new();
        base.insert(rel.to_string(), stamp_for(b"short"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert!(matches!(&result.changes[0], LocalChange::Modified { .. }));
    }

    #[test]
    fn file_absent_on_disk_but_present_in_base_reports_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let mut base = BTreeMap::new();
        base.insert("notes/gone.md".to_string(), stamp_for(b"was here"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert_eq!(result.changes.len(), 1);
        match &result.changes[0] {
            LocalChange::Deleted { path } => assert_eq!(path, "notes/gone.md"),
            other => panic!("expected Deleted, got {other:?}"),
        }
    }

    #[test]
    fn dot_files_and_dot_directories_are_skipped_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".hidden-file.md", b"secret");
        write(dir.path(), ".git/config", b"secret");
        write(dir.path(), "notes/.hidden-nested.md", b"secret");
        write(dir.path(), "notes/visible.md", b"visible");

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();
        let paths: Vec<&str> = result.changes.iter().map(|c| c.path()).collect();
        assert_eq!(paths, vec!["notes/visible.md"], "{paths:?}");
    }

    #[test]
    fn every_non_hidden_extension_is_included_regardless_of_type() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".crystalline.yaml", b"config: true");
        write(dir.path(), "assets/logo.png", b"binary-ish content");
        write(dir.path(), "MANIFEST.md", b"# Manifest");

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();
        let mut paths: Vec<&str> = result.changes.iter().map(|c| c.path()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![".crystalline.yaml", "MANIFEST.md", "assets/logo.png"]
        );
    }

    #[test]
    fn nested_directories_produce_forward_slash_paths() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/b/c/deep.md", b"deep content");

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].path(), "a/b/c/deep.md");
        assert!(!result.changes[0].path().contains('\\'));
    }

    #[test]
    fn oversize_file_is_reported_as_skipped_large_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let oversized = vec![0u8; (MAX_SHARED_FILE_BYTES + 1) as usize];
        write(dir.path(), "notes/huge.md", &oversized);

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();
        assert!(result.changes.is_empty());
        assert_eq!(
            result.skipped_large,
            vec![("notes/huge.md".to_string(), oversized.len() as u64)]
        );
    }

    #[test]
    fn rename_pairing_matches_an_added_hash_against_a_deleted_base_hash() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes/new-name.md", b"moved content");
        let mut base = BTreeMap::new();
        base.insert("notes/old-name.md".to_string(), stamp_for(b"moved content"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        let pairs = pair_renames(&result, &base);
        assert_eq!(
            pairs,
            vec![(
                "notes/old-name.md".to_string(),
                "notes/new-name.md".to_string()
            )]
        );
    }

    #[test]
    fn no_pairing_when_added_and_deleted_hashes_differ() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes/new-name.md", b"brand new content");
        let mut base = BTreeMap::new();
        base.insert(
            "notes/old-name.md".to_string(),
            stamp_for(b"entirely different content"),
        );

        let result = detect_local_changes(dir.path(), &base).unwrap();
        let pairs = pair_renames(&result, &base);
        assert!(pairs.is_empty(), "{pairs:?}");
    }
}
