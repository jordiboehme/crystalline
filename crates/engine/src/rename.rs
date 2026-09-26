//! Renaming a domain on this machine. The rename runs as a series of steps,
//! each one idempotent so a rename a crash interrupted can be completed by
//! running every step again; this module holds the steps that are not a
//! method of the store they change.

use std::collections::HashSet;
use std::io;
use std::path::Path;

/// Move the per-domain state folder `<parent>/<old>` to `<parent>/<new>`, for
/// the `origins/` and `overlays/` folders a domain keeps under its name.
///
/// Idempotent: no `old` and an existing `new` is fine (moved already), and so
/// is neither (the domain never had such a folder). Both present is an error
/// naming the next step, since merging two folders of state is not something
/// to guess at. A missing `parent` is the same as an empty one.
///
/// Presence is read from the exact names `parent` lists, never from
/// [`Path::exists`], which answers yes for `notes` when only `Notes` is there
/// on a case-insensitive filesystem. A rename of one folder to a spelling the
/// filesystem folds onto that same folder (a change of case, or of Unicode
/// normalization on APFS; same device and inode) goes through a temporary
/// sibling `<old>.renaming`, so the spelling really changes and the second
/// half never sees its target as already present; a leftover
/// `<old>.renaming` from an interrupted run is carried on to `new`. A
/// DIFFERENT folder the filesystem folds onto `new` counts as present.
///
/// On Windows a folder that holds an open file (the overlay journal, or one
/// a watcher is reading) cannot be renamed; the caller pauses the domain
/// before this step so nothing holds one.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the rename verb that calls it is not wired yet")
)]
pub(crate) fn move_state_dir(parent: &Path, old: &str, new: &str) -> io::Result<()> {
    if old == new {
        return Ok(());
    }
    let present = entry_names(parent)?;
    let has = |name: &str| present.contains(name);
    let both = || {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "both {old} and {new} exist under {}; move the files by hand and rerun",
                parent.display()
            ),
        )
    };
    let temp = format!("{old}.renaming");
    // One folder under two spellings: a change of case, or any other spelling
    // the filesystem folds onto the same folder (APFS also ignores Unicode
    // normalization). It moves through the temporary name, and a replay that
    // finds only the temporary name finishes it.
    let same_folder = old.to_lowercase() == new.to_lowercase()
        || (!has(old) && has(&temp))
        || (has(old) && !has(new) && same_dir(&parent.join(old), &parent.join(new)));
    if same_folder {
        let staged = match (has(old), has(&temp)) {
            (true, true) => return Err(both_staged(parent, old, &temp)),
            (true, false) if has(new) => return Err(both()),
            (true, false) => {
                std::fs::rename(parent.join(old), parent.join(&temp))?;
                true
            }
            (false, staged) => staged,
        };
        if staged {
            if has(new) {
                return Err(both());
            }
            std::fs::rename(parent.join(&temp), parent.join(new))?;
        }
        return Ok(());
    }
    // A different folder whose name differs from `new` only in case is the
    // target itself to a case-insensitive filesystem, and renaming onto it
    // would replace it when empty. The exact listing misses it; asking the
    // filesystem does not. The same folder was handled above.
    let new_taken = has(new) || parent.join(new).symlink_metadata().is_ok();
    match (has(old), new_taken) {
        (true, true) => Err(both()),
        (true, false) => std::fs::rename(parent.join(old), parent.join(new)),
        (false, _) => Ok(()),
    }
}

/// Whether `a` and `b` name one and the same directory, as they do on a
/// case-insensitive filesystem for two spellings that differ only in case.
/// Device and inode on unix; elsewhere the canonical paths, which Windows
/// resolves to the name as stored on disk. Anything unreadable is not the
/// same.
fn same_dir(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (a.symlink_metadata(), b.symlink_metadata()) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
    }
}

/// The error for an interrupted case-only move that left both the original
/// and its temporary sibling behind.
fn both_staged(parent: &Path, old: &str, temp: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "both {old} and {temp} exist under {}; move the files by hand and rerun",
            parent.display()
        ),
    )
}

