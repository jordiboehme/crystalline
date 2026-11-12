//! Generating the OKF `index.md` files of a file domain.
//!
//! A file domain keeps one `index.md` per folder that holds knowledge, so the
//! domain can be navigated statically - in an editor, on a git forge, by any
//! OKF consumer - without Crystalline running. The bundle-root index file
//! declares the OKF version; every other one carries no frontmatter at all,
//! which is exactly what the spec asks of a directory index.
//!
//! The pass is a full-domain regeneration: walk the domain, render every
//! folder's index file, write only the ones whose content actually changed and
//! remove an `index.md` from a folder that no longer holds knowledge. That is
//! cheaper to reason about than per-folder dirty tracking (one move already
//! touches three folders plus their ancestors) and idempotent, so a
//! regeneration that changes nothing leaves every mtime alone and never wakes
//! the watcher.
//!
//! The reserved names themselves are never indexed (see the sync walk), so
//! writing these files creates no store rows, no embeddings and no search
//! hits.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crystalline_core::{
    DomainLayout, IndexEntry, PathClass, ReflectionMode, render_index, slash_rel,
};
use walkdir::WalkDir;

/// What one regeneration pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct IndexRefresh {
    /// Index files written because their content changed (or did not exist).
    pub written: usize,
    /// Index files removed because their folder no longer holds knowledge.
    pub removed: usize,
    /// Index files left untouched because their content was already correct.
    pub unchanged: usize,
    /// Concept files opened and parsed for their title and description.
    pub read: usize,
    /// Bytes those reads returned.
    pub bytes_read: u64,
}

/// Regenerate every `index.md` under `root`, the filesystem root of a file
/// domain whose Reflection is in `mode` (the mode in effect, so a pending
/// engram under `reflections/` is still listed and an effective Reflection's
/// folder is left out).
///
/// Blocking IO throughout: callers run it off the async runtime's worker
/// threads. Nothing here is fatal. A file that cannot be read still gets an
/// entry (titled by its filename stem), and a write or removal that fails is
/// logged and skipped, so a locked or read-only path never fails the write,
/// move, delete or sync that asked for the regeneration.
pub(crate) fn refresh(root: &Path, mode: ReflectionMode) -> IndexRefresh {
    let folders = collect(root, mode);
    let mut refresh = IndexRefresh {
        read: folders.read,
        bytes_read: folders.bytes_read,
        ..IndexRefresh::default()
    };

    // Every folder that holds knowledge directly or anywhere below it gets an
    // index file; the rest must not keep a stale one.
    let mut rendered: BTreeMap<String, String> = BTreeMap::new();
    for (folder, entries) in folders.wanted() {
        rendered.insert(folder.clone(), render_index(&entries, folder.is_empty()));
    }

    for (folder, content) in &rendered {
        let path = index_path(root, folder);
        let current = std::fs::read_to_string(&path).ok();
        if current.as_deref() == Some(content.as_str()) {
            refresh.unchanged += 1;
            continue;
        }
        match write_atomic(&path, content) {
            Ok(()) => refresh.written += 1,
            Err(e) => tracing::warn!("could not write {}: {e}", path.display()),
        }
    }

    for folder in &folders.existing_indexes {
        if rendered.contains_key(folder) {
            continue;
        }
        let path = index_path(root, folder);
        match std::fs::remove_file(&path) {
            Ok(()) => refresh.removed += 1,
            Err(e) => tracing::warn!("could not remove {}: {e}", path.display()),
        }
    }

    refresh
}

/// The domain as the walk found it: which folders hold concepts and where an
/// `index.md` already sits.
#[derive(Default)]
struct Walked {
    /// Folder path (domain-relative, forward-slashed, empty for the root) to
    /// its concept entries.
    concepts: BTreeMap<String, Vec<IndexEntry>>,
    /// Folders that already hold an `index.md`.
    existing_indexes: BTreeSet<String>,
    /// Concept files read for their title and description.
    read: usize,
    /// Bytes those reads returned.
    bytes_read: u64,
}

