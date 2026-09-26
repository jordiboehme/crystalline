//! Renaming a domain on this machine. The rename runs as a series of steps,
//! each one idempotent so a rename a crash interrupted can be completed by
//! running every step again; this module holds the journal that records
//! which steps are done, the pause that keeps everything else off the domain
//! while it moves, and the steps that are not a method of the store they
//! change.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crystalline_core::config::NameOrigin;
use serde::{Deserialize, Serialize};

/// The file the rename journal lives in, directly under the state directory.
pub(crate) const JOURNAL_FILE: &str = "rename-journal.json";

/// How long a write, a sync or a rename waits on a domain another rename has
/// paused, or on the writes a rename is waiting to see finish.
pub(crate) const RENAME_WAIT: Duration = Duration::from_secs(30);

/// One step of a domain rename, in the order they run. The journal records
/// each one as it completes; a rename completed after a crash runs every step
/// the journal does not list, and every step is safe to run twice.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RenameStep {
    /// Write the new name into the domain's MANIFEST (a full rename only).
    Manifest,
    /// Respell links to the domain in every domain this machine can write (a
    /// full rename only).
    Relink,
    /// Rename the index's domain row in place, keeping its id.
    IndexRow,
    /// Move the visibility, membership and share-link records.
    AuthTables,
    /// Move the `origins/<name>/` state folder.
    OriginsDir,
    /// Move the `overlays/<name>/` draft journal folder.
    OverlaysDir,
    /// Move the provision receipt's entries.
    ProvisionReceipt,
    /// Write the configuration: the new key, the old spellings as aliases.
    Config,
}

impl RenameStep {
    /// The steps of a rename of this machine's name only.
    pub(crate) const LOCAL: [RenameStep; 6] = [
        RenameStep::IndexRow,
        RenameStep::AuthTables,
        RenameStep::OriginsDir,
        RenameStep::OverlaysDir,
        RenameStep::ProvisionReceipt,
        RenameStep::Config,
    ];

    /// The steps of a rename that also writes the MANIFEST and respells
    /// links: those two first, then the local ones.
    pub(crate) const FULL: [RenameStep; 8] = [
        RenameStep::Manifest,
        RenameStep::Relink,
        RenameStep::IndexRow,
        RenameStep::AuthTables,
        RenameStep::OriginsDir,
        RenameStep::OverlaysDir,
        RenameStep::ProvisionReceipt,
        RenameStep::Config,
    ];

    /// The step's name as a report and the journal spell it.
    pub fn name(self) -> &'static str {
        match self {
            RenameStep::Manifest => "manifest",
            RenameStep::Relink => "relink",
            RenameStep::IndexRow => "index_row",
            RenameStep::AuthTables => "auth_tables",
            RenameStep::OriginsDir => "origins_dir",
            RenameStep::OverlaysDir => "overlays_dir",
            RenameStep::ProvisionReceipt => "provision_receipt",
            RenameStep::Config => "config",
        }
    }
}

/// A rename in progress, written before its first step and after every step,
/// and deleted once the configuration carries the new name. A journal found
/// at startup is a rename a crash stopped, and it is finished before the
/// daemon serves anything.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RenameJournal {
    /// The journal format, 1.
    pub version: u32,
    /// The local name the domain had.
    pub old: String,
    /// The local name it gets.
    pub new: String,
    /// Whether the rename leaves the MANIFEST and content alone.
    pub local_only: bool,
    /// What the configuration records for `new`.
    pub origin: NameOrigin,
    /// Every spelling the domain had before step one (local name, canonical
    /// name, aliases), minus `new`. Captured once, before any step: after the
    /// `Manifest` step the name table no longer knows the previous canonical,
    /// so Relink, Config and recovery read the spellings from here, never
    /// from the table.
    pub old_spellings: Vec<String>,
    /// The old spellings that reached this domain through the name table
    /// before step one: the ones the relink step respells. A declared name
    /// another domain's local name shadows, a contested one and an alias the
    /// table dropped name some other domain or none, and links spelled with
    /// them are not this domain's to rewrite.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relink_spellings: Vec<String>,
    /// The name the domain's MANIFEST declares once the rename is done, if
    /// any: what the Config step records as the canonical name last seen. The
    /// name it declared before step one for a rename of this machine's name
    /// only, and `new` for a full rename whose MANIFEST write lands in the
    /// folder or the database.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    /// Who asked for a full rename, `None` for the machine owner. The
    /// MANIFEST and relink steps write as this caller, so a review-mode
    /// domain takes the edit into this caller's draft, also when the rename
    /// is finished after a crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<RenameCaller>,
    /// The domains the caller could write when the rename started, sorted:
    /// the relink step respells links in these and lists the ones it finds
    /// anywhere else. Kept here because a daemon finishing the rename at
    /// startup has no way to ask what the caller may write.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writable: Vec<String>,
    /// Whether the MANIFEST step writes into the caller's draft (the domain
    /// reviews changes) rather than into the folder or the database.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manifest_draft: bool,
    /// The steps completed so far, in order.
    pub done: Vec<RenameStep>,
}