/// The exact names `parent` lists, empty when it does not exist.
fn entry_names(parent: &Path) -> io::Result<HashSet<String>> {
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => return Err(e),
    };
    let mut names = HashSet::new();
    for entry in entries {
        names.insert(entry?.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(parent: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_state_folder_moves_with_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let origins = dir.path().join("origins");
        std::fs::create_dir_all(origins.join("eng/sub")).unwrap();
        std::fs::write(origins.join("eng/state.json"), "{}").unwrap();
        std::fs::write(origins.join("eng/sub/more.txt"), "more").unwrap();

        move_state_dir(&origins, "eng", "platform").unwrap();
        assert_eq!(listing(&origins), vec!["platform".to_string()]);
        assert_eq!(
            std::fs::read_to_string(origins.join("platform/state.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            std::fs::read_to_string(origins.join("platform/sub/more.txt")).unwrap(),
            "more"
        );

        move_state_dir(&origins, "eng", "platform").expect("a second call is fine");
        assert_eq!(listing(&origins), vec!["platform".to_string()]);
    }

    #[test]
    fn neither_folder_nor_parent_is_nothing_to_move() {
        let dir = tempfile::tempdir().unwrap();
        move_state_dir(dir.path(), "eng", "platform").unwrap();
        assert!(listing(dir.path()).is_empty());
        move_state_dir(&dir.path().join("missing"), "eng", "platform").unwrap();
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn both_folders_present_is_an_error_naming_the_next_step() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("eng")).unwrap();
        std::fs::create_dir(dir.path().join("platform")).unwrap();
        let err = move_state_dir(dir.path(), "eng", "platform").unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("both eng and platform exist") && text.contains("by hand"),
            "{text}"
        );
        assert_eq!(
            listing(dir.path()),
            vec!["eng".to_string(), "platform".to_string()],
            "nothing moved"
        );
    }

    /// Whether the filesystem under `dir` folds case: a folder made as
    /// `Probe` answers to `probe`.
    fn folds_case(dir: &Path) -> bool {
        let probe = dir.join("Probe");
        std::fs::create_dir(&probe).unwrap();
        let folds = dir.join("probe").exists();
        std::fs::remove_dir(&probe).unwrap();
        folds
    }

    /// On a case-insensitive filesystem (the macOS and Windows default) a
    /// move onto `Platform` while `platform` exists is a move onto that
    /// folder, which a plain rename would silently replace when it is empty.
    /// It is refused like any other move onto an existing folder. On a
    /// case-sensitive filesystem `Platform` and `platform` are two folders,
    /// the case does not arise, and the test returns early after saying so
    /// on stderr.
    #[test]
    fn a_move_onto_a_folder_differing_only_in_case_is_refused_where_case_folds() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        if !folds_case(parent) {
            eprintln!("skipped: {} is case-sensitive", parent.display());
            return;
        }
        std::fs::create_dir(parent.join("eng")).unwrap();
        std::fs::write(parent.join("eng/file"), "kept").unwrap();
        std::fs::create_dir(parent.join("platform")).unwrap();

        let err = move_state_dir(parent, "eng", "Platform").unwrap_err();
        assert!(
            err.to_string().contains("both eng and Platform exist"),
            "{err}"
        );
        assert_eq!(
            listing(parent),
            vec!["eng".to_string(), "platform".to_string()],
            "nothing moved and the empty folder was not replaced"
        );
        assert_eq!(
            std::fs::read_to_string(parent.join("eng/file")).unwrap(),
            "kept"
        );
    }

    /// A genuine case-only rename of one folder where the filesystem folds
    /// case: the target spelling resolves to the source folder itself, which
    /// must not read as "both exist". The case really changes, and a second
    /// call is fine. Skips (early return, message on stderr) where case does
    /// not fold; `a_case_only_move_goes_through_a_temporary_name` covers the
    /// plain path everywhere.
    #[test]
    fn a_case_only_rename_of_one_folder_changes_its_case_where_case_folds() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        if !folds_case(parent) {
            eprintln!("skipped: {} is case-sensitive", parent.display());
            return;
        }
        std::fs::create_dir(parent.join("Eng")).unwrap();
        std::fs::write(parent.join("Eng/file"), "kept").unwrap();
        assert!(
            same_dir(&parent.join("Eng"), &parent.join("eng")),
            "the premise: `eng` resolves to the folder `Eng`"
        );

        move_state_dir(parent, "Eng", "eng").unwrap();
        assert_eq!(listing(parent), vec!["eng".to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.join("eng/file")).unwrap(),
            "kept"
        );
        move_state_dir(parent, "Eng", "eng").expect("a second call is fine");
        assert_eq!(listing(parent), vec!["eng".to_string()]);
    }

    /// A spelling the filesystem folds onto the same folder without it being
    /// a change of case (APFS ignores Unicode normalization: a composed and a
    /// decomposed `e` with an accent name one folder) moves like a case
    /// change, through the temporary name, including the replay from it.
    /// Skips (early return, message on stderr) where the two spellings are
    /// two folders.
    #[test]
    fn a_folded_spelling_of_one_folder_moves_through_the_temporary_name() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        let composed = "caf\u{e9}";
        let decomposed = "cafe\u{301}";
        std::fs::create_dir(parent.join(composed)).unwrap();
        if !same_dir(&parent.join(composed), &parent.join(decomposed)) {
            eprintln!(
                "skipped: {} keeps composed and decomposed names apart",
                parent.display()
            );
            return;
        }
        std::fs::write(parent.join(composed).join("file"), "kept").unwrap();

        move_state_dir(parent, composed, decomposed).unwrap();
        assert_eq!(listing(parent), vec![decomposed.to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.join(decomposed).join("file")).unwrap(),
            "kept"
        );

        // A replay after the first half: only the temporary name is left.
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        std::fs::create_dir(parent.join(format!("{composed}.renaming"))).unwrap();
        move_state_dir(parent, composed, decomposed).unwrap();
        assert_eq!(listing(parent), vec![decomposed.to_string()]);
    }

    /// On a case-insensitive filesystem `notes` reads as present while only
    /// `Notes` is there, so a direct check-then-rename would skip the move or
    /// refuse it. Runs on every OS: on a case-sensitive one it is the plain
    /// path through the temporary name.
    #[test]
    fn a_case_only_move_goes_through_a_temporary_name() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        std::fs::create_dir(parent.join("Notes")).unwrap();
        std::fs::write(parent.join("Notes/file"), "kept").unwrap();

        move_state_dir(parent, "Notes", "notes").unwrap();
        assert_eq!(listing(parent), vec!["notes".to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.join("notes/file")).unwrap(),
            "kept"
        );

        move_state_dir(parent, "Notes", "notes").expect("a second call is fine");
        assert_eq!(listing(parent), vec!["notes".to_string()]);
    }

    /// A case-only move interrupted between its two renames leaves only the
    /// temporary name; running it again finishes the move.
    #[test]
    fn an_interrupted_case_only_move_is_finished_from_the_temporary_name() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        std::fs::create_dir(parent.join("Notes.renaming")).unwrap();
        std::fs::write(parent.join("Notes.renaming/file"), "kept").unwrap();

        move_state_dir(parent, "Notes", "notes").unwrap();
        assert_eq!(listing(parent), vec!["notes".to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.join("notes/file")).unwrap(),
            "kept"
        );
    }
}