impl Walked {
    /// Folder to entries for every folder that gets an index file: its own
    /// concepts plus one entry per subdirectory that holds knowledge below it.
    /// A folder with neither is absent, so its stale index file is removed.
    fn wanted(&self) -> BTreeMap<String, Vec<IndexEntry>> {
        // A folder holds knowledge when it holds a concept itself or when one
        // of its descendants does, so each concept folder marks itself and
        // every ancestor up to the bundle root.
        let mut holding: BTreeSet<String> = BTreeSet::new();
        for (folder, entries) in &self.concepts {
            if entries.is_empty() {
                continue;
            }
            let mut current = Some(folder.clone());
            while let Some(f) = current {
                current = parent_of(&f);
                holding.insert(f);
            }
        }

        let mut wanted: BTreeMap<String, Vec<IndexEntry>> = BTreeMap::new();
        for folder in &holding {
            let mut entries = self.concepts.get(folder).cloned().unwrap_or_default();
            for child in &holding {
                if parent_of(child).as_deref() == Some(folder.as_str()) {
                    let name = child.rsplit('/').next().unwrap_or(child.as_str());
                    entries.push(IndexEntry::folder(name));
                }
            }
            if !entries.is_empty() {
                wanted.insert(folder.clone(), entries);
            }
        }
        wanted
    }
}

/// Walk the domain, reading each concept's title and description.
fn collect(root: &Path, mode: ReflectionMode) -> Walked {
    let mut walked = Walked::default();

    // The same layout the sync walk applies, so the index lists exactly the
    // files Crystalline indexes: no hidden file or folder, none of the
    // artifact folders the MANIFEST provisions from inside this root and,
    // while the Reflection is in effect, nothing under `reflections/`.
    let layout = DomainLayout::at(root, mode);
    let walker = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !layout.prunes_dir(&slash_rel(root, e.path())));

    for entry in walker.filter_map(Result::ok) {
        let Some(rel) = rel_path(root, entry.path()) else {
            continue;
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        match layout.classify(&rel) {
            PathClass::Reserved => {
                if name == crystalline_core::INDEX_FILE {
                    walked
                        .existing_indexes
                        .insert(parent_of(&rel).unwrap_or_default());
                }
                continue;
            }
            PathClass::Engram | PathClass::EngramAsset if name.to_lowercase().ends_with(".md") => {}
            PathClass::Engram
            | PathClass::EngramAsset
            | PathClass::Hidden
            | PathClass::Artifact
            | PathClass::Other
            | PathClass::Reflection(_) => continue,
        }
        let folder = parent_of(&rel).unwrap_or_default();
        let (title, description, bytes) = title_and_description(entry.path());
        walked.read += 1;
        walked.bytes_read += bytes;
        walked
            .concepts
            .entry(folder)
            .or_default()
            .push(IndexEntry::file(&name, &title, description.as_deref()));
    }

    // The walk leaves an effective Reflection's folder out, so the listings a
    // pending switch had written there are looked for on their own: each is
    // an existing index of a folder that holds no knowledge, so it goes.
    if mode == ReflectionMode::Effective {
        walked
            .existing_indexes
            .extend(stale_reflection_listings(root));
    }

    walked
}

/// The folders under the Reflection's folder (any letter case) that hold an
/// `index.md` this pass generated, from the time the switch was pending and
/// the folder held engrams. An `index.md` that does not read as generated is
/// somebody's own file and is not named. Hidden folders (the baseline) are
/// not entered.
fn stale_reflection_listings(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let off = DomainLayout::bare(ReflectionMode::Off);
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let is_folder = entry.file_type().is_ok_and(|t| t.is_dir())
            && entry.file_name().to_str().is_some_and(|n| {
                n.eq_ignore_ascii_case(crystalline_core::reflection::REFLECTIONS_DIR)
            });
        if !is_folder {
            continue;
        }
        let walk = WalkDir::new(entry.path())
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || off.enters_dir(&slash_rel(root, e.path())));
        for item in walk.filter_map(Result::ok) {
            if !item.file_type().is_file()
                || item.file_name() != std::ffi::OsStr::new(crystalline_core::INDEX_FILE)
            {
                continue;
            }
            let generated = std::fs::read_to_string(item.path())
                .is_ok_and(|text| crystalline_core::is_generated_index(&text));
            if generated && let Some(rel) = rel_path(root, item.path()) {
                found.push(parent_of(&rel).unwrap_or_default());
            }
        }
    }
    found
}

