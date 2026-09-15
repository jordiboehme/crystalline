//! The files overlay: everything an actor wrote in review mode that is not an
//! engram.
//!
//! A domain that reviews changes has one rule the whole mode rests on: the
//! folder on disk changes only by a pull. An engram write keeps that rule by
//! landing as an index row in the writer's own dimension, mirrored by
//! [`crate::overlay_journal`]. Everything else - today that is attachments, the
//! files a person adds beside their pages - has nowhere to be a row, so it
//! lands here instead of in the folder.
//!
//! The layout is one file per path, per actor:
//!
//! ```text
//! <state_dir>/overlays/<domain>/<actor>/files/<path>             a draft file
//! <state_dir>/overlays/<domain>/<actor>/files/<path>.tombstone   that actor's deletion of the reviewed file
//! ```
//!
//! `<path>` is a validated attachment path
//! ([`crystalline_core::validate_asset_path`]), so every file here stands under
//! `assets/` and carries an allowlisted extension.
//!
//! **Inside the journal's actor folder on purpose.** An actor's drafts and the
//! files they wrote drafting them are one person's private work with one
//! lifetime, so putting this tree under
//! [`crate::overlay_journal::actor_dir`] makes the removals agree by
//! construction: `journal_remove_domain`'s single `remove_dir_all` takes the
//! files with the drafts, and there is no second sweep for a later caller to
//! forget. The two trees cannot collide, and the reason is worth stating on
//! both sides. The journal's walk keeps only `.md` and `.md.tombstone`; an
//! attachment path can never end in `.md`, because `.md` is not on the
//! attachment extension allowlist. So no file here is ever read back as a
//! draft, and the same fact makes `<path>.tombstone` unspellable as a path:
//! `.tombstone` is not an allowlisted extension either.
//!
//! **The `testing`-feature refusal guards this root the same way.** Every
//! function here takes the `state_dir` the engine resolved through
//! [`crate::engine::Engine::journal_state_dir`] and never resolves one of its
//! own, so a fixture that forgot `Engine::with_state_dir` is refused before a
//! byte is written rather than reaching a developer's real state directory.
//!
//! **A draft file and a deletion are mutually exclusive on disk**, exactly as
//! they are in the journal, and for the same reason the order of each pair of
//! steps is what it is: [`put`] writes the bytes and then removes the sidecar,
//! [`tombstone`] removes the bytes and then writes the sidecar, so a crash
//! between two steps leaves the older of the two intentions rather than an
//! ambiguous state. [`held`] resolves a pair it still finds in favour of the
//! bytes, which is the journal's rule too.
//!
//! Containment is asserted here and not delegated: `domain` and `actor` are
//! screened with [`crate::overlay_journal::one_segment`] and `path` with
//! [`crystalline_core::validate_asset_path`], on the way in, at every entry
//! point.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::overlay_journal::{
    actor_dir, one_segment, prune_empty, remove_if_present, save, tombstone_of,
};

/// The folder an actor's non-engram files stand in, inside that actor's own
/// journal folder.
pub(crate) const FILES_DIR: &str = "files";

/// One entry an actor holds in a domain's files overlay: a file they wrote, or
/// their deletion of a reviewed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileEntry {
    /// The actor key whose entry this is.
    pub actor: String,
    /// The domain-relative attachment path, forward-slashed.
    pub path: String,
    /// Whether this is a deletion rather than bytes.
    pub tombstone: bool,
}

/// What one actor's files overlay holds, with the honesty flag beside it.
///
/// `unreadable` is here for the reason [`crate::overlay_journal::JournalRead`]
/// carries one: an empty answer means "this actor wrote nothing here" only when
/// nothing failed on the way to it, and a removal gate acts on the difference.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileRead {
    /// Every entry that could be enumerated, ordered by path.
    pub entries: Vec<FileEntry>,
    /// Whether anything the read tried to reach could not be reached.
    pub unreadable: bool,
}

