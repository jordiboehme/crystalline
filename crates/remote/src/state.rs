//! The durable per-domain origin state: which repository and branch a domain
//! tracks, the base snapshot manifest change detection compares against, the
//! open share proposals and any outstanding conflicts.
//!
//! Everything here is rooted at a directory the caller passes in (the
//! service resolves it through `crystalline_core::config::origin_state_dir`,
//! one directory per domain):
//!
//! ```text
//! <dir>/state.json      the OriginState, saved atomically
//! <dir>/base/           a snapshot of the domain subtree as of base_commit
//! <dir>/conflicts/<id>/ per-conflict copies of the base and upstream sides
//! ```
//!
//! Relative paths are always forward-slash normalized in [`OriginState`] and
//! on the wire; the platform path separator only appears at the filesystem
//! boundary, in [`base_path`] and the conflict helpers below it. Every rel
//! path crossing that boundary is validated there: anything shaped like a
//! traversal, an absolute path or a Windows drive-prefix component is
//! rejected with [`RemoteError::State`] naming the offending path rather than
//! turned into a write outside the base tree. Upstream repository content is
//! untrusted input in this feature's threat model, so this validation is not
//! optional hardening, it is the boundary the rest of the crate relies on.
//!
//! Every operation here is plain, synchronous `std::fs`: this module knows
//! nothing about async runtimes, and callers on an async runtime wrap calls
//! in `spawn_blocking` themselves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::RemoteError;
use crate::merge::ConflictKind;

/// The current [`OriginState::version`]. `save` refuses to write a state
/// carrying any other value: a version mismatch means either corrupt state
/// or a future format this build does not understand, and guessing at either
/// is worse than failing loudly.
pub const CURRENT_VERSION: u32 = 1;

/// How many entries [`OriginState::push_history`] keeps.
const HISTORY_CAP: usize = 20;

const STATE_FILE_NAME: &str = "state.json";
const BASE_DIR_NAME: &str = "base";
const BASE_TMP_DIR_NAME: &str = "base.tmp";
const CONFLICTS_DIR_NAME: &str = "conflicts";
const CONFLICT_BASE_FILE_NAME: &str = "base";
const CONFLICT_UPSTREAM_FILE_NAME: &str = "upstream";

/// The durable state of one domain's connection to its GitHub origin.
///
/// Saved as `state.json` under the domain's origin state directory. Everything
/// needed to resume work after a restart lives here: what has been pulled in
/// already (`base_commit`, `files`), what is waiting for review (`proposals`),
/// what happened to proposals that already left (`history`) and what still
/// needs a human or agent decision (`conflicts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OriginState {
    /// The on-disk format version. Currently always [`CURRENT_VERSION`].
    pub version: u32,
    /// The repository this domain tracks, `owner/name`.
    pub repo: String,
    /// The branch this domain tracks.
    pub branch: String,
    /// The last origin commit fully integrated locally. Empty before the
    /// first pull.
    pub base_commit: String,
    /// The ETag of the last branch-ref probe, for conditional polling.
    pub ref_etag: Option<String>,
    /// When the branch was last checked for new upstream commits.
    pub last_checked: Option<DateTime<Utc>>,
    /// The base snapshot manifest: relative path to stamp, the prefilter
    /// [`crate::changes::detect_local_changes`] uses to skip hashing files
    /// that plainly have not changed.
    pub files: BTreeMap<String, BaseStamp>,
    /// Share proposals still open for review.
    pub proposals: Vec<Proposal>,
    /// Merged or discarded proposals kept for status display, newest first,
    /// capped at 20 by [`OriginState::push_history`].
    pub history: Vec<Proposal>,
    /// Conflicts from a previous pull still waiting to be resolved.
    pub conflicts: Vec<Conflict>,
}

/// The recorded shape of a file in the base snapshot: enough to tell, without
/// reading the file, whether it might have changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseStamp {
    /// The SHA-256 hex digest of the file's content as of the base snapshot.
    pub sha256: String,
    /// The file's size in bytes as of the base snapshot.
    pub size: u64,
}

