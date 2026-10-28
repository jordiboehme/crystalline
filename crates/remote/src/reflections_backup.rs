//! The backup of a home instance's Reflection: the whole `reflections/`
//! folder, committed to its own branch of the domain's repository. Single
//! writer (the home instance), so there is no merge; the branch shares no
//! history with the tracked one, so it never carries an engram.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::RemoteError;
use crate::provider::{OriginSpec, Provider, TreeWrite};

/// The branch a Reflection is backed up to. It shares the `crystalline/`
/// prefix with the share branches, so a cleanup that ever deletes branches
/// by prefix must name this one as an exception.
pub const BACKUP_BRANCH: &str = "crystalline/reflections";

const STATE_FILE: &str = "reflections-backup.json";

/// What the last backup committed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BackupState {
    /// Schema version, 1.
    pub v: u32,
    /// The commit the branch pointed at after the last backup.
    pub head: Option<String>,
    /// Repo path to SHA-256 of what that commit holds.
    pub files: BTreeMap<String, String>,
}

/// What one backup did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupOutcome {
    /// The folder matches the last backup.
    NothingToBackUp,
    /// A commit landed on the branch.
    Committed {
        /// The new head of [`BACKUP_BRANCH`].
        sha: String,
        /// How many paths the commit wrote or removed.
        files: usize,
    },
}

