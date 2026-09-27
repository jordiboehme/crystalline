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
    /// What the relink step did, recorded with the step so a rename finished
    /// by a later call or at the next start reports the whole of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relinked: Option<RelinkReport>,
    /// The index, configuration and state directory the rename was started
    /// against. A journal lives in the state directory, but the index a
    /// command opens can be another one (`--db`, `--config`), and running the
    /// journal's steps there would move this machine's state and
    /// configuration while the index it belongs to keeps the old name. Only
    /// an engine that opened the same three runs it. A journal without one is
    /// run by nobody.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<RenameOwner>,
    /// The steps completed so far, in order.
    pub done: Vec<RenameStep>,
}

/// What a rename journal belongs to: the index, the configuration file and
/// the state directory of the engine that started it, each path in its
/// canonical form so two spellings of one file compare equal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenameOwner {
    /// The index as its store names it: the database file, or the Postgres
    /// host and database without credentials. `None` for an in-memory store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    /// The configuration file the engine persists to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// The state directory the journal lives in.
    pub state_dir: String,
}

impl RenameOwner {
    /// The owner for an engine over `index` (as its store names it), `config`
    /// and `state_dir`.
    pub fn new(index: Option<&str>, config: Option<&Path>, state_dir: &Path) -> RenameOwner {
        RenameOwner {
            index: index.map(|i| canonical_text(Path::new(i))),
            config: config.map(canonical_text),
            state_dir: canonical_text(state_dir),
        }
    }

    /// The owner for an engine that opens `db_path` under `loaded`'s
    /// configuration, named without opening anything: the index as its store
    /// would name it ([`crystalline_index::store_location`]), the
    /// configuration file `loaded` resolved and `state_dir`.
    pub fn for_opened(
        loaded: &crate::overlay::LoadedConfig,
        db_path: &Path,
        state_dir: &Path,
    ) -> RenameOwner {
        RenameOwner::new(
            crystalline_index::store_location(&loaded.effective.database(), Some(db_path))
                .as_deref(),
            Some(&loaded.path),
            state_dir,
        )
    }

    /// The same owner with every path spelled canonically as of now: a
    /// journal or a machine owner recorded before a file existed compares
    /// equal to one recorded after. Applied to both sides of a comparison.
    pub fn normalized(&self) -> RenameOwner {
        RenameOwner {
            index: self.index.as_deref().map(|i| canonical_text(Path::new(i))),
            config: self.config.as_deref().map(|c| canonical_text(Path::new(c))),
            state_dir: canonical_text(Path::new(&self.state_dir)),
        }
    }

    /// Whether `self` and `other` name the same index, configuration and
    /// state directory, each spelled canonically as of now.
    pub fn same_as(&self, other: &RenameOwner) -> bool {
        self.normalized() == other.normalized()
    }

    /// The three, for a log line or a refusal.
    pub fn describe(&self) -> String {
        format!(
            "index {}, configuration {}, state directory {}",
            self.index.as_deref().unwrap_or("in memory"),
            self.config.as_deref().unwrap_or("none"),
            self.state_dir
        )
    }

    /// Each part in which `self` differs from `other`, in words that name
    /// both sides (`self` first as "this command opened", `other` second as
    /// "this machine's own"); empty when the two are the same.
    pub fn differences_from(&self, other: &RenameOwner) -> Vec<String> {
        let (this, other) = (self.normalized(), other.normalized());
        let (this, other) = (&this, &other);
        let side =
            |value: &Option<String>, none: &str| value.clone().unwrap_or_else(|| none.to_string());
        let mut parts = Vec::new();
        if this.index != other.index {
            parts.push(format!(
                "the index {} is not this machine's own index {}",
                side(&this.index, "in memory"),
                side(&other.index, "in memory")
            ));
        }
        if this.config != other.config {
            parts.push(format!(
                "the configuration {} is not this machine's own configuration {}",
                side(&this.config, "none"),
                side(&other.config, "none")
            ));
        }
        if this.state_dir != other.state_dir {
            parts.push(format!(
                "the state directory {} is not this machine's own state directory {}",
                this.state_dir, other.state_dir
            ));
        }
        parts
    }
}

/// This machine's own index, configuration and state directory, as a rename
/// journal records them: what a plain command, with no `--db` and no
/// `--config`, opens, the environment overlay included. For Turso the index
/// is always `index.db` in the state directory; for Postgres it is the
/// database the configuration names.
pub fn machine_rename_owner() -> anyhow::Result<RenameOwner> {
    let state_dir = crystalline_core::config::state_dir()?;
    let loaded = crate::overlay::load(None)?;
    let db_path = crystalline_core::config::index_db_path()?;
    Ok(RenameOwner::for_opened(&loaded, &db_path, &state_dir))
}

/// What an opener told an engine about this machine's own index,
/// configuration and state directory ([`crate::engine::Engine::with_machine_owner_lookup`]).
#[derive(Debug, Clone)]
pub enum MachineOwner {
    /// Named: a rename and a name adoption run only when the engine opened
    /// exactly these.
    Known(RenameOwner),
    /// The lookup failed, for the reason given (this machine's default
    /// configuration does not load, say). Nothing can be compared, so a
    /// rename is refused, a name adoption and the finishing of a rename
    /// journal are skipped, until the lookup works again.
    Unknown(String),
}