/// A share proposal (a GitHub pull request) opened from local changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// The proposal number.
    pub number: u64,
    /// The web URL a human reviews the proposal at.
    pub url: String,
    /// The branch carrying the proposed commits.
    pub branch: String,
    /// The proposal title.
    pub title: String,
    /// When the proposal was opened.
    pub created_at: DateTime<Utc>,
    /// The proposal's current lifecycle state.
    pub status: ProposalStatus,
    /// The files the proposal changes.
    pub files: Vec<ProposedFile>,
}

/// The lifecycle state of a [`Proposal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProposalStatus {
    /// Still open for review.
    Open,
    /// Merged into the tracked branch.
    Merged,
    /// Closed without merging.
    Declined,
}

/// One file changed by a [`Proposal`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedFile {
    /// The file's repo-relative path.
    pub path: String,
    /// How the file changed.
    pub change: ProposedChange,
    /// The SHA-256 hex digest of the file's new content, absent when
    /// `change` is [`ProposedChange::Deleted`].
    pub sha256: Option<String>,
}

/// How a single file changed within a [`Proposal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProposedChange {
    /// The file is new in the proposal.
    Added,
    /// The file's content changed in the proposal.
    Modified,
    /// The file is removed by the proposal.
    Deleted,
}

/// An unresolved conflict from a previous pull, recorded so it can be
/// revisited: what path, what kind, and copies of the two sides that could
/// not be merged automatically (see [`record_conflict_files`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    /// Eight lowercase hex characters, naming `conflicts/<id>/` on disk. See
    /// [`new_conflict_id`].
    pub id: String,
    /// The file's path, relative to the domain root.
    pub path: String,
    /// How the conflict came about.
    pub kind: ConflictKind,
    /// The base commit in effect when the conflict was detected.
    pub base_commit: String,
    /// The upstream commit the conflicting change came from.
    pub upstream_commit: String,
    /// When the conflict was detected.
    pub detected_at: DateTime<Utc>,
}

impl OriginState {
    /// A fresh, empty state for a domain that has never pulled from `repo`
    /// yet: `base_commit` empty, every collection empty, at
    /// [`CURRENT_VERSION`].
    pub fn new(repo: impl Into<String>, branch: impl Into<String>) -> OriginState {
        OriginState {
            version: CURRENT_VERSION,
            repo: repo.into(),
            branch: branch.into(),
            base_commit: String::new(),
            ref_etag: None,
            last_checked: None,
            files: BTreeMap::new(),
            proposals: Vec::new(),
            history: Vec::new(),
            conflicts: Vec::new(),
        }
    }