/// What an actor holds at one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    /// Nothing: this path is whatever the folder says it is.
    Nothing,
    /// This actor's own bytes stand here.
    Bytes,
    /// This actor has deleted the reviewed file here.
    Tombstone,
}

/// Whose files overlay a write lands in.
///
/// Today it is always the writer's own actor, and this is one function rather
/// than a field read at each write site because exactly one thing will ever
/// change that answer: a **join**. When a caller's session is joined to another
/// actor's granted draft (the share-link grant Task 12 composes and Task 13
/// wires into a session), an upload or a delete performed inside that join
/// lands in the OWNER's files overlay rather than the joined caller's, and the
/// receipt says so in as many words. Everything else - a role, an instance
/// admin flag, a domain right - widens what a caller may reach and never whose
/// draft their writing is, which is the rule
/// [`crate::scope::overlay_actor`] already states for engram writes.
pub(crate) fn target_actor<'a>(writer: &'a str, joined: Option<&'a str>) -> &'a str {
    joined.unwrap_or(writer)
}

/// The folder one actor's files overlay stands in for one domain.
fn files_dir(state_dir: &Path, domain: &str, actor: &str) -> io::Result<PathBuf> {
    Ok(actor_dir(state_dir, domain, actor)?.join(FILES_DIR))
}

/// The file one actor's copy of `path` stands at, with the path re-checked
/// against the attachment rules on the way in.
///
/// The check is repeated here rather than trusted from a caller for the reason
/// the journal repeats its own: this is the last line before a path becomes a
/// place on disk, and one of the three names it is built from is chosen by
/// whoever holds an account.
///
/// The screen is the string rule alone, deliberately, which is where this
/// differs from `Engine::contained_asset_path`: that one canonicalizes as well,
/// because a domain folder is a place a person keeps their own files and can
/// hold a symlink somebody put there. This tree is written only by this module
/// and holds only what it wrote, so the string rule - which
/// [`crystalline_core::validate_asset_path`] makes strict (no `.`, `..`, hidden
/// segment, backslash or colon, and an allowlisted extension) - is the whole of
/// it. It is the same reading [`crate::overlay_journal`]'s entry path states
/// for its own paths.
pub(crate) fn file(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<PathBuf> {
    crystalline_core::validate_asset_path(path).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "the files overlay refuses the path '{}': {e}",
                path.escape_debug()
            ),
        )
    })?;
    let mut file = files_dir(state_dir, domain, actor)?;
    for seg in path.split('/') {
        file.push(seg);
    }
    Ok(file)
}

/// Write one actor's copy of `path`.
///
/// The bytes go down first and the sidecar beside them is removed second, so a
/// crash in between leaves both - which [`held`] resolves as the bytes, the
/// intention this call was carrying out.
pub(crate) fn put(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let abs = file(state_dir, domain, actor, path)?;
    save(&abs, bytes)?;
    remove_if_present(&tombstone_of(&abs))
}