/// The Reflection's folders at `root`: every top-level directory whose name
/// folds to `reflections`, in their spelling on disk, sorted. The layout
/// folds the case too, so a folder spelled `Reflections` is backed up on a
/// file system that keeps the case.
fn reflection_folders(root: &Path) -> Result<Vec<PathBuf>, RemoteError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(RemoteError::working_tree(".", e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| RemoteError::working_tree(".", e))?;
        let name = entry.file_name();
        let folds = name
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(crystalline_core::reflection::REFLECTIONS_DIR));
        if folds && entry.path().is_dir() {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

/// Every file under `root/reflections` the layout backs up
/// (`PathClass::backs_up` with the Reflection in effect: pages, the pact,
/// threads, the hidden baseline and the Reflection's files, never a temp or
/// stray file), by repo path. A walk error is an error: an entry that cannot
/// be read must never look like a deleted one.
pub(crate) fn collect(
    root: &Path,
    subpath: Option<&str>,
) -> Result<BTreeMap<String, Vec<u8>>, RemoteError> {
    let layout = crystalline_core::DomainLayout::bare(crystalline_core::ReflectionMode::Effective);
    let prefix = subpath
        .map(|s| s.trim_matches('/'))
        .filter(|s| !s.is_empty());
    let mut out = BTreeMap::new();
    for dir in reflection_folders(root)? {
        for entry in WalkDir::new(&dir) {
            let entry = entry.map_err(|err| {
                let at = err
                    .path()
                    .map(|p| crystalline_core::slash_rel(root, p))
                    .unwrap_or_else(|| crystalline_core::reflection::REFLECTIONS_DIR.to_string());
                RemoteError::working_tree(
                    at,
                    err.into_io_error()
                        .unwrap_or_else(|| std::io::Error::other("the folder walk failed")),
                )
            })?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = crystalline_core::slash_rel(root, entry.path());
            if !layout.classify(&rel).backs_up() {
                continue;
            }
            let repo = match prefix {
                Some(sub) => format!("{sub}/{rel}"),
                None => rel.clone(),
            };
            let bytes = std::fs::read(entry.path())
                .map_err(|source| RemoteError::WorkingTree { path: rel, source })?;
            out.insert(repo, bytes);
        }
    }
    Ok(out)
}

fn load(state_dir: &Path) -> BackupState {
    std::fs::read(state_dir.join(STATE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(state_dir: &Path, state: &BackupState) -> Result<(), RemoteError> {
    std::fs::create_dir_all(state_dir)?;
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| RemoteError::State(e.to_string()))?;
    // Through a temp file and a rename, so a crash mid-write leaves the old
    // state or the new one, never half of one.
    let tmp = state_dir.join(format!("{STATE_FILE}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, state_dir.join(STATE_FILE))?;
    Ok(())
}

/// The origin's subpath, trimmed, or `None` for the repository root.
fn prefix(spec: &OriginSpec) -> Option<&str> {
    spec.subpath
        .as_deref()
        .map(|s| s.trim_matches('/'))
        .filter(|s| !s.is_empty())
}

/// The repo path `path` below the Reflection folder of the domain at
/// `prefix` (the folder's own name in any case left off), or `None` when it
/// is not inside it.
fn inner_path<'a>(path: &'a str, prefix: Option<&str>) -> Option<&'a str> {
    let rest = match prefix {
        Some(sub) => path.strip_prefix(sub)?.strip_prefix('/')?,
        None => path,
    };
    let (first, inner) = rest.split_once('/')?;
    first
        .eq_ignore_ascii_case(crystalline_core::reflection::REFLECTIONS_DIR)
        .then_some(inner)
}

/// Whether the repo path `path` is inside the Reflection of the domain at
/// `prefix`: its first segment below the prefix folds to `reflections`.
fn is_ours(path: &str, prefix: Option<&str>) -> bool {
    let rest = match prefix {
        Some(sub) => path.strip_prefix(sub).and_then(|r| r.strip_prefix('/')),
        None => Some(path),
    };
    rest.and_then(|r| r.split('/').next()).is_some_and(|first| {
        first.eq_ignore_ascii_case(crystalline_core::reflection::REFLECTIONS_DIR)
    })
}

/// Whether the branch went from `saved` to `remote` without touching this
/// domain's Reflection. A comparison that fails or is cut short answers no,
/// so the caller rewrites every file rather than trusting a stale state.
async fn moved_by_others_only(
    provider: &dyn Provider,
    spec: &OriginSpec,
    saved: &str,
    remote: &str,
    prefix: Option<&str>,
) -> bool {
    match provider.compare(spec, saved, remote).await {
        Ok(cmp) if !cmp.truncated => !cmp.files.iter().any(|change| {
            is_ours(&change.path, prefix)
                || matches!(&change.kind, crate::provider::ChangeKind::Renamed { previous } if is_ours(previous, prefix))
        }),
        _ => false,
    }
}

/// Commit what changed under `domain_root/reflections` since the last
/// backup to [`BACKUP_BRANCH`]. The state is saved only after the branch
/// moved, so a failure anywhere before leaves the next attempt to commit the
/// same change once.
///
/// `held` names files inside the Reflection (paths below `reflections/`)
/// the caller holds back: they are left out as if they were not on disk, so
/// a copy the branch head holds is removed from it (its history keeps it),
/// and they go out again on the first backup that no longer holds them.
pub async fn backup_reflections(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    message: &str,
    held: &BTreeSet<String>,
) -> Result<BackupOutcome, RemoteError> {
    let mut state = load(state_dir);
    let mut files = collect(domain_root, spec.subpath.as_deref())?;
    if !held.is_empty() {
        let sub = prefix(spec);
        files.retain(|path, _| !inner_path(path, sub).is_some_and(|inner| held.contains(inner)));
    }
    let hashes: BTreeMap<String, String> = files
        .iter()
        .map(|(p, b)| (p.clone(), crate::state::sha256_hex(b)))
        .collect();
    let remote_head = provider.branch_ref(spec, BACKUP_BRANCH).await?;
    if hashes == state.files && remote_head.is_some() && remote_head == state.head {
        return Ok(BackupOutcome::NothingToBackUp);
    }
    // A folder with nothing in it and no branch yet has nothing to start a
    // branch with: no commit, and no endless retry.
    if files.is_empty() && remote_head.is_none() {
        return Ok(BackupOutcome::NothingToBackUp);
    }
    // A head this instance did not write (the state file was lost, a crash
    // came between the branch move and the state save, or the branch moved
    // under it) says nothing about what the branch holds, so every file is
    // written, not the difference against a stale state. A head that moved
    // only through another domain of the same repository (another subpath
    // shares the branch) left this domain's files as the state says, so the
    // difference is enough.
    let lost = match (&remote_head, &state.head) {
        (Some(remote), Some(saved)) if remote != saved => {
            !moved_by_others_only(provider, spec, saved, remote, prefix(spec)).await
        }
        (Some(_), None) => true,
        _ => false,
    };
    let mut writes = Vec::new();
    for (path, bytes) in &files {
        let fresh = remote_head.is_none() || lost || state.files.get(path) != hashes.get(path);
        if fresh {
            writes.push(TreeWrite {
                path: path.clone(),
                blob_sha: Some(provider.create_blob(spec, bytes).await?),
            });
        }
    }
    let sha = match remote_head {
        None => {
            // A root tree: the branch starts with no parent and holds the
            // Reflection alone, so it never shares a file with the tracked
            // branch.
            let tree = provider.create_root_tree(spec, &writes).await?;
            let commit = provider.create_commit(spec, message, &tree, &[]).await?;
            provider.create_branch(spec, BACKUP_BRANCH, &commit).await?;
            commit
        }
        Some(head) => {
            for path in state.files.keys().filter(|p| !hashes.contains_key(*p)) {
                // A head this instance did not write may no longer hold the
                // path (the commit a crash kept from the state removed it
                // already): a delete goes out only for a path the head holds,
                // never a delete of nothing.
                if lost && provider.read_file(spec, &head, path).await?.is_none() {
                    continue;
                }
                writes.push(TreeWrite {
                    path: path.clone(),
                    blob_sha: None,
                });
            }
            if writes.is_empty() {
                // The state lagged the folder only in what it recorded: no
                // commit for nothing; the state catches up.
                state.v = 1;
                state.head = Some(head);
                state.files = hashes;
                save(state_dir, &state)?;
                return Ok(BackupOutcome::NothingToBackUp);
            }
            let tree = provider.create_tree(spec, &head, &writes).await?;
            let commit = provider
                .create_commit(spec, message, &tree, std::slice::from_ref(&head))
                .await?;
            provider
                .update_branch(spec, BACKUP_BRANCH, &commit, false)
                .await?;
            commit
        }
    };
    state.v = 1;
    state.head = Some(sha.clone());
    state.files = hashes;
    save(state_dir, &state)?;
    Ok(BackupOutcome::Committed {
        sha,
        files: writes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_paths_carry_the_subpath_and_the_hidden_baseline() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for rel in [
            "reflections/a.md",
            "reflections/a.threads.md",
            "reflections/.reflected/a.md",
            "reflections/.a.md.tmp",
            "notes/engram.md",
        ] {
            let abs = root.join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(abs, rel).unwrap();
        }
        let files = collect(root, Some("kb/eng")).unwrap();
        let paths: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            [
                "kb/eng/reflections/.reflected/a.md",
                "kb/eng/reflections/a.md",
                "kb/eng/reflections/a.threads.md"
            ],
            "the layout's backs_up: the baseline yes, a temp file no"
        );
        let root_files = collect(root, None).unwrap();
        assert!(
            root_files.keys().all(|p| p.starts_with("reflections/")),
            "never an engram"
        );
    }

    #[test]
    fn a_folder_spelled_with_capitals_is_backed_up_in_its_own_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("Reflections/.reflected")).unwrap();
        std::fs::write(root.join("Reflections/a.md"), "# A\n").unwrap();
        std::fs::write(root.join("Reflections/.reflected/a.md"), "# A\n").unwrap();
        std::fs::create_dir_all(root.join("reflectionsx")).unwrap();
        std::fs::write(root.join("reflectionsx/b.md"), "# B\n").unwrap();
        let files = collect(root, None).unwrap();
        let paths: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(paths, ["Reflections/.reflected/a.md", "Reflections/a.md"]);
    }

    #[test]
    fn only_paths_inside_the_domains_reflection_are_its_own() {
        assert!(is_ours("reflections/a.md", None));
        assert!(is_ours("Reflections/a.md", None));
        assert!(is_ours("kb/eng/reflections/a.md", Some("kb/eng")));
        assert!(!is_ours("kb/ops/reflections/a.md", Some("kb/eng")));
        assert!(!is_ours("kb/eng/notes/a.md", Some("kb/eng")));
        assert!(!is_ours("kb/engx/reflections/a.md", Some("kb/eng")));
        assert!(!is_ours("kb/eng/reflections/a.md", None));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_read_fails_the_backup() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("reflections/locked")).unwrap();
        std::fs::write(root.join("reflections/locked/a.md"), "x").unwrap();
        std::fs::set_permissions(
            root.join("reflections/locked"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        // A user the permissions do not stop (root) reads everything; then
        // there is nothing to prove.
        let enforced = std::fs::read_dir(root.join("reflections/locked")).is_err();
        let result = collect(root, None);
        std::fs::set_permissions(
            root.join("reflections/locked"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        if enforced {
            assert!(
                result.is_err(),
                "an unreadable folder is an error, never an empty one"
            );
        }
    }
}