    /// Loads the state saved under `dir`, or `None` when `dir/state.json`
    /// does not exist yet (a domain that has never connected to an origin).
    ///
    /// Any content that is not valid JSON in the expected shape is a
    /// [`RemoteError::State`] naming the path, not a panic and not a value
    /// silently treated as absent: corrupt state must never be mistaken for
    /// "no state yet".
    pub fn load(dir: &Path) -> Result<Option<OriginState>, RemoteError> {
        let path = dir.join(STATE_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map(Some)
                .map_err(|e| RemoteError::State(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Saves this state under `dir`, creating `dir` if it does not exist yet.
    ///
    /// Writes a temporary file in the same directory and renames it over
    /// `state.json`, so a reader (including a concurrent load from another
    /// process) never observes a partially written file. Refuses to save a
    /// state whose `version` is not [`CURRENT_VERSION`]: writing an
    /// unsupported version to disk would make it unreadable by the code that
    /// is supposed to load it back.
    pub fn save(&self, dir: &Path) -> Result<(), RemoteError> {
        if self.version != CURRENT_VERSION {
            return Err(RemoteError::State(format!(
                "cannot save origin state at version {}: this build only writes version {CURRENT_VERSION}",
                self.version
            )));
        }
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            RemoteError::State(format!("could not serialize the origin state: {e}"))
        })?;
        let path = dir.join(STATE_FILE_NAME);
        let tmp = dir.join(format!("{STATE_FILE_NAME}.tmp.{}", std::process::id()));
        std::fs::write(&tmp, json.as_bytes())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Inserts `proposal` at the front of `history` and truncates to the
    /// newest 20, so `history` always reads newest first and never grows
    /// without bound.
    pub fn push_history(&mut self, proposal: Proposal) {
        self.history.insert(0, proposal);
        self.history.truncate(HISTORY_CAP);
    }
}

/// The filesystem path of `rel`'s copy in the base snapshot under `dir`.
///
/// `rel` always uses forward slashes (the convention `OriginState` and every
/// other function in this module follow); this is the one place that gets
/// translated to the platform's own separator, via [`to_platform_path`],
/// which also validates it. A `rel` that is empty, absolute, normalizes to
/// no components at all, or has a `.`, `..`, `:` or `\` component is
/// rejected with [`RemoteError::State`] naming the offending path rather
/// than joined onto `dir`. Every base-tree writer and reader in this module
/// funnels through here, so this is the one place upstream content shaped
/// like a path-traversal or a Windows drive-prefix attempt is turned away
/// before it ever reaches the filesystem.
pub fn base_path(dir: &Path, rel: &str) -> Result<PathBuf, RemoteError> {
    Ok(dir.join(BASE_DIR_NAME).join(to_platform_path(rel)?))
}

/// Writes `bytes` as `rel`'s base snapshot copy under `dir`, creating any
/// parent directories it needs.
///
/// `rel` is validated by [`base_path`] before anything is written: a path
/// shaped like a traversal, an absolute path or a Windows drive-prefix
/// attempt is rejected with [`RemoteError::State`] naming the offending
/// path, and nothing is written.
pub fn write_base_file(dir: &Path, rel: &str, bytes: &[u8]) -> Result<(), RemoteError> {
    let path = base_path(dir, rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Reads `rel`'s base snapshot copy under `dir`, or `None` if it has never
/// been written.
///
/// `rel` is validated by [`base_path`]: a path shaped like a traversal, an
/// absolute path or a Windows drive-prefix attempt is rejected with
/// [`RemoteError::State`] naming the offending path rather than read.
pub fn read_base_file(dir: &Path, rel: &str) -> Result<Option<Vec<u8>>, RemoteError> {
    match std::fs::read(base_path(dir, rel)?) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Removes `rel`'s base snapshot copy under `dir`, then prunes any parent
/// directory left empty by the removal, all the way up to (but not
/// including) `base/` itself. Removing a file that was never written is not
/// an error.
///
/// `rel` is validated by [`base_path`]: a path shaped like a traversal, an
/// absolute path or a Windows drive-prefix attempt is rejected with
/// [`RemoteError::State`] naming the offending path rather than acted on.
pub fn remove_base_file(dir: &Path, rel: &str) -> Result<(), RemoteError> {
    let path = base_path(dir, rel)?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    let base_root = dir.join(BASE_DIR_NAME);
    let mut current = path.parent().map(Path::to_path_buf);
    while let Some(p) = current {
        if p == base_root || !p.starts_with(&base_root) {
            break;
        }
        let is_empty = std::fs::read_dir(&p).map(|mut entries| entries.next().is_none());
        match is_empty {
            Ok(true) => {
                std::fs::remove_dir(&p)?;
                current = p.parent().map(Path::to_path_buf);
            }
            _ => break,
        }
    }
    Ok(())
}

/// Wholesale-replaces the base snapshot under `dir` with exactly `files`:
/// every path in `files` is written, and every path that existed in the
/// previous base snapshot but is not in `files` is gone afterward.
///
/// Writes the new tree to a sibling `base.tmp/` directory first, then swaps
/// it in for `base/`, so a reader never sees a half-replaced tree.
///
/// Every key in `files` is validated by [`to_platform_path`] before
/// anything is written: the first one shaped like a traversal, an absolute
/// path or a Windows drive-prefix attempt fails the whole call with
/// [`RemoteError::State`] naming the offending path, and the previous
/// `base/` is left untouched.
pub fn replace_base_tree(dir: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), RemoteError> {
    let tmp_root = dir.join(BASE_TMP_DIR_NAME);
    if tmp_root.exists() {
        std::fs::remove_dir_all(&tmp_root)?;
    }
    std::fs::create_dir_all(&tmp_root)?;
    for (rel, bytes) in files {
        let path = tmp_root.join(to_platform_path(rel)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }

    let base_root = dir.join(BASE_DIR_NAME);
    if base_root.exists() {
        std::fs::remove_dir_all(&base_root)?;
    }
    std::fs::rename(&tmp_root, &base_root)?;
    Ok(())
}

/// Checks every path in `files` against its on-disk base snapshot copy,
/// returning the relative paths that are missing or whose size or content no
/// longer match the recorded [`BaseStamp`]. An empty result means the base
/// snapshot is fully intact. Used by recovery, orchestrated by a later task.
///
/// Every key in `files` is validated by [`base_path`]: the first one shaped
/// like a traversal, an absolute path or a Windows drive-prefix attempt
/// fails the whole call with [`RemoteError::State`] naming the offending
/// path.
pub fn verify_base(
    dir: &Path,
    files: &BTreeMap<String, BaseStamp>,
) -> Result<Vec<String>, RemoteError> {
    let mut bad = Vec::new();
    for (rel, stamp) in files {
        let path = base_path(dir, rel)?;
        match std::fs::metadata(&path) {
            Ok(meta) => {
                if meta.len() != stamp.size {
                    bad.push(rel.clone());
                    continue;
                }
                let bytes = std::fs::read(&path)?;
                if sha256_hex(&bytes) != stamp.sha256 {
                    bad.push(rel.clone());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bad.push(rel.clone()),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(bad)
}

/// The directory holding `id`'s recorded conflict copies under `dir`.
fn conflict_dir(dir: &Path, id: &str) -> PathBuf {
    dir.join(CONFLICTS_DIR_NAME).join(id)
}

/// Records the pre-advance base and upstream copies of a conflicting file
/// under `conflicts/<id>/`, so a later resolution step can present both
/// sides without needing them to still be reachable upstream or in the
/// working tree. `None` on either side (the file did not exist there) writes
/// no file for that side, and removes one if a previous call had written it.
pub fn record_conflict_files(
    dir: &Path,
    id: &str,
    base: Option<&[u8]>,
    upstream: Option<&[u8]>,
) -> Result<(), RemoteError> {
    let conflict_dir = conflict_dir(dir, id);
    std::fs::create_dir_all(&conflict_dir)?;
    write_or_clear(&conflict_dir.join(CONFLICT_BASE_FILE_NAME), base)?;
    write_or_clear(&conflict_dir.join(CONFLICT_UPSTREAM_FILE_NAME), upstream)?;
    Ok(())
}

fn write_or_clear(path: &Path, content: Option<&[u8]>) -> Result<(), RemoteError> {
    match content {
        Some(bytes) => std::fs::write(path, bytes)?,
        None => match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        },
    }
    Ok(())
}

/// The two sides of a recorded conflict: `(base, upstream)`, either `None` if
/// that side was never recorded (or was recorded absent).
pub type ConflictFiles = (Option<Vec<u8>>, Option<Vec<u8>>);

/// Reads back `id`'s recorded conflict copies.
pub fn read_conflict_files(dir: &Path, id: &str) -> Result<ConflictFiles, RemoteError> {
    let conflict_dir = conflict_dir(dir, id);
    let base = read_optional(&conflict_dir.join(CONFLICT_BASE_FILE_NAME))?;
    let upstream = read_optional(&conflict_dir.join(CONFLICT_UPSTREAM_FILE_NAME))?;
    Ok((base, upstream))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, RemoteError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Removes `id`'s recorded conflict copies entirely. Clearing a conflict
/// that was never recorded (or already cleared) is not an error.
pub fn clear_conflict(dir: &Path, id: &str) -> Result<(), RemoteError> {
    match std::fs::remove_dir_all(conflict_dir(dir, id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// A fresh conflict id: 8 lowercase hex characters drawn from a random UUID
/// v4, short enough to read comfortably in a `conflicts/<id>/` path while
/// keeping collisions practically impossible for the handful of conflicts a
/// domain has open at once.
pub fn new_conflict_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Converts a forward-slash relative path (the only form `OriginState` and
/// the wire ever use) into a platform-native [`PathBuf`], so paths compare
/// and serialize identically across every platform this compiles for.
///
/// This is the single chokepoint every base-tree writer and reader in this
/// module funnels through before a `rel` path ever touches the filesystem,
/// so the validation lives here once rather than being repeated at each
/// call site. Rejected with [`RemoteError::State`] naming the offending
/// path:
///
/// - an empty path, or one that normalizes to no components at all (for
///   example `"."`)
/// - a leading `/` (an absolute path, not a relative one)
/// - any `.` or `..` component
/// - any component containing `:` (a Windows drive-prefix former) or `\`
///   (kept out even though `archive.rs` already normalizes backslashes
///   before content reaches this layer, so this module's own guarantee
///   never depends on a caller upstream having done so)
fn to_platform_path(rel: &str) -> Result<PathBuf, RemoteError> {
    if rel.is_empty() || rel.starts_with('/') {
        return Err(RemoteError::State(format!(
            "rejected path outside the domain tree: {rel}"
        )));
    }
    let mut path = PathBuf::new();
    let mut has_component = false;
    for part in rel.split('/') {
        if part.is_empty() {
            continue;
        }
        if part == "." || part == ".." || part.contains(':') || part.contains('\\') {
            return Err(RemoteError::State(format!(
                "rejected path outside the domain tree: {rel}"
            )));
        }
        path.push(part);
        has_component = true;
    }
    if !has_component {
        return Err(RemoteError::State(format!(
            "rejected path outside the domain tree: {rel}"
        )));
    }
    Ok(path)
}

/// The lowercase hex SHA-256 digest of `bytes`, encoded exactly like
/// `crystalline_index::sync`'s own hasher so stamps stay comparable with the
/// search index's file stamps.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn sample_state() -> OriginState {
        let mut state = OriginState::new("acme/brand-knowledge", "main");
        state.base_commit = "deadbeef".to_string();
        state.files.insert(
            "notes/example.md".to_string(),
            BaseStamp {
                sha256: "a".repeat(64),
                size: 42,
            },
        );
        state
    }

    #[test]
    fn new_state_starts_at_version_one_and_empty() {
        let state = OriginState::new("acme/brand-knowledge", "main");
        assert_eq!(state.version, 1);
        assert_eq!(state.repo, "acme/brand-knowledge");
        assert_eq!(state.branch, "main");
        assert_eq!(state.base_commit, "");
        assert!(state.files.is_empty());
        assert!(state.proposals.is_empty());
        assert!(state.history.is_empty());
        assert!(state.conflicts.is_empty());
    }

    #[test]
    fn round_trip_save_then_load_equals_original() {
        let dir = tempfile::tempdir().unwrap();
        let state = sample_state();
        state.save(dir.path()).unwrap();
        let loaded = OriginState::load(dir.path()).unwrap();
        assert_eq!(loaded, Some(state));
    }

    #[test]
    fn load_returns_none_when_state_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(OriginState::load(dir.path()).unwrap(), None);
    }

    #[test]
    fn load_returns_state_error_naming_the_path_on_corrupt_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"not json").unwrap();
        let err = OriginState::load(dir.path()).unwrap_err();
        match err {
            crate::error::RemoteError::State(msg) => {
                assert!(msg.contains(&path.display().to_string()), "{msg}");
            }
            other => panic!("expected State error, got {other:?}"),
        }
    }

    #[test]
    fn save_leaves_no_tmp_file_behind_on_success() {
        let dir = tempfile::tempdir().unwrap();
        sample_state().save(dir.path()).unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["state.json".to_string()], "{entries:?}");
    }

    #[test]
    fn save_creates_the_directory_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("origin");
        sample_state().save(&nested).unwrap();
        assert!(nested.join("state.json").exists());
    }

    #[test]
    fn save_rejects_a_state_with_an_unsupported_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = sample_state();
        state.version = 2;
        let err = state.save(dir.path()).unwrap_err();
        assert!(matches!(err, crate::error::RemoteError::State(_)));
        assert!(!dir.path().join("state.json").exists());
    }

    #[test]
    fn write_read_remove_base_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_base_file(dir.path(), "notes/example.md", b"hello world").unwrap();
        assert_eq!(
            read_base_file(dir.path(), "notes/example.md").unwrap(),
            Some(b"hello world".to_vec())
        );
        remove_base_file(dir.path(), "notes/example.md").unwrap();
        assert_eq!(
            read_base_file(dir.path(), "notes/example.md").unwrap(),
            None
        );
    }

    #[test]
    fn read_base_file_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_base_file(dir.path(), "missing.md").unwrap(), None);
    }

    #[test]
    fn write_base_file_still_accepts_a_nested_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        write_base_file(dir.path(), "deep/nested/path/file.md", b"content").unwrap();
        assert_eq!(
            read_base_file(dir.path(), "deep/nested/path/file.md").unwrap(),
            Some(b"content".to_vec())
        );
    }

    #[test]
    fn write_base_file_rejects_a_parent_traversal_component() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_base_file(dir.path(), "../x.md", b"nope").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn read_base_file_rejects_a_parent_traversal_component() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_base_file(dir.path(), "../x.md").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn remove_base_file_rejects_a_parent_traversal_component() {
        let dir = tempfile::tempdir().unwrap();
        let err = remove_base_file(dir.path(), "../x.md").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn replace_base_tree_rejects_a_parent_traversal_component() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        files.insert("../x.md".to_string(), b"nope".to_vec());
        let err = replace_base_tree(dir.path(), &files).unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn verify_base_rejects_a_parent_traversal_component() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = BTreeMap::new();
        manifest.insert(
            "../x.md".to_string(),
            BaseStamp {
                sha256: sha256_hex(b"x"),
                size: 1,
            },
        );
        let err = verify_base(dir.path(), &manifest).unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn write_base_file_rejects_a_windows_drive_prefix_component() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_base_file(dir.path(), "notes/C:evil.md", b"nope").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn write_base_file_rejects_a_backslash_component() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_base_file(dir.path(), "a\\b.md", b"nope").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn write_base_file_rejects_an_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_base_file(dir.path(), "/abs.md", b"nope").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn write_base_file_rejects_an_empty_path() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_base_file(dir.path(), "", b"nope").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn write_base_file_rejects_a_path_that_normalizes_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_base_file(dir.path(), ".", b"nope").unwrap_err();
        assert!(matches!(err, RemoteError::State(_)));
    }

    #[test]
    fn remove_base_file_prunes_now_empty_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write_base_file(dir.path(), "a/b/c.md", b"content").unwrap();
        remove_base_file(dir.path(), "a/b/c.md").unwrap();
        assert!(!dir.path().join("base").join("a").exists());
        assert!(dir.path().join("base").exists());
    }

    #[test]
    fn remove_base_file_keeps_a_sibling_files_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_base_file(dir.path(), "a/b/c.md", b"content").unwrap();
        write_base_file(dir.path(), "a/keep.md", b"content").unwrap();
        remove_base_file(dir.path(), "a/b/c.md").unwrap();
        assert!(!dir.path().join("base").join("a").join("b").exists());
        assert!(dir.path().join("base").join("a").join("keep.md").exists());
    }

    #[test]
    fn replace_base_tree_swaps_content_and_removes_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        write_base_file(dir.path(), "stale.md", b"old").unwrap();

        let mut files = BTreeMap::new();
        files.insert("notes/new.md".to_string(), b"new content".to_vec());
        replace_base_tree(dir.path(), &files).unwrap();

        assert_eq!(read_base_file(dir.path(), "stale.md").unwrap(), None);
        assert_eq!(
            read_base_file(dir.path(), "notes/new.md").unwrap(),
            Some(b"new content".to_vec())
        );
        assert!(!dir.path().join("base.tmp").exists());
    }