/// Mark one actor's deletion of the reviewed file at `path`.
///
/// The bytes are removed first and the sidecar written second: a crash in
/// between leaves neither, which reads as this actor holding nothing at the
/// path - behind their intention by one write rather than ambiguous.
pub(crate) fn tombstone(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<()> {
    let abs = file(state_dir, domain, actor, path)?;
    remove_if_present(&abs)?;
    save(&tombstone_of(&abs), b"")
}

/// Drop whatever one actor holds at `path`, bytes or sidecar.
///
/// The folders it stood in are pruned while they are empty, up to and including
/// `files/` - and no further: the actor's own folder belongs to the journal and
/// outlives a cleared file.
pub(crate) fn clear(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<()> {
    let abs = file(state_dir, domain, actor, path)?;
    remove_if_present(&tombstone_of(&abs))?;
    remove_if_present(&abs)?;
    let stop = files_dir(state_dir, domain, actor)?;
    prune_empty(abs.parent(), &stop);
    Ok(())
}

/// One actor's bytes at `path`, or [`None`] when they hold no bytes there -
/// which includes holding a deletion, since a deletion is not bytes. A caller
/// that has to tell the two apart asks [`held`].
pub(crate) fn read(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
) -> io::Result<Option<Vec<u8>>> {
    let abs = file(state_dir, domain, actor, path)?;
    match std::fs::read(&abs) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// How many bytes one actor's copy of `path` holds, or [`None`] when they hold
/// no bytes there.
///
/// A stat, never a read: the size of a file is a question its metadata answers,
/// and reading the whole file to ask it would be both a wasted read and a
/// stricter answer than the act it previews - a file whose bytes this process
/// cannot read still has a size, and deleting it still takes that many bytes
/// away. The same reading `Engine::attachment_delete_size` already applies to
/// the folder.
pub(crate) fn size(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
) -> io::Result<Option<u64>> {
    let abs = file(state_dir, domain, actor, path)?;
    match std::fs::metadata(&abs) {
        Ok(meta) => Ok(Some(meta.len())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// What one actor holds at `path`. Bytes win over a sidecar, which is the
/// crash window the two writers above document resolved the same way
/// [`crate::overlay_journal`] resolves it.
pub(crate) fn held(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<Held> {
    let abs = file(state_dir, domain, actor, path)?;
    if abs.is_file() {
        return Ok(Held::Bytes);
    }
    if tombstone_of(&abs).is_file() {
        return Ok(Held::Tombstone);
    }
    Ok(Held::Nothing)
}

/// Every entry one actor holds in one domain, ordered by path, and whether
/// anything could not be enumerated.
///
/// No bytes are read: what a listing needs is which paths this actor has
/// touched and which way.
pub(crate) fn entries(state_dir: &Path, domain: &str, actor: &str) -> FileRead {
    let Ok(dir) = files_dir(state_dir, domain, actor) else {
        // A name that cannot address a folder is not a folder that is empty:
        // nothing was read, and a caller about to delete may not be told
        // otherwise.
        return FileRead {
            entries: Vec::new(),
            unreadable: true,
        };
    };
    let mut entries = Vec::new();
    let unreadable = collect(&dir, "", actor, &mut entries);
    entries.sort_by(|a: &FileEntry, b: &FileEntry| a.path.cmp(&b.path));
    FileRead {
        entries,
        unreadable,
    }
}

/// How many entries each actor holds in one domain's files overlay, files and
/// sidecars alike, with the honesty flag beside them.
///
/// What one domain's whole files overlay holds, per actor, with the honesty
/// flag for the domain's own folder beside it.
///
/// The listing every lifecycle caller reads: the removal gate counts it, the
/// fold plan names it, the fold folds it and the sweep ends it. Note what it is
/// NOT twinned with: [`crate::overlay_journal::journal_counts`] counts drafts
/// only, so a domain's journal count under-reports what its removal actually
/// sweeps by exactly what this answers.
pub(crate) struct DomainFiles {
    /// Every actor who has a files overlay folder in this domain, and what it
    /// holds - **including an actor whose folder could not be enumerated**,
    /// whose [`FileRead`] is empty and flagged. Leaving them out is what made a
    /// plan report "nothing to decide" over somebody's only copy of their work.
    pub(crate) per_actor: BTreeMap<String, FileRead>,
    /// Whether the domain's own overlay folder could be enumerated. `false`
    /// with an empty map is a domain nobody has drafted in.
    pub(crate) unreadable: bool,
}

impl DomainFiles {
    /// How many entries each actor holds, for the callers that only ever needed
    /// a number. An actor whose folder could not be read counts zero here and
    /// is caught by [`DomainFiles::unlistable`] instead.
    pub(crate) fn counts(&self) -> BTreeMap<String, u64> {
        self.per_actor
            .iter()
            .filter(|(_, read)| !read.entries.is_empty())
            .map(|(actor, read)| (actor.clone(), read.entries.len() as u64))
            .collect()
    }

    /// The first actor whose files could not be listed, or [`None`] when every
    /// one of them could.
    ///
    /// The domain's own folder failing is reported as an actor of `None`
    /// alongside: `Some(None)` is "this domain's overlay folder could not be
    /// read", `Some(Some(actor))` is "that actor's could not".
    pub(crate) fn unlistable(&self) -> Option<Option<&str>> {
        if self.unreadable {
            return Some(None);
        }
        self.per_actor
            .iter()
            .find(|(_, read)| read.unreadable)
            .map(|(actor, _)| Some(actor.as_str()))
    }
}

/// Read one domain's whole files overlay: one walk, one answer.
///
/// One walk is the point rather than an optimization: the pass that counted and
/// the pass that listed used to walk every actor's tree twice and could
/// disagree about what was there in between.
pub(crate) fn by_actor(state_dir: &Path, domain: &str) -> DomainFiles {
    let mut per_actor: BTreeMap<String, FileRead> = BTreeMap::new();
    let Ok(dir) = crate::overlay_journal::domain_dir(state_dir, domain) else {
        return DomainFiles {
            per_actor,
            unreadable: true,
        };
    };
    let actors = match std::fs::read_dir(&dir) {
        Ok(actors) => actors,
        // A domain nobody has drafted in has no folder, and that is a certain
        // answer rather than an unknown one.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return DomainFiles {
                per_actor,
                unreadable: false,
            };
        }
        Err(e) => {
            tracing::warn!(
                domain = domain,
                "the files overlay folder could not be read: {e}"
            );
            return DomainFiles {
                per_actor,
                unreadable: true,
            };
        }
    };
    for actor in actors.flatten() {
        if !actor.path().is_dir() {
            continue;
        }
        let Some(name) = actor.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if one_segment(&name, "actor").is_err() {
            continue;
        }
        // An actor with no files folder at all takes no part in this; one whose
        // folder exists is listed whatever is in it, empty and unreadable
        // alike. An empty folder listed is what lets the sweep end it, and an
        // unreadable one listed is what lets the fold refuse by name.
        //
        // **Asked with an explicit match, never with `exists`.**
        // [`std::path::Path::exists`] is `metadata().is_ok()`, which answers
        // `false` both for "not there" and for "cannot be determined" - so an
        // actor whose own folder cannot be traversed used to answer "no files
        // folder" and be dropped here. An actor who is not in the listing is
        // one the fold's refusal cannot name and no sweep ever reaches, which
        // is the silent omission this listing exists to prevent, one directory
        // further up. `symlink_metadata` rather than `metadata` so a dangling
        // symlink is a thing that is there rather than a thing that is not.
        let Ok(files) = files_dir(state_dir, domain, &name) else {
            continue;
        };
        match std::fs::symlink_metadata(&files) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!(
                    domain = domain,
                    actor = name.as_str(),
                    "the files folder of '{name}' could not be asked about: {e}"
                );
                per_actor.insert(
                    name.clone(),
                    FileRead {
                        entries: Vec::new(),
                        unreadable: true,
                    },
                );
            }
            Ok(_) => {
                per_actor.insert(name.clone(), entries(state_dir, domain, &name));
            }
        }
    }
    DomainFiles {
        per_actor,
        unreadable: false,
    }
}

/// Drop one actor's whole files overlay, answering how many entries it held.
///
/// The counterpart of [`crate::overlay_journal::journal_remove_domain`] for one
/// actor: the discard half of leaving review mode takes this folder with that
/// actor's rows. A whole domain's files go with its journal folder already, by
/// construction - this tree stands inside it, which is why there is a per-actor
/// sweep here and no per-domain one.
pub(crate) fn remove_actor(state_dir: &Path, domain: &str, actor: &str) -> io::Result<u64> {
    let dir = files_dir(state_dir, domain, actor)?;
    let held = entries(state_dir, domain, actor).entries.len() as u64;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(held),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

/// Walk one actor's files folder, appending every entry under it. Answers
/// whether anything under it could not be read.
///
/// A path that is not a legal attachment path is skipped in silence - the
/// atomic write's temporary sibling among them - exactly as the journal skips
/// what is not a draft.
fn collect(dir: &Path, prefix: &str, actor: &str, out: &mut Vec<FileEntry>) -> bool {
    let listed = match std::fs::read_dir(dir) {
        Ok(listed) => listed,
        // An actor who has written no files has no folder: certain, and empty.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
        Err(e) => {
            tracing::warn!(
                actor = actor,
                "a files overlay folder could not be read: {e}"
            );
            return true;
        }
    };
    let mut unreadable = false;
    for entry in listed {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(actor = actor, "a files overlay entry was skipped: {e}");
                unreadable = true;
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if entry.path().is_dir() {
            unreadable |= collect(&entry.path(), &rel, actor, out);
            continue;
        }
        let (rel, tombstone) = match rel.strip_suffix(crate::overlay_journal::TOMBSTONE_SUFFIX) {
            Some(base) => (base.to_string(), true),
            None => (rel, false),
        };
        if crystalline_core::validate_asset_path(&rel).is_err() {
            continue;
        }
        out.push(FileEntry {
            actor: actor.to_string(),
            path: rel,
            tombstone,
        });
    }
    unreadable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00binary\x00bytes";

    /// The layout, on disk, and the two forms never coexisting at one path.
    #[test]
    fn a_file_lands_under_the_actors_files_folder_and_a_deletion_is_a_sidecar() {
        let tmp = dir();
        let state = tmp.path();

        put(state, "team", "alice", "assets/deck.png", PNG).unwrap();
        assert!(
            state
                .join("overlays/team/alice/files/assets/deck.png")
                .is_file(),
            "bytes stand at <state_dir>/overlays/<domain>/<actor>/files/<path>"
        );
        assert_eq!(
            read(state, "team", "alice", "assets/deck.png")
                .unwrap()
                .as_deref(),
            Some(PNG)
        );
        assert_eq!(
            held(state, "team", "alice", "assets/deck.png").unwrap(),
            Held::Bytes
        );

        tombstone(state, "team", "alice", "assets/deck.png").unwrap();
        assert!(
            state
                .join("overlays/team/alice/files/assets/deck.png.tombstone")
                .is_file(),
            "a deletion is a zero-byte sidecar beside where the bytes would stand"
        );
        assert!(
            !state
                .join("overlays/team/alice/files/assets/deck.png")
                .exists()
        );
        assert_eq!(
            held(state, "team", "alice", "assets/deck.png").unwrap(),
            Held::Tombstone
        );
        assert!(
            read(state, "team", "alice", "assets/deck.png")
                .unwrap()
                .is_none()
        );

        put(state, "team", "alice", "assets/deck.png", PNG).unwrap();
        assert!(
            !state
                .join("overlays/team/alice/files/assets/deck.png.tombstone")
                .exists(),
            "a write takes the sidecar away"
        );
        assert_eq!(
            held(state, "team", "alice", "assets/deck.png").unwrap(),
            Held::Bytes
        );

        // The size is the file's own length, answered off its metadata, and
        // absent wherever there are no bytes to measure.
        assert_eq!(
            size(state, "team", "alice", "assets/deck.png").unwrap(),
            Some(PNG.len() as u64)
        );
        assert_eq!(
            size(state, "team", "alice", "assets/never.png").unwrap(),
            None
        );
        tombstone(state, "team", "alice", "assets/deck.png").unwrap();
        assert_eq!(
            size(state, "team", "alice", "assets/deck.png").unwrap(),
            None,
            "a deletion holds no bytes, so it has no size"
        );
    }

    /// Every name is screened, and the path additionally has to be an
    /// attachment path - which is what keeps an `.md` out of this tree.
    #[test]
    fn a_files_overlay_refuses_every_path_the_journal_refuses_and_every_non_attachment_path() {
        let tmp = dir();
        let state = tmp.path();

        for (domain, actor, path) in [
            ("../escape", "alice", "assets/deck.png"),
            ("team/nested", "alice", "assets/deck.png"),
            ("team", "..", "assets/deck.png"),
            ("team", "a/b", "assets/deck.png"),
            ("C:", "alice", "assets/deck.png"),
            ("team", "C:", "assets/deck.png"),
            ("team", "a:b", "assets/deck.png"),
            ("team", "alice", "../escape.png"),
            ("team", "alice", "assets/../../escape.png"),
            ("team", "alice", "/assets/absolute.png"),
            ("team", "alice", ""),
            // The invariant the sidecar rests on: `.tombstone` is not an
            // allowlisted extension, so the spelling can never collide.
            ("team", "alice", "assets/deck.png.tombstone"),
            // Not under `assets/`, so not a file this overlay holds.
            ("team", "alice", "notes/plan.md"),
            // An extension the allowlist does not carry.
            ("team", "alice", "assets/x.exe"),
            // The one that keeps the journal walk honest: an engram suffix is
            // never an attachment path, so no `.md` can stand in this tree.
            ("team", "alice", "assets/x.md"),
        ] {
            assert!(
                put(state, domain, actor, path, PNG).is_err(),
                "put took ({domain}, {actor}, {path})"
            );
            assert!(tombstone(state, domain, actor, path).is_err());
            assert!(clear(state, domain, actor, path).is_err());
            assert!(read(state, domain, actor, path).is_err());
            assert!(held(state, domain, actor, path).is_err());
        }
        assert!(
            !tmp.path().join("escape.png").exists()
                && !tmp.path().parent().unwrap().join("escape.png").exists(),
            "nothing was written outside the files overlay"
        );
        assert!(remove_actor(state, "../escape", "alice").is_err());
    }

    /// The folders a cleared file stood in go with it, up to `files/` and no
    /// further: the actor's own folder is the journal's and outlives this.
    #[test]
    fn clearing_the_last_file_prunes_up_to_the_files_folder_and_no_further() {
        let tmp = dir();
        let state = tmp.path();

        crate::overlay_journal::journal_write(
            state,
            "team",
            "alice",
            "plan.md",
            "---\ntype: engram\ntitle: Plan\npermalink: plan\nstatus: draft\n---\n\n# Plan\n",
        )
        .unwrap();
        put(state, "team", "alice", "assets/deep/deck.png", PNG).unwrap();
        clear(state, "team", "alice", "assets/deep/deck.png").unwrap();

        assert!(
            !state.join("overlays/team/alice/files").exists(),
            "the files folder goes with the last file under it"
        );
        assert!(
            state.join("overlays/team/alice/plan.md").is_file(),
            "and the actor's own folder, which holds their journal drafts, stays"
        );
    }

    /// **A folder that cannot be asked about at all is flagged, never skipped.**
    ///
    /// "Not there" and "cannot be determined" are one answer from
    /// [`std::path::Path::exists`], which is `metadata().is_ok()`. An actor
    /// whose own folder cannot be traversed therefore answered "no files
    /// folder" and was dropped from the listing - and an actor who is not in
    /// the listing is an actor the fold's refusal cannot name, whose files no
    /// sweep ever ends, and whose bytes are left in a domain that reviews
    /// nothing. That is the same silent omission this whole listing exists to
    /// prevent, one directory further up.
    ///
    /// Unix only - the permission bits are the discriminator, and Windows has
    /// no equivalent that leaves the parent listable. Skipped in the one
    /// environment where the bits do not bind (a run as root), with a note
    /// rather than a silent pass.
    #[cfg(unix)]
    #[test]
    fn an_actor_whose_files_folder_cannot_be_stated_is_flagged_rather_than_skipped() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = dir();
        let state = tmp.path();
        put(state, "team", "alice", "assets/deck.png", PNG).unwrap();
        put(state, "team", "bob", "assets/his.png", PNG).unwrap();

        // Bob's own folder is untraversable, so a stat of the `files` folder
        // inside it cannot be answered either way. His folder itself is still
        // listed by the domain walk, because that needs traverse on the domain
        // folder rather than on his.
        let bob = state.join("overlays/team/bob");
        std::fs::set_permissions(&bob, std::fs::Permissions::from_mode(0o000)).unwrap();
        if bob.join("files").exists() {
            std::fs::set_permissions(&bob, std::fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!(
                "skipped: this process stats through a mode-000 directory, so the permission \
                 bits cannot discriminate here (a run as root)"
            );
            return;
        }

        let held = by_actor(state, "team");
        assert_eq!(
            held.unlistable(),
            Some(Some("bob")),
            "the actor nothing could be learned about is named"
        );
        assert!(
            held.per_actor.contains_key("bob"),
            "and he is in the listing, or no refusal could name him and no sweep \
             would ever reach him"
        );
        assert_eq!(
            held.counts().get("alice"),
            Some(&1),
            "while the actor who could be read is read"
        );

        // Left as we found it, so the tempdir can be removed.
        std::fs::set_permissions(&bob, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// An answer that could not see everything says so, and a sidecar counts
    /// as an entry exactly as a file does.
    #[test]
    fn counts_and_entries_carry_the_unreadable_flag() {
        let tmp = dir();
        let state = tmp.path();

        put(state, "team", "alice", "assets/a.png", PNG).unwrap();
        tombstone(state, "team", "alice", "assets/b.png").unwrap();
        put(state, "team", "bob", "assets/c.png", PNG).unwrap();

        let read = entries(state, "team", "alice");
        assert_eq!(
            read.entries,
            vec![
                FileEntry {
                    actor: "alice".to_string(),
                    path: "assets/a.png".to_string(),
                    tombstone: false,
                },
                FileEntry {
                    actor: "alice".to_string(),
                    path: "assets/b.png".to_string(),
                    tombstone: true,
                },
            ],
            "one actor's own entries, ordered by path, a sidecar among them"
        );
        assert!(!read.unreadable);

        let held = by_actor(state, "team");
        assert_eq!(held.counts().get("alice"), Some(&2));
        assert_eq!(held.counts().get("bob"), Some(&1));
        assert!(!held.unreadable, "nothing failed to enumerate");
        assert!(held.unlistable().is_none(), "and every actor answered");

        // A files folder that is not a folder: nothing can be enumerated, and
        // an empty answer must not read as `this actor holds nothing`.
        std::fs::remove_dir_all(state.join("overlays/team/bob/files")).unwrap();
        std::fs::write(state.join("overlays/team/bob/files"), "not a folder").unwrap();
        let read = entries(state, "team", "bob");
        assert!(read.entries.is_empty());
        assert!(read.unreadable, "an unreadable overlay is not an empty one");
        let held = by_actor(state, "team");
        assert_eq!(
            held.unlistable(),
            Some(Some("bob")),
            "the actor whose files could not be listed is named, not dropped"
        );
        assert!(
            held.per_actor.contains_key("bob"),
            "and he is still in the listing, or a plan would report nothing to \
             decide over work nobody can see"
        );
        assert_eq!(held.counts().get("bob"), None, "with no count to give");

        // An actor who has written nothing is a different answer: empty and
        // certain.
        let read = entries(state, "team", "never");
        assert!(read.entries.is_empty() && !read.unreadable);

        assert_eq!(remove_actor(state, "team", "alice").unwrap(), 2);
        assert!(!state.join("overlays/team/alice/files").exists());
        assert_eq!(
            remove_actor(state, "team", "alice").unwrap(),
            0,
            "an actor with no files overlay sweeps to nothing"
        );
    }

    /// The join seam: today it answers the writer's own actor, and the doc on
    /// [`target_actor`] names the one thing that will change that.
    #[test]
    fn the_target_actor_is_the_writers_own_when_no_join_is_given() {
        assert_eq!(target_actor("alice", None), "alice");
        assert_eq!(target_actor("alice", Some("bob")), "bob");
    }
}