/// A rename journal as it waits in a state directory, for a caller outside
/// the engine: `doctor` names it, and a command that opened another index
/// than this machine's own can tell whether it is that journal's to finish.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRename {
    /// The local name the domain had.
    pub old: String,
    /// The local name it gets.
    pub new: String,
    /// Whether the rename leaves the MANIFEST and content alone.
    pub local_only: bool,
    /// What the journal belongs to; `None` for a journal that does not say.
    pub owner: Option<RenameOwner>,
    /// The steps completed so far, in order.
    pub done: Vec<RenameStep>,
    /// The steps still to run, in order.
    pub remaining: Vec<RenameStep>,
}

/// The rename journal waiting under `state_dir`, or `None` when no rename is
/// pending. A journal that does not parse is an error naming the file.
pub fn pending_rename(state_dir: &Path) -> io::Result<Option<PendingRename>> {
    Ok(RenameJournal::load(state_dir)?.map(|journal| {
        let remaining = journal
            .steps()
            .iter()
            .copied()
            .filter(|step| !journal.done.contains(step))
            .collect();
        PendingRename {
            old: journal.old,
            new: journal.new,
            local_only: journal.local_only,
            owner: journal.owner,
            done: journal.done,
            remaining,
        }
    }))
}

/// Delete the rename journal under `state_dir`, leaving every step it
/// already ran as it is. Only for a person who decided to finish or undo
/// that rename by hand.
pub fn discard_pending_rename(state_dir: &Path) -> io::Result<()> {
    RenameJournal::remove(state_dir)
}

/// `path` in its canonical form, the same before and after the file exists.
///
/// An existing path is canonicalized whole. An absolute path to a file not
/// created yet (a configuration a daemon started before its first `domain
/// add`) is spelled as its nearest existing ancestor, canonicalized, with the
/// rest appended: so a folder reached through a symlink (`/tmp` on macOS) or
/// spelled with Windows' verbatim `\\?\` prefix once canonical compares
/// equal to itself before and after the file is written. A relative path
/// that does not exist (a Postgres `host:port/db` location among them) is
/// kept as given.
pub(crate) fn canonical_text(path: &Path) -> String {
    canonical_path(path).display().to_string()
}

fn canonical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if !path.is_absolute() {
        return path.to_path_buf();
    }
    let mut missing = Vec::new();
    let mut current = path;
    while let (Some(parent), Some(name)) = (current.parent(), current.file_name()) {
        missing.push(name.to_os_string());
        if let Ok(mut canonical) = std::fs::canonicalize(parent) {
            for name in missing.iter().rev() {
                canonical.push(name);
            }
            return canonical;
        }
        current = parent;
    }
    path.to_path_buf()
}

/// What a full rename's relink step respelled and what it left alone.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct RelinkReport {
    /// Per domain, in name order.
    pub rewritten: Vec<RelinkCount>,
    /// The engrams it found and did not respell.
    pub left_behind: Vec<LeftBehind>,
}

/// The engrams and references the relink step respelled in one domain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RelinkCount {
    pub domain: String,
    pub engrams: usize,
    pub references: usize,
}

/// One engram whose links the relink step did not respell: in a domain the
/// caller could only read, or one whose rewrite failed (`reason`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct LeftBehind {
    pub domain: String,
    pub path: String,
    pub references: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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

    /// A configuration not written yet, below a folder reached through a
    /// symlink (`/tmp` on macOS), is spelled the way it will be once it
    /// exists, so an owner recorded before the first `domain add` compares
    /// equal to one recorded after it.
    #[cfg(unix)]
    #[test]
    fn a_missing_file_below_a_symlinked_folder_is_spelled_as_it_will_be() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
        let path = tmp.path().join("link/crystalline/config.yaml");

        let before = canonical_text(&path);
        assert_eq!(
            before,
            std::fs::canonicalize(&real)
                .unwrap()
                .join("crystalline/config.yaml")
                .display()
                .to_string()
        );
        std::fs::create_dir_all(real.join("crystalline")).unwrap();
        std::fs::write(real.join("crystalline/config.yaml"), "").unwrap();
        assert_eq!(canonical_text(&path), before);

        let owner = |config: &str| RenameOwner {
            index: None,
            config: Some(config.to_string()),
            state_dir: "/state".to_string(),
        };
        assert!(owner(&path.display().to_string()).same_as(&owner(&before)));
        // A Postgres location is no path and stays as given.
        assert_eq!(
            canonical_text(Path::new("localhost:5432/kb")),
            "localhost:5432/kb"
        );
    }

    /// On Windows a canonical path carries the verbatim `\\?\` prefix; a
    /// file not written yet gets it too, the same spelling it has once it
    /// exists.
    #[cfg(windows)]
    #[test]
    fn a_missing_file_carries_the_verbatim_prefix_it_will_have() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("crystalline").join("config.yaml");
        let before = canonical_text(&path);
        assert!(before.starts_with(r"\\?\"), "{before}");
        std::fs::create_dir_all(tmp.path().join("crystalline")).unwrap();
        std::fs::write(&path, "").unwrap();
        assert_eq!(canonical_text(&path), before);
    }

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