    #[test]
    fn verify_base_reports_missing_mismatched_and_intact() {
        let dir = tempfile::tempdir().unwrap();
        write_base_file(dir.path(), "intact.md", b"same content").unwrap();
        write_base_file(dir.path(), "mismatched.md", b"changed on disk").unwrap();

        let mut manifest = BTreeMap::new();
        manifest.insert(
            "intact.md".to_string(),
            BaseStamp {
                sha256: sha256_hex(b"same content"),
                size: "same content".len() as u64,
            },
        );
        manifest.insert(
            "mismatched.md".to_string(),
            BaseStamp {
                sha256: sha256_hex(b"original content"),
                size: "original content".len() as u64,
            },
        );
        manifest.insert(
            "missing.md".to_string(),
            BaseStamp {
                sha256: sha256_hex(b"never written"),
                size: "never written".len() as u64,
            },
        );

        let mut bad = verify_base(dir.path(), &manifest).unwrap();
        bad.sort();
        assert_eq!(
            bad,
            vec!["mismatched.md".to_string(), "missing.md".to_string()]
        );
    }

    #[test]
    fn record_and_read_conflict_files_round_trip_including_absent_sides() {
        let dir = tempfile::tempdir().unwrap();
        let id = "abc12345";
        record_conflict_files(
            dir.path(),
            id,
            Some(b"base content"),
            Some(b"upstream content"),
        )
        .unwrap();
        let (base, upstream) = read_conflict_files(dir.path(), id).unwrap();
        assert_eq!(base, Some(b"base content".to_vec()));
        assert_eq!(upstream, Some(b"upstream content".to_vec()));
    }