/// One concept's `title` and `description` frontmatter. A file that cannot be
/// read or parsed yields neither, so the entry falls back to its filename stem
/// and carries no description clause: an unreadable file still shows up in the
/// listing rather than silently vanishing from it. The third value is the
/// number of bytes read.
fn title_and_description(path: &Path) -> (String, Option<String>, u64) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (String::new(), None, 0);
    };
    let bytes = text.len() as u64;
    match crystalline_core::parse_engram(&text) {
        Ok(engram) => (
            engram.frontmatter.title.clone(),
            engram.frontmatter.description.clone(),
            bytes,
        ),
        Err(_) => (String::new(), None, bytes),
    }
}

/// The absolute path of a folder's index file.
fn index_path(root: &Path, folder: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for segment in folder.split('/').filter(|s| !s.is_empty()) {
        path.push(segment);
    }
    path.push(crystalline_core::INDEX_FILE);
    path
}

/// The domain-relative, forward-slashed path of `path` under `root`, or `None`
/// when it does not sit cleanly under it. The root itself maps to the empty
/// string, the key every folder map uses for the bundle root.
fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for component in rel.components() {
        let std::path::Component::Normal(segment) = component else {
            return None;
        };
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&segment.to_string_lossy());
    }
    Some(out)
}

/// The parent folder of a domain-relative path, or `None` for the root itself.
fn parent_of(rel: &str) -> Option<String> {
    if rel.is_empty() {
        return None;
    }
    match rel.rfind('/') {
        Some(i) => Some(rel[..i].to_string()),
        None => Some(String::new()),
    }
}

/// Write through a sibling temp file and rename, so a reader (or the watcher)
/// never sees a partially written index.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = crystalline_core::path::hidden_temp_path(path, "tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod perf;

#[cfg(test)]
mod tests {
    use super::*;