/// The signed-in account a full rename acts for, as the journal keeps it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RenameCaller {
    /// The login name.
    pub account: String,
    /// Whether that account held the instance admin role.
    #[serde(default)]
    pub admin: bool,
}

impl RenameJournal {
    /// Where the journal lives under `state_dir`.
    pub(crate) fn path(state_dir: &Path) -> PathBuf {
        state_dir.join(JOURNAL_FILE)
    }

    /// The journal under `state_dir`, or `None` when no rename is pending.
    /// A file that does not parse is an error naming it: guessing at a half
    /// written rename is not something to do silently.
    pub(crate) fn load(state_dir: &Path) -> io::Result<Option<RenameJournal>> {
        let path = RenameJournal::path(state_dir);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the rename journal {} could not be read ({e}); finish or undo that rename \
                     by hand, then delete the file",
                    path.display()
                ),
            )
        })
    }

    /// Write the journal atomically: a temporary file beside it, then a
    /// rename over it, so a crash leaves the previous journal or this one and
    /// never half of either.
    pub(crate) fn save(&self, state_dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(state_dir)?;
        let path = RenameJournal::path(state_dir);
        let temp = state_dir.join(format!("{JOURNAL_FILE}.tmp"));
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temp, &path)
    }

    /// Delete the journal; a missing one is fine.
    pub(crate) fn remove(state_dir: &Path) -> io::Result<()> {
        match std::fs::remove_file(RenameJournal::path(state_dir)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The steps this rename runs, in order.
    pub(crate) fn steps(&self) -> &'static [RenameStep] {
        if self.local_only {
            &RenameStep::LOCAL
        } else {
            &RenameStep::FULL
        }
    }
}

/// The domains a rename has paused, and the writes still running in each.
///
/// A write, a sync or a watcher pass into a paused domain waits (up to
/// [`RENAME_WAIT`]) or is skipped; a rename, once it has paused a domain,
/// waits for the writes already running in it to finish before its first
/// step, so nothing writes under the old name after the index row has moved.
#[derive(Default)]
pub(crate) struct RenamePause {
    paused: std::sync::RwLock<HashSet<String>>,
    writers: std::sync::Mutex<HashMap<String, usize>>,
    changed: tokio::sync::Notify,
}

impl RenamePause {
    /// Whether `name` is paused.
    pub(crate) fn is_paused(&self, name: &str) -> bool {
        self.paused.read().unwrap().contains(name)
    }

    /// Whether any domain is paused.
    pub(crate) fn any(&self) -> bool {
        !self.paused.read().unwrap().is_empty()
    }

    /// Every paused name.
    pub(crate) fn names(&self) -> Vec<String> {
        self.paused.read().unwrap().iter().cloned().collect()
    }

    /// Pause `names`.
    pub(crate) fn pause(&self, names: &[&str]) {
        let mut paused = self.paused.write().unwrap();
        paused.extend(names.iter().map(|n| n.to_string()));
    }

    /// Lift the pause on `names` and wake everything waiting on it.
    pub(crate) fn resume(&self, names: &[&str]) {
        {
            let mut paused = self.paused.write().unwrap();
            for name in names {
                paused.remove(*name);
            }
        }
        self.changed.notify_waiters();
    }

    /// Wait until none of `names` is paused. Answers whether it had to wait
    /// at all, or the name still paused when `limit` ran out.
    pub(crate) async fn wait_clear(
        &self,
        names: &[String],
        limit: Duration,
    ) -> Result<bool, String> {
        let deadline = tokio::time::Instant::now() + limit;
        let mut waited = false;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let busy = {
                let paused = self.paused.read().unwrap();
                names.iter().find(|n| paused.contains(n.as_str())).cloned()
            };
            let Some(busy) = busy else {
                return Ok(waited);
            };
            waited = true;
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(busy);
            }
        }
    }

    /// Count a write into `name`, unless `name` is paused.
    pub(crate) fn try_enter(&self, name: &str) -> Option<WriteTicket<'_>> {
        *self
            .writers
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_default() += 1;
        let ticket = WriteTicket {
            pause: self,
            name: name.to_string(),
        };
        // Counted before the check, so a rename that paused the domain in
        // between either sees this write in its count or this write sees the
        // pause; never neither.
        if self.is_paused(name) {
            return None;
        }
        Some(ticket)
    }

    /// Wait until no write counted into `name` is still running. False when
    /// `limit` ran out first.
    pub(crate) async fn drained(&self, name: &str, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.writers.lock().unwrap().contains_key(name) {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return false;
            }
        }
    }
}