    #[test]
    fn record_conflict_files_with_an_absent_side_reads_back_none() {
        let dir = tempfile::tempdir().unwrap();
        let id = "def67890";
        record_conflict_files(dir.path(), id, None, Some(b"upstream only")).unwrap();
        let (base, upstream) = read_conflict_files(dir.path(), id).unwrap();
        assert_eq!(base, None);
        assert_eq!(upstream, Some(b"upstream only".to_vec()));
    }

    #[test]
    fn clear_conflict_removes_the_conflict_directory() {
        let dir = tempfile::tempdir().unwrap();
        let id = "11112222";
        record_conflict_files(dir.path(), id, Some(b"base"), Some(b"upstream")).unwrap();
        clear_conflict(dir.path(), id).unwrap();
        let (base, upstream) = read_conflict_files(dir.path(), id).unwrap();
        assert_eq!(base, None);
        assert_eq!(upstream, None);
    }

    #[test]
    fn clear_conflict_on_an_unknown_id_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        clear_conflict(dir.path(), "ffffffff").unwrap();
    }

    #[test]
    fn new_conflict_id_is_eight_lowercase_hex_chars() {
        let id = new_conflict_id();
        assert_eq!(id.len(), 8, "{id}");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{id}"
        );
    }

    #[test]
    fn new_conflict_id_is_not_trivially_constant() {
        let a = new_conflict_id();
        let b = new_conflict_id();
        assert_ne!(a, b);
    }

    #[test]
    fn push_history_inserts_at_front_and_caps_at_twenty() {
        let mut state = OriginState::new("acme/brand-knowledge", "main");
        for i in 0..25u64 {
            state.push_history(Proposal {
                number: i,
                url: format!("https://github.com/acme/brand-knowledge/pull/{i}"),
                branch: format!("share/{i}"),
                title: format!("Proposal {i}"),
                created_at: chrono::Utc::now(),
                status: ProposalStatus::Merged,
                files: Vec::new(),
            });
        }
        assert_eq!(state.history.len(), 20);
        assert_eq!(state.history.first().unwrap().number, 24);
        assert_eq!(state.history.last().unwrap().number, 5);
    }

    #[test]
    fn proposed_file_carries_an_optional_sha256() {
        let added = ProposedFile {
            path: "notes/new.md".to_string(),
            change: ProposedChange::Added,
            sha256: Some(sha256_hex(b"content")),
        };
        let deleted = ProposedFile {
            path: "notes/old.md".to_string(),
            change: ProposedChange::Deleted,
            sha256: None,
        };
        assert!(added.sha256.is_some());
        assert!(deleted.sha256.is_none());
    }
}