    fn engram(title: &str, description: Option<&str>) -> String {
        let mut out = format!("---\ntype: engram\ntitle: {title}\n");
        if let Some(d) = description {
            out.push_str(&format!("description: {d}\n"));
        }
        out.push_str("status: current\n---\n\n# Body\n");
        out
    }

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        std::fs::read_to_string(root.join(rel)).unwrap()
    }

    #[test]
    fn a_domain_gets_a_root_index_and_one_per_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "MANIFEST.md", &engram("Ops", Some("The ops domain")));
        write(root, "runbooks/restart.md", &engram("Restart", None));
        write(
            root,
            "runbooks/deep/rollback.md",
            &engram("Rollback", Some("How to roll back")),
        );

        let report = refresh(root, ReflectionMode::Off);
        assert_eq!(report.written, 3);
        assert_eq!(report.removed, 0);

        assert_eq!(
            read(root, "index.md"),
            "---\nokf_version: \"0.2\"\n---\n\n\
             # Contents\n\n\
             * [Ops](MANIFEST.md) - The ops domain\n\
             * [runbooks/](runbooks/)\n"
        );
        assert_eq!(
            read(root, "runbooks/index.md"),
            "# Contents\n\n* [deep/](deep/)\n* [Restart](restart.md)\n"
        );
        assert_eq!(
            read(root, "runbooks/deep/index.md"),
            "# Contents\n\n* [Rollback](rollback.md) - How to roll back\n"
        );
    }

    #[test]
    fn a_second_pass_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", &engram("A", None));
        write(root, "sub/b.md", &engram("B", None));

        assert_eq!(refresh(root, ReflectionMode::Off).written, 2);
        let second = refresh(root, ReflectionMode::Off);
        assert_eq!(
            second.written, 0,
            "an unchanged index must not be rewritten"
        );
        assert_eq!(second.unchanged, 2);
        assert_eq!(second.removed, 0);
    }

    #[test]
    fn an_emptied_folder_loses_its_index_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", &engram("A", None));
        write(root, "sub/b.md", &engram("B", None));
        refresh(root, ReflectionMode::Off);
        assert!(root.join("sub/index.md").is_file());

        std::fs::remove_file(root.join("sub/b.md")).unwrap();
        let report = refresh(root, ReflectionMode::Off);
        assert_eq!(report.removed, 1);
        assert!(!root.join("sub/index.md").exists());
        assert_eq!(
            read(root, "index.md"),
            "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [A](a.md)\n"
        );
    }

    /// The listings a pending switch left under `reflections/` go once the
    /// Reflection is in effect; an `index.md` somebody wrote there stays.
    #[test]
    fn an_effective_reflection_loses_the_listings_it_had_while_pending() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "notes/a.md", &engram("A", None));
        write(root, "reflections/guides/old.md", &engram("Old", None));
        refresh(root, ReflectionMode::Off);
        assert!(root.join("reflections/index.md").is_file());
        assert!(root.join("reflections/guides/index.md").is_file());
        std::fs::remove_file(root.join("reflections/guides/old.md")).unwrap();
        write(
            root,
            "reflections/own/index.md",
            "# Our pages\n\nRead these first.\n",
        );

        let report = refresh(root, ReflectionMode::Effective);
        assert_eq!(report.removed, 2);
        assert!(!root.join("reflections/index.md").exists());
        assert!(!root.join("reflections/guides/index.md").exists());
        assert!(root.join("reflections/own/index.md").is_file());
        assert_eq!(refresh(root, ReflectionMode::Effective).removed, 0);
    }

    #[test]
    fn a_folder_holding_only_subfolders_still_gets_an_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "top/mid/leaf.md", &engram("Leaf", None));

        refresh(root, ReflectionMode::Off);
        assert_eq!(
            read(root, "index.md"),
            "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [top/](top/)\n"
        );
        assert_eq!(read(root, "top/index.md"), "# Contents\n\n* [mid/](mid/)\n");
        assert_eq!(
            read(root, "top/mid/index.md"),
            "# Contents\n\n* [Leaf](leaf.md)\n"
        );
    }

    #[test]
    fn the_root_index_carries_frontmatter_only_at_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "sub/a.md", &engram("A", None));
        refresh(root, ReflectionMode::Off);
        assert!(read(root, "index.md").starts_with("---\nokf_version: \"0.2\"\n---\n"));
        assert!(!read(root, "sub/index.md").contains("okf_version"));
    }

    #[test]
    fn hidden_folders_and_reserved_files_are_never_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a.md", &engram("A", None));
        write(root, "log.md", "an OKF log file, no frontmatter\n");
        write(root, ".hidden/secret.md", &engram("Secret", None));
        write(root, "notes.txt", "not markdown\n");

        refresh(root, ReflectionMode::Off);
        let index = read(root, "index.md");
        assert_eq!(
            index,
            "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [A](a.md)\n"
        );
        assert!(!root.join(".hidden/index.md").exists());
    }

    #[test]
    fn a_file_without_a_title_falls_back_to_its_filename() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "orphan-note.md", "no frontmatter at all\n");
        refresh(root, ReflectionMode::Off);
        assert_eq!(
            read(root, "index.md"),
            "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [orphan-note](orphan-note.md)\n"
        );
    }

    #[test]
    fn an_empty_domain_gets_no_index_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let report = refresh(root, ReflectionMode::Off);
        assert_eq!(report.written, 0);
        assert!(!root.join("index.md").exists());
    }
}
