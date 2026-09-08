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

    /// Whether this change is a generated directory index rather than
    /// knowledge somebody wrote.
    ///
    /// An index refresh rides along with a share so the team repository stays
    /// browsable, but it is derived from the files beside it and says nothing
    /// on its own. Every surface that counts unshared work to decide whether
    /// to offer sharing at all leaves these out, and every surface that lists
    /// what a share carries draws them as one quiet line rather than among the
    /// engrams.
    pub fn is_generated_index(&self) -> bool {
        crystalline_core::is_index_path(self.path())
    }
}

/// The result of comparing a domain's working tree against its base
/// snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalChanges {
    /// Every detected change, sorted by path. Walk order differs per
    /// filesystem and classification does not all happen during the walk
    /// anyway, so the paths' own order is the only stable one.
    pub changes: Vec<LocalChange>,
    /// Files skipped for exceeding [`MAX_SHARED_FILE_BYTES`], with their
    /// sizes in bytes.
    pub skipped_large: Vec<(String, u64)>,
    /// For every base-snapshot path the walk found on disk under a spelling
    /// that differs only in case, that spelling on disk. Keyed by the base's
    /// spelling, which is the one every change, every proposal and every
    /// snapshot record uses. Read it through [`LocalChanges::disk_path`].
    ///
    /// This map is the difference between a path a share can name and a path a
    /// share can open. A case-only difference is reported at the base spelling
    /// so what travels upstream is the name the repository already knows, and
    /// on a case-insensitive filesystem that name happens to open the file too.
    /// On a case-sensitive one it does not, and without this map every read
    /// behind such a change would fail with a bare `No such file or directory`
    /// naming a path the user cannot see.
    ///
    /// Every adoption is recorded, including the ones that produced no change
    /// at all, so a later replay of a recorded layer can resolve a path it was
    /// handed as well.
    pub disk_paths: BTreeMap<String, String>,
}

impl LocalChanges {
    /// How many changes are knowledge somebody wrote: everything except a
    /// generated directory index.
    ///
    /// This is the number that decides whether a domain has anything to say -
    /// the count `status` reports, the one a share button and a picker badge
    /// read. A domain whose only difference from its origin is a handful of
    /// refreshed listings has nothing to say, and offering to share it would be
    /// offering churn.
    pub fn substantive_count(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| !c.is_generated_index())
            .count()
    }

    /// How many changes are generated directory indexes, the other half of
    /// [`Self::substantive_count`]. What the surfaces render as one line.
    pub fn index_count(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.is_generated_index())
            .count()
    }

    /// Where to actually open `reported` on this machine.
    ///
    /// Every caller that reads a file behind a change must go through this
    /// rather than joining the reported path onto the domain root: a change
    /// whose spelling on disk differs only in case is reported at the base's
    /// spelling, and on a case-sensitive filesystem that spelling opens
    /// nothing. Identity for every other path, which is nearly all of them.
    pub fn disk_path<'a>(&'a self, reported: &'a str) -> &'a str {
        self.disk_paths
            .get(reported)
            .map(String::as_str)
            .unwrap_or(reported)
    }
}