/// One running write into a domain, counted until it is dropped.
pub(crate) struct WriteTicket<'a> {
    pause: &'a RenamePause,
    name: String,
}

impl Drop for WriteTicket<'_> {
    fn drop(&mut self) {
        {
            let mut writers = self.pause.writers.lock().unwrap();
            if let Some(count) = writers.get_mut(&self.name) {
                *count -= 1;
                if *count == 0 {
                    writers.remove(&self.name);
                }
            }
        }
        self.pause.changed.notify_waiters();
    }
}

/// A test seam: a rename armed with one stops after its step, says so, and
/// goes on only when released. See `Engine::hold_rename_after`.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct RenameHold {
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg(any(test, feature = "testing"))]
impl RenameHold {
    /// Resolves once the rename has finished the step it was held after.
    pub async fn reached(&self) {
        self.reached.notified().await;
    }

    /// Let the held rename go on.
    pub fn release(&self) {
        self.release.notify_one();
    }

    /// The rename's side: say the step is done, then wait for the release.
    pub(crate) async fn hold(&self) {
        self.reached.notify_one();
        self.release.notified().await;
    }
}

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
/// sibling `.<old>.renaming` (the leading dot keeps it clear of every
/// domain name), so the spelling really changes and the second
/// half never sees its target as already present; a leftover
/// `.<old>.renaming` from an interrupted run is carried on to `new`. A
/// DIFFERENT folder the filesystem folds onto `new` counts as present.
///
/// On Windows a folder that holds an open file (the overlay journal, or one
/// a watcher is reading) cannot be renamed; the caller pauses the domain
/// before this step so nothing holds one.
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
    // A leading dot, which `validate_domain_name` refuses, so no domain's
    // own folder can ever carry this name.
    let temp = format!(".{old}.renaming");
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
            // With the folder moved aside, anything that answers to `new` is
            // a different folder, the exact spelling or one the filesystem
            // folds onto it, and renaming onto it could replace it.
            if has(new) || parent.join(new).symlink_metadata().is_ok() {
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

    /// `eng.renaming` is a valid domain name, so a folder of that name is
    /// another domain's state, never a leftover of this move: renaming `eng`,
    /// which has no folder here, leaves it alone.
    #[test]
    fn another_domains_folder_named_like_a_leftover_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        std::fs::create_dir(parent.join("eng.renaming")).unwrap();
        std::fs::write(parent.join("eng.renaming/file"), "theirs").unwrap();

        move_state_dir(parent, "eng", "platform").unwrap();
        move_state_dir(parent, "eng", "Eng").unwrap();
        assert_eq!(listing(parent), vec!["eng.renaming".to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.join("eng.renaming/file")).unwrap(),
            "theirs"
        );
    }

    /// With the folder moved aside to its temporary name, a different folder
    /// the filesystem folds onto `new` is refused rather than replaced. Skips
    /// (early return, message on stderr) where case does not fold.
    #[test]
    fn a_replay_onto_a_folder_folded_onto_the_target_is_refused_where_case_folds() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        if !folds_case(parent) {
            eprintln!("skipped: {} is case-sensitive", parent.display());
            return;
        }
        std::fs::create_dir(parent.join(".Notes.renaming")).unwrap();
        std::fs::write(parent.join(".Notes.renaming/file"), "kept").unwrap();
        std::fs::create_dir(parent.join("NOTES")).unwrap();

        let err = move_state_dir(parent, "Notes", "notes").unwrap_err();
        assert!(
            err.to_string().contains("both Notes and notes exist"),
            "{err}"
        );
        assert_eq!(
            listing(parent),
            vec![".Notes.renaming".to_string(), "NOTES".to_string()],
            "nothing moved and the empty folder was not replaced"
        );
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
        std::fs::create_dir(parent.join(format!(".{composed}.renaming"))).unwrap();
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
        std::fs::create_dir(parent.join(".Notes.renaming")).unwrap();
        std::fs::write(parent.join(".Notes.renaming/file"), "kept").unwrap();

        move_state_dir(parent, "Notes", "notes").unwrap();
        assert_eq!(listing(parent), vec!["notes".to_string()]);
        assert_eq!(
            std::fs::read_to_string(parent.join("notes/file")).unwrap(),
            "kept"
        );
    }
}