/// Detects local changes in `domain_root` relative to `base`, the base
/// snapshot manifest from [`crate::state::OriginState::files`].
///
/// Walk rules, mirroring `crystalline_index::sync`'s conventions:
///
/// - dot-files and dot-directories are skipped at any depth; the domain root
///   itself is never pruned, even if its own name starts with a dot.
/// - `log.md` is skipped at any depth: an append-only activity log is written
///   rather than derived, and sharing one would collide on every line.
///   `index.md` is NOT skipped - the generated directory index travels with the
///   domain so the team repository stays browsable (see [`is_excluded_path`]).
/// - every non-hidden file is included regardless of extension.
/// - a file larger than [`MAX_SHARED_FILE_BYTES`] is reported in
///   `skipped_large` instead of being hashed or classified as a change.
/// - a file with no base entry is [`LocalChange::Added`].
/// - a file with a base entry is hashed and compared against the recorded
///   size and digest; content that still matches (a file touched or rewritten
///   with identical bytes, whatever its new mtime) is not a change at all,
///   and anything else is [`LocalChange::Modified`].
/// - a base entry with no file on disk is [`LocalChange::Deleted`], unless
///   another base entry differing from it only in case IS on disk, in which
///   case nothing is reported: the two cannot be told apart on a
///   case-insensitive filesystem, and offering to delete one of them is
///   offering to delete the file the user can see.
/// - a file whose path differs from exactly one base entry by case alone is
///   that entry, not an addition plus a deletion. The case-folding pass in the
///   body states the exact conditions, and why a case-sensitive filesystem is
///   handled by those conditions rather than by probing for one.
///
/// Relative paths are always forward-slash normalized, regardless of
/// platform, and the returned changes are sorted by path: directory walk
/// order differs per filesystem, and everything downstream - proposal
/// bodies, outcome listings, previews - reads better and tests stably when
/// the order is the paths' own.
pub fn detect_local_changes(
    domain_root: &Path,
    base: &BTreeMap<String, BaseStamp>,
) -> Result<LocalChanges, RemoteError> {
    let mut changes = Vec::new();
    let mut skipped_large = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // Walked files with no byte-exact base entry. Held back until the walk is
    // over so the case-folding pass below can weigh every disk path at once
    // rather than guess from whatever walk order the filesystem happened to
    // hand us. The hash is `Some` for a file that was read, `None` for one
    // skipped as too large: such a file reports no change either way, but it
    // still has to be able to claim its base entry so that entry is not
    // reported deleted.
    let mut unmatched: Vec<(String, u64, Option<String>)> = Vec::new();

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
        if is_excluded_name(&fname) {
            continue;
        }

        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let rel = rel_path(domain_root, entry.path());
        let size = meta.len();
        seen.insert(rel.clone());

        if size > MAX_SHARED_FILE_BYTES {
            if !base.contains_key(&rel) {
                unmatched.push((rel.clone(), size, None));
            }
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
                unmatched.push((rel, size, Some(sha256)));
            }
        }
    }

    // Case-only path differences are the same file, not an addition plus a
    // deletion.
    //
    // Why this exists: git can hold `Classes/Common/a.md` and
    // `classes/common/a.md` at once; macOS APFS and Windows NTFS cannot. So a
    // team repository that once carried both spellings, then collapsed them
    // into one with a rename, leaves a member's base snapshot holding the old
    // spelling while the walk yields the new one. Comparing the two as byte
    // strings reported that file both `Added` under the new name and `Deleted`
    // under the old one, and the deletion was shareable: one click from
    // proposing that the team repository delete a file that exists.
    //
    // The fold is applied only where it is unambiguous. A walked path with no
    // exact base entry adopts a base entry when all three hold:
    //
    //   1. exactly one base key folds to the same value, so there is no
    //      question which entry is meant,
    //   2. that base key was not itself found on disk, so nothing is being
    //      taken away from a file that matched exactly, and
    //   3. exactly one unmatched walked path folds to that value, so two disk
    //      files are never both claiming one base entry.
    //
    // Fail any of them and nothing changes: the walked path is `Added` and the
    // base key is `Deleted`, exactly as before. Those are genuinely ambiguous
    // trees, and there guessing is worse than reporting - a repository holding
    // both spellings is what verify rule `E009` exists to flag.
    //
    // Case-sensitive filesystems: this deliberately does NOT probe the
    // filesystem, and relies on the guard above alone. Four reasons.
    //
    //   - The guard already covers the case that matters: two files differing
    //     only in case coexisting on ext4 or a case-sensitive APFS volume.
    //     That is two situations, and each is safe for its own reason. If the
    //     base records both spellings, both files match byte-exactly, neither
    //     is unmatched and this pass is never reached at all. If the base
    //     records only one, the other file is unmatched and does reach the
    //     pass - and condition 2 rejects it, because the base key it would
    //     fold onto is sitting in `seen`, matched by the file beside it.
    //   - A probe would answer the wrong question. The base snapshot may have
    //     been written on another machine and another filesystem than the one
    //     walking now - pulled by Linux CI, `status` run on a mac - so the
    //     local filesystem's case sensitivity says nothing about where the
    //     recorded spellings came from.
    //   - One probe could not even answer for one domain. A domain root can
    //     span mount points of differing case sensitivity, so a probe at the
    //     root would be wrong for the paths below the other mount.
    //   - It would add an IO failure mode to a function whose only IO today is
    //     reading files it is already walking.
    //
    // What it costs, on every platform: a deliberate case-only rename (the old
    // spelling gone, the new one present, both otherwise unique) is read as
    // the file it already was, so it is not shared. That is a real capability
    // given up. A case-only rename is perfectly representable everywhere -
    // it leaves one path, and APFS and NTFS are both case-preserving - so
    // nothing about the filesystems excuses the loss.
    //
    // It is given up because this function cannot tell the two apart. A
    // deliberate rename and a spelling collapsed upstream and then pulled
    // present identically: the base holds one spelling, the disk holds the
    // other, the content is the same. No local evidence separates them. So the
    // asymmetry of being wrong decides it. Read it as a rename and the
    // upstream case is a proposal to delete a teammate's file; read it as the
    // same file and the deliberate case is one rename that has to be made
    // again. A missed change costs a later sync, a phantom deletion costs
    // somebody their work.
    let mut folded_base: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for key in base.keys() {
        folded_base.entry(fold_case(key)).or_default().push(key);
    }
    let mut folded_disk: BTreeMap<String, usize> = BTreeMap::new();
    for (rel, _, _) in &unmatched {
        *folded_disk.entry(fold_case(rel)).or_default() += 1;
    }

    let mut adopted: BTreeSet<&str> = BTreeSet::new();
    let mut disk_paths: BTreeMap<String, String> = BTreeMap::new();
    for (rel, size, sha256) in &unmatched {
        let folded = fold_case(rel);
        let claim = folded_base
            .get(&folded)
            .filter(|keys| keys.len() == 1)
            .filter(|_| folded_disk.get(&folded) == Some(&1))
            .map(|keys| keys[0])
            .filter(|key| !seen.contains(key.as_str()));

        if let Some(key) = claim {
            adopted.insert(key.as_str());
            disk_paths.insert(key.clone(), rel.clone());
        }

        match (claim, sha256) {
            (Some(key), Some(sha256)) => {
                // The base's spelling, not the disk's: what travels upstream
                // has to be the name the repository already knows. That name
                // is, by construction, one the walk never saw - adoption
                // requires it not be in `seen` - so on a case-sensitive
                // filesystem it opens nothing, and this branch is reachable
                // there: a Linux user who re-cases a directory and then edits
                // a file inside it lands exactly here. `disk_paths` carries
                // the spelling that does open, so the share reads the file
                // that exists while the proposal writes the name the
                // repository knows.
                let stamp = &base[key];
                if stamp.size != *size || stamp.sha256 != *sha256 {
                    changes.push(LocalChange::Modified {
                        path: key.clone(),
                        sha256: sha256.clone(),
                    });
                }
            }
            // Too large to hash or share, so no change either way; claiming
            // the base entry above is the whole point, so it is not reported
            // gone.
            (Some(_), None) => {}
            (None, Some(sha256)) => changes.push(LocalChange::Added {
                path: rel.clone(),
                sha256: sha256.clone(),
            }),
            (None, None) => {}
        }
    }

    // A base entry with no file on disk is a deletion - unless another base
    // entry that differs from it only in case IS on disk.
    //
    // That is the ambiguity the fold above declines to resolve, seen from the
    // other side. The base holds `Common/X.md` and `common/X.md`; a macOS or
    // Windows checkout can hold only one of them, so one matches byte-exactly
    // and the other looks gone. It is not gone, it was never checked out, and
    // on that machine there is no evidence at all that would tell those two
    // apart - `E009` reports the pair, but only where the pair can exist, so
    // never on the platform that suffers from it, and a verify finding does
    // not gate a share in any case.
    //
    // So the same asymmetry that governs the fold governs this: reporting the
    // deletion offers to remove a file the user can see, one click from
    // proposing that a teammate's work be deleted, while suppressing it on a
    // case-sensitive filesystem loses a real deletion, which costs one later
    // sync once the pair is renamed apart. This does not touch the fold and
    // guesses at nothing: it declines to claim a file is gone when a file that
    // is indistinguishable from it on this machine is right there.
    let removed_but_for_case: BTreeSet<&str> = folded_base
        .values()
        .filter(|keys| keys.len() > 1 && keys.iter().any(|k| seen.contains(k.as_str())))
        .flat_map(|keys| keys.iter().map(|k| k.as_str()))
        .collect();

    for rel in base.keys() {
        if !seen.contains(rel)
            && !adopted.contains(rel.as_str())
            && !removed_but_for_case.contains(rel.as_str())
        {
            changes.push(LocalChange::Deleted { path: rel.clone() });
        }
    }

    changes.sort_by(|a, b| a.path().cmp(b.path()));

    Ok(LocalChanges {
        changes,
        skipped_large,
        disk_paths,
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

/// The single hidden-path rule every ingestion boundary in this crate must
/// apply, so [`crate::state::OriginState::files`] only ever contains paths
/// [`detect_local_changes`] can also see: without this, a hidden file stamped
/// into the base snapshot by one path but skipped by the working-tree walk
/// looks like a local deletion in `status`, and worse, like something to
/// delete upstream in a share proposal.
///
/// True when `rel_path` (a forward-slash relative path, domain-rooted) is, or
/// sits under, a hidden directory or file: any `/`-separated component for
/// which [`is_hidden`] is true. This mirrors the walk [`detect_local_changes`]
/// performs component by component, so a component that is itself
/// [`DOMAIN_CONFIG_FILE_NAME`] never makes the whole path hidden on its own,
/// but a hidden ancestor directory still does even when the leaf name is
/// `.crystalline.yaml` (the walk would have pruned that directory before ever
/// reaching the file, so this function must agree).
///
/// Used by [`crate::archive::extract_tarball`] (skipping hidden entries
/// before they are written or stamped into a base snapshot) and
/// [`crate::ops`]'s compare-path filter (dropping hidden upstream changes
/// before a blob is ever fetched for one).
pub(crate) fn is_hidden_path(rel_path: &str) -> bool {
    rel_path.split('/').any(is_hidden)
}

/// The full "never travels with a domain" rule: a hidden path, or the OKF
/// reserved log file (`log.md`).
///
/// The two reserved names part company here. `log.md` is an append-only
/// activity log, written rather than derived, and two members appending to
/// their own copies would collide on every line, so it never travels.
/// `index.md` is generated from the files it lists, and a team repository whose
/// folders carry no index is not browsable on the forge at all - so it does
/// travel, as an ordinary entry in snapshots, share trees and layer records.
/// What keeps that convergent rather than a tug of war is that the local
/// generator stays the single authority on its content: a pull records the
/// origin's copy in the base snapshot and never writes it to disk (see
/// [`crate::ops::pull`]), so the next share simply carries the locally
/// generated listing upstream.
///
/// Both the walk here (through [`is_excluded_name`], which must agree with
/// this) and every ingestion boundary apply this same rule, so an excluded
/// file never lands in a base snapshot the walk cannot see again.
pub(crate) fn is_excluded_path(rel_path: &str) -> bool {
    is_hidden_path(rel_path)
        || (crystalline_core::is_reserved_path(rel_path)
            && !crystalline_core::is_index_path(rel_path))
}

/// [`is_excluded_path`]'s rule stated over a bare filename, for the walk, which
/// meets each name on its own rather than as a whole path. The two must always
/// agree: a name this admits and that path rule rejects would be stamped into a
/// base snapshot the walk can never see again.
fn is_excluded_name(name: &str) -> bool {
    is_hidden(name)
        || (crystalline_core::is_reserved_file(name) && !crystalline_core::is_index_file(name))
}

/// Two paths' case-insensitive identity: what macOS and Windows treat as one
/// path, from [`crystalline_core::fold_path_case`].
///
/// Verify rule `E009` (`crystalline_core::verify`, `format::check_domain`)
/// asks the same question about one domain's files, and the two must fold the
/// same way or the rule and this function disagree about which paths collide.
/// They call one implementation rather than being kept in step by hand.
///
/// Folding coarsely is the safe direction here: two paths folded together
/// that a filesystem would keep apart trip the ambiguity guard in
/// [`detect_local_changes`], which then changes nothing at all.
fn fold_case(path: &str) -> String {
    crystalline_core::fold_path_case(path)
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
    fn is_excluded_path_covers_hidden_paths_and_the_activity_log_but_not_the_index() {
        // The generated directory index travels with a domain, at the root and
        // anywhere below it, so the team repository stays browsable; the
        // activity log never does, and neither does anything hidden.
        assert!(!is_excluded_path("index.md"));
        assert!(!is_excluded_path("runbooks/index.md"));
        assert!(is_excluded_path("log.md"));
        assert!(is_excluded_path("runbooks/log.md"));
        assert!(is_excluded_path(".github/workflows/ci.yml"));
        assert!(!is_excluded_path("runbooks/restart.md"));
        assert!(!is_excluded_path(".crystalline.yaml"));
    }

    #[test]
    fn the_walks_name_rule_agrees_with_the_path_rule() {
        assert!(!is_excluded_name("index.md"));
        assert!(is_excluded_name("log.md"));
        assert!(is_excluded_name(".gitignore"));
        assert!(!is_excluded_name("restart.md"));
        assert!(!is_excluded_name(".crystalline.yaml"));
    }

    #[test]
    fn generated_indexes_are_walked_like_any_other_file_and_logs_are_not() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "index.md", b"# Contents");
        write(dir.path(), "runbooks/index.md", b"# Contents");
        write(dir.path(), "runbooks/log.md", b"* something happened");
        write(dir.path(), "runbooks/restart.md", b"restart it");

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();
        let mut paths: Vec<&str> = result.changes.iter().map(|c| c.path()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec!["index.md", "runbooks/index.md", "runbooks/restart.md"]
        );
    }

    #[test]
    fn a_change_set_splits_into_real_work_and_index_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "index.md", b"# Contents");
        write(dir.path(), "runbooks/index.md", b"# Contents");
        write(dir.path(), "runbooks/restart.md", b"restart it");

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(result.changes.len(), 3);
        assert_eq!(result.substantive_count(), 1);
        assert_eq!(result.index_count(), 2);
    }

    #[test]
    fn an_index_only_change_set_counts_as_no_real_work() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "index.md", b"# Contents");

        let result = detect_local_changes(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(result.substantive_count(), 0);
        assert_eq!(result.index_count(), 1);
        assert!(result.changes[0].is_generated_index());
    }

    #[test]
    fn is_hidden_path_is_true_for_a_hidden_file_at_root() {
        assert!(is_hidden_path(".gitignore"));
    }

    #[test]
    fn is_hidden_path_is_true_for_a_file_under_a_hidden_directory() {
        assert!(is_hidden_path(".github/workflows/ci.yml"));
    }

    #[test]
    fn is_hidden_path_is_true_for_a_nested_hidden_directory() {
        assert!(is_hidden_path("notes/.trash/old.md"));
    }

    #[test]
    fn is_hidden_path_is_false_for_an_ordinary_nested_file() {
        assert!(!is_hidden_path("notes/a/b/deep.md"));
    }

    #[test]
    fn is_hidden_path_makes_an_exception_for_the_domain_config_file_at_root() {
        assert!(!is_hidden_path(".crystalline.yaml"));
    }

    #[test]
    fn is_hidden_path_makes_an_exception_for_the_domain_config_file_nested_under_a_visible_dir() {
        assert!(!is_hidden_path("notes/.crystalline.yaml"));
    }

    #[test]
    fn is_hidden_path_still_hides_the_domain_config_file_under_a_hidden_directory() {
        // The domain config file's own name is exempt, but a hidden ancestor
        // directory still hides it: the walk would have pruned `.config`
        // before ever reaching the file inside it.
        assert!(is_hidden_path(".config/.crystalline.yaml"));
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

    // --- case-only path differences ---
    //
    // The team repository these are named for held
    // `classes/Platform.Components.Common` and
    // `classes/platform.components.common` at once; a merged pull request
    // collapsed each pair into the properly-cased spelling as a rename with no
    // content change. Every base snapshot below plays the part of a member who
    // pulled before that landed, so it carries the old spelling while the disk
    // carries the new one. The base is in memory, and only ever one file lands
    // on disk, so these run identically on a case-insensitive filesystem and a
    // case-sensitive one.

    const BASE_SPELLING: &str = "classes/Platform.Components.Common/CustomHeaderModule.md";
    const DISK_SPELLING: &str = "classes/platform.components.common/CustomHeaderModule.md";

    #[test]
    fn a_case_only_path_difference_with_identical_content_is_not_a_change_at_all() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), DISK_SPELLING, b"header module");
        let mut base = BTreeMap::new();
        base.insert(BASE_SPELLING.to_string(), stamp_for(b"header module"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert!(
            result.changes.is_empty(),
            "a case-only difference is the same file: {:?}",
            result.changes
        );
    }

    #[test]
    fn a_case_only_path_difference_with_new_content_is_one_modified_at_the_base_spelling() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), DISK_SPELLING, b"header module, revised");
        let mut base = BTreeMap::new();
        base.insert(BASE_SPELLING.to_string(), stamp_for(b"header module"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert_eq!(
            result.changes,
            vec![LocalChange::Modified {
                path: BASE_SPELLING.to_string(),
                sha256: crate::state::sha256_hex(b"header module, revised"),
            }],
            "expected one Modified carrying the base spelling"
        );
        // The reported path is one the walk never saw, so on a case-sensitive
        // filesystem it opens nothing. Every reader has to go through
        // `disk_path`, and it has to lead to the file that is really there.
        assert_eq!(result.disk_path(BASE_SPELLING), DISK_SPELLING);
        assert!(
            dir.path().join(result.disk_path(BASE_SPELLING)).is_file(),
            "the path a share reads through must exist on this filesystem"
        );
        assert_eq!(result.disk_path("notes/untouched.md"), "notes/untouched.md");
    }

    #[test]
    fn an_adoption_that_reports_no_change_still_records_where_the_file_is() {
        // A replayed layer can hand a recorded path back long after the share
        // that made it, and that path is the base spelling. The map has to
        // cover adoptions that produced nothing, or that replay reads a path
        // that does not exist.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), DISK_SPELLING, b"header module");
        let mut base = BTreeMap::new();
        base.insert(BASE_SPELLING.to_string(), stamp_for(b"header module"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert!(result.changes.is_empty(), "{:?}", result.changes);
        assert_eq!(result.disk_path(BASE_SPELLING), DISK_SPELLING);
    }

    #[test]
    fn a_base_spelling_left_behind_by_a_checkout_is_not_offered_for_deletion() {
        // The colleague's repository as it stood BEFORE the pull request
        // collapsed the pair: the base carries both spellings, and a macOS
        // checkout could only ever hold one of them. The one it holds matches
        // byte-exactly; the other is not gone, it was never written. Offering
        // to delete it is offering to delete the file on screen.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), DISK_SPELLING, b"header module");
        let mut base = BTreeMap::new();
        base.insert(DISK_SPELLING.to_string(), stamp_for(b"header module"));
        base.insert(BASE_SPELLING.to_string(), stamp_for(b"header module"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert!(
            result.changes.is_empty(),
            "no deletion may be proposed for a spelling this checkout could not hold: {:?}",
            result.changes
        );
    }

    #[test]
    fn a_deletion_is_still_reported_when_no_spelling_of_it_is_on_disk() {
        // The suppression above must not swallow a real deletion: with neither
        // spelling on disk there is no file the pair could be standing for.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "classes/kept.md", b"kept");
        let mut base = BTreeMap::new();
        base.insert("classes/kept.md".to_string(), stamp_for(b"kept"));
        base.insert(DISK_SPELLING.to_string(), stamp_for(b"header module"));
        base.insert(BASE_SPELLING.to_string(), stamp_for(b"header module"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        let mut paths: Vec<&str> = result.changes.iter().map(|c| c.path()).collect();
        paths.sort_unstable();
        assert_eq!(paths, vec![BASE_SPELLING, DISK_SPELLING]);
        assert!(
            result
                .changes
                .iter()
                .all(|c| matches!(c, LocalChange::Deleted { .. })),
            "{:?}",
            result.changes
        );
    }

    #[test]
    fn a_genuine_deletion_beside_a_case_only_difference_reports_only_the_genuine_one() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), DISK_SPELLING, b"header module");
        let mut base = BTreeMap::new();
        base.insert(BASE_SPELLING.to_string(), stamp_for(b"header module"));
        base.insert("classes/retired.md".to_string(), stamp_for(b"retired"));

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert_eq!(
            result.changes,
            vec![LocalChange::Deleted {
                path: "classes/retired.md".to_string(),
            }]
        );
    }

    #[test]
    fn a_case_only_rename_landing_upstream_leaves_a_clean_local_status() {
        // The report itself: after the upstream rename landed, `status`
        // showed two deletions of files sitting on the member's disk, and
        // the share dialog offered to publish them.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "classes/platform.components.common/CustomHeaderModule.md",
            b"header module",
        );
        write(
            dir.path(),
            "classes/platform.components.utils/DateUtils.md",
            b"date utils",
        );
        let mut base = BTreeMap::new();
        base.insert(
            "classes/Platform.Components.Common/CustomHeaderModule.md".to_string(),
            stamp_for(b"header module"),
        );
        base.insert(
            "classes/Platform.Components.Utils/DateUtils.md".to_string(),
            stamp_for(b"date utils"),
        );

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert!(
            result.changes.is_empty(),
            "no deletion may be proposed for a file that is on disk: {:?}",
            result.changes
        );
    }

    #[test]
    fn two_base_keys_folding_together_are_ambiguous_and_nothing_is_folded() {
        // A repository genuinely holding both spellings. Guessing which one
        // the disk file stands for would be worse than reporting, so this
        // falls through to the byte-exact behaviour - and verify rule `E009`
        // is what tells the member to rename one of them.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), DISK_SPELLING, b"header module");
        let mut base = BTreeMap::new();
        base.insert(BASE_SPELLING.to_string(), stamp_for(b"header module"));
        base.insert(
            "classes/PLATFORM.COMPONENTS.COMMON/CustomHeaderModule.md".to_string(),
            stamp_for(b"header module"),
        );

        let result = detect_local_changes(dir.path(), &base).unwrap();
        let mut rules: Vec<&str> = result
            .changes
            .iter()
            .map(|c| match c {
                LocalChange::Added { .. } => "added",
                LocalChange::Modified { .. } => "modified",
                LocalChange::Deleted { .. } => "deleted",
            })
            .collect();
        rules.sort_unstable();
        assert_eq!(rules, vec!["added", "deleted", "deleted"]);
    }

    #[test]
    fn a_base_key_present_on_disk_exactly_is_never_adopted_by_another_spelling() {
        // Only reachable on a case-sensitive filesystem, where the two files
        // legitimately coexist, so the test builds the situation the walk
        // would see rather than requiring one: the exact match wins and the
        // other spelling is a genuine addition.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "notes/Alpha.md", b"alpha");
        let mut base = BTreeMap::new();
        base.insert("notes/Alpha.md".to_string(), stamp_for(b"alpha"));

        let sensitive = {
            let probe = dir.path().join("notes/alpha.md");
            std::fs::write(&probe, b"lowercase alpha").unwrap();
            std::fs::read(dir.path().join("notes/Alpha.md")).unwrap() == b"alpha"
        };
        if !sensitive {
            // A case-insensitive filesystem just overwrote `notes/Alpha.md`
            // rather than making a second file, so there is no pair here to
            // test with. Say so: a test that returns in silence reads exactly
            // like a test that checked something.
            eprintln!(
                "skipped: this filesystem is case-insensitive, so the two spellings cannot coexist"
            );
            return;
        }

        let result = detect_local_changes(dir.path(), &base).unwrap();
        assert_eq!(
            result.changes,
            vec![LocalChange::Added {
                path: "notes/alpha.md".to_string(),
                sha256: crate::state::sha256_hex(b"lowercase alpha"),
            }]
        );
    }
}
