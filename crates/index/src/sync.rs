//! The sync engine: bring the index in step with a domain's files on disk.
//!
//! Files on disk are the source of truth; the index is derived. A sync walks the
//! domain folder, uses a modification-time and size prefilter to avoid hashing
//! unchanged files, hashes the survivors with SHA-256, classifies each as new,
//! modified, deleted or moved (a moved file has an identical checksum to a
//! vanished path and is renamed in place without reparsing), parses only the
//! genuinely changed files, applies everything in one transaction and resolves
//! forward references in a single batch at the end.
//!
//! A domain's own batch can only resolve references into domains that are
//! already indexed, so a run over several domains ends with one more pass:
//! [`resolve_forward_refs`] re-runs the same two store methods over every
//! domain the run applied, once all of them are in, and a reference pointing
//! forward into a domain registered later no longer has to wait for the next
//! sync to stop reading as unresolved.
//!
//! Hashing and parsing run off-thread with bounded concurrency; all database
//! writes stay on the calling task and commit together.
//!
//! # Two phases, so the store lock only covers database work
//!
//! A sync is two phases: [`scan_domain`] is pure filesystem and CPU (walk, stat,
//! hash) and takes the stamp snapshot as input rather than reading the store, so
//! a caller runs it with no store lock held; [`apply_scan`] is the transactional
//! apply and touches the store only. [`sync_domain_with`] composes the two for
//! callers that do not manage the lock. Splitting them keeps the store mutex off
//! the long walk-and-hash pass of a large domain.
//!
//! # Bounded memory: the changed set is applied in slabs
//!
//! The scan classifies with checksums alone and keeps no file contents, so a
//! [`DomainScan`] is metadata whatever the domain's size. The apply then works
//! through the changed set in slabs of [`SYNC_SLAB_FILES`] files: each slab is
//! read, parsed and chunked off-thread and every result is upserted as it
//! arrives, so the pipeline never holds more than one slab of contents at a time.
//! A full sync of a multi-gigabyte domain therefore peaks at slab size, not at a
//! multiple of the domain.
//!
//! The price is that a changed file is read twice, once to hash it and once to
//! parse it, and that the parse of each slab runs inside the apply's transaction.
//! Both are deliberate: the checksums of the whole changed set must be known
//! before the first write, because a vanished path is only a delete once no new
//! file claims its checksum as a move, and deletes must land before the upserts
//! so a file that moves and is edited in one pass frees its permalink for the new
//! path. The second read is page-cache warm in practice, and the parse only ever
//! covers a slab.
//!
//! [`scan_paths`] is a second front on the same classification machinery for the
//! file watcher: its candidates come from a given list of relative paths instead
//! of a full walk, so a one-file edit in a large domain costs one stat and one
//! hash rather than walking every entry. Both fronts feed the identical
//! [`apply_scan`], so the targeted pass inherits every TOCTOU guard unchanged.
//! The watcher's full fallback, the startup sync and manual sync all reconcile
//! any gap, so the targeted front only has to be convergent, never perfect.
//!
//! # Attachments ride the same two phases
//!
//! Files under the reserved `assets/` prefix are not engrams: they are hashed
//! and recorded as attachment metadata instead, and nothing under the prefix is
//! ever parsed as markdown. The scan classifies them (stat plus the allowlist,
//! no bytes read), the apply reads and hashes only those whose size or mtime
//! moved against the recorded row and reconciles the rows in the same
//! transaction. A full walk deletes every row it did not see; a targeted scan
//! only touches the paths it was given. Virtual domains have no walker, so their
//! attachments are written through the engine seam alone.
//!
//! # Convergence under a concurrent writer
//!
//! Between the snapshot and the apply another writer (an MCP edit or a second
//! instance in collaboration mode) can change both the index and the files.
//! [`apply_scan`] re-reads the live stamps inside its transaction and skips any
//! classified change whose live db stamp no longer matches the snapshot it was
//! classified against (a delete additionally skips when its file reappeared on
//! disk), counting each skip in [`SyncReport::deferred`]. Every skip is safe
//! because it leaves the system in a state a later pass reconciles: a skip on a
//! changed db stamp leaves an index state newer than the scan, and a skip on a
//! reappeared file leaves a watcher event already queued for that write. In both
//! cases the next sync sees the divergence through the stamp prefilter. No skip
//! can wedge permanently, because a stamp only changes when content changes, so
//! the prefilter keeps re-selecting a diverged path until an uncontended pass
//! applies it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use walkdir::WalkDir;

use crystalline_core::reflection::ReflectionFile;
use crystalline_core::{
    DomainLayout, MAX_ATTACHMENT_BYTES, PARSE_GENERATION, PathClass, ReflectionMode,
    attachment_mime, parse_engram, slash_rel, validate_asset_path,
};

use crate::embed::{ChunkParams, chunk_engram};
use crate::error::{IndexError, Result};
use crate::store::{
    AssetTable, AttachmentRow, DomainId, DomainKind, EngramRecord, FileStamp, NewChunk,
    RebuildKind, ReflectionRow, Store,
};

/// Maximum concurrent hashing or parsing tasks.
const CONCURRENCY: usize = 8;

/// Changed files read, parsed and upserted per slab of the apply phase. The
/// pipeline holds one slab of contents at most, so this bounds a sync's peak
/// memory independently of the domain's size.
const SYNC_SLAB_FILES: usize = 256;

/// What one path of an apply became, as the daemon announces it to whoever is
/// listening. The counts on [`SyncReport`] say how many; this says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathChangeKind {
    Added,
    Modified,
    Deleted,
    Moved,
}

/// One path the apply moved, deleted, added or rewrote. Never serialized: the
/// report's JSON is a summary for people and the changes are for the engine's
/// change bus, which announces them one by one and then forgets them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChange {
    pub kind: PathChangeKind,
    /// After the change; for `Deleted`, the path that went.
    pub path: String,
    /// `Moved` only: the path before.
    pub from: Option<String>,
    /// `Moved` only: the permalink the row answered to before the rename. The
    /// store decides whether a permalink follows its path, so both ends are
    /// read from it rather than derived.
    pub from_permalink: Option<String>,
    /// The permalink after the change, when the index could say.
    pub permalink: Option<String>,
    /// The content's lowercase hex SHA-256, the same value the stamp records
    /// and every write receipt reports. Absent for `Deleted`.
    pub checksum: Option<String>,
}

/// What happened to one path of a domain's Reflection, as the engine
/// announces it: the kinds of the `reflection` event a sync can find on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReflectionPathKind {
    /// A page or the pact was written.
    Changed,
    /// A page or the pact went.
    Deleted,
    /// The threads file beside a page was written or removed.
    Threads,
    /// A file under `reflections/assets/` was written or removed.
    Files,
}

/// One path of a domain's Reflection a sync found changed on disk. Never
/// serialized, like [`PathChange`]: it is for the change bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectionPathChange {
    /// Inside `reflections/`: the page for `Changed`, `Deleted` and
    /// `Threads`, `assets/...` for `Files`.
    pub path: String,
    pub kind: ReflectionPathKind,
    /// The lowercase hex SHA-256 of the file as it was read (the threads
    /// file for `Threads`), so the bus can tell the watcher's echo of an
    /// engine write from a new change. Absent when the file is gone.
    pub checksum: Option<String>,
}

/// What a complete walk saw of the Reflection, for the change bus. A
/// threads file has no row and no stamp, so the walk reads every one and
/// hands over its checksum; the engine compares them with what it saw and
/// wrote before. Content, not times: a file restored with an old time or a
/// share whose clock runs ahead or behind changes nothing here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReflectionWalk {
    /// Whether the Reflection was in effect for this walk. A walk with it
    /// off tells the engine that the next walk in effect is the switch
    /// coming on, which the `switched` event already says.
    pub effective: bool,
    /// The page each threads file belongs to (inside `reflections/`), with
    /// the lowercase hex SHA-256 of the file's bytes.
    pub threads: BTreeMap<String, String>,
    /// Every page and the pact the walk found, so a threads file that went
    /// with its page is told from one removed beside a page that stays.
    pub pages: BTreeSet<String>,
}

/// The outcome of a sync over one domain.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SyncReport {
    /// The domain name.
    pub domain: String,
    /// Newly indexed engrams.
    pub added: usize,
    /// Re-indexed engrams whose content changed.
    pub updated: usize,
    /// Engrams removed because their file was deleted.
    pub deleted: usize,
    /// Engrams renamed in place because their file moved (no reparse).
    pub moved: usize,
    /// Files unchanged since the last sync.
    pub unchanged: usize,
    /// Classified changes the apply skipped because a concurrent writer moved
    /// the db stamp (or recreated the file) between the snapshot and the apply.
    /// A later pass reconciles each one, so a non-zero count marks a busy system,
    /// not a failure.
    pub deferred: usize,
    /// Files that could not be read, parsed or upserted, with the reason.
    pub failed: Vec<(String, String)>,
    /// Forward references resolved at the end of this sync.
    pub relations_resolved: u64,
    /// Prose wikilinks resolved at the end of this sync.
    #[serde(default)]
    pub links_resolved: u64,
    /// Forward references this domain only resolved in the final cross-domain
    /// pass of a multi-domain run, once every other domain was indexed - see
    /// [`resolve_forward_refs`]. Kept apart from `relations_resolved` rather
    /// than added into it, so a reader can tell a reference that resolved
    /// against an already-indexed target from one that had to wait for a
    /// domain later in the run.
    #[serde(default)]
    pub relations_resolved_late: u64,
    /// Prose wikilinks resolved in the same final cross-domain pass.
    #[serde(default)]
    pub links_resolved_late: u64,
    /// Rows of the Reflection's tables this apply wrote or deleted.
    #[serde(default)]
    pub reflections_changed: usize,
    /// The rows this complete walk kept although it did not see their files,
    /// because the domain moved to this instance on a shared database and
    /// the move is unresolved ([`crate::HostMove`]). `None` when it kept
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_elsewhere: Option<crate::OnlyElsewhere>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Every path this apply changed, in apply order. Skipped by serde on
    /// both sides: the wire shape of a report is its counts.
    #[serde(skip)]
    pub changes: Vec<PathChange>,
    /// Every path of the Reflection this pass found changed: the pages, the
    /// pact and the files it wrote or deleted rows for, and the threads
    /// files a targeted pass was pointed at. Skipped by serde like
    /// `changes`.
    #[serde(skip)]
    pub reflection_changes: Vec<ReflectionPathChange>,
    /// Set by every complete walk, never by a targeted pass: see
    /// [`ReflectionWalk`]. Skipped by serde like `changes`.
    #[serde(skip)]
    pub reflection_walk: Option<ReflectionWalk>,
    /// Files the walk skipped or the scan or the apply could not read: what
    /// `failed` lists for want of the bytes, plus the walk entries that could
    /// not even be stat'd (a file that read but did not parse is not counted).
    /// A whole-domain reparse is not stamped while it is non-zero, so a file
    /// unreadable during the reparse is read again by the next one. Not part
    /// of the wire shape.
    #[serde(skip)]
    pub unread: usize,
    /// Whether this apply ended an unresolved host move, so what waited on
    /// the move (the Reflection backup) can follow. Not part of the wire
    /// shape.
    #[serde(skip)]
    pub host_move_resolved: bool,
}

/// A file found on disk during the walk.
struct Scanned {
    rel: String,
    abs: PathBuf,
    mtime: i64,
    size: u64,
}

/// A classified change waiting for its slab: where the file is, the stat it was
/// classified against and whether the path was already indexed (so the apply can
/// tell an add from an update). The contents are deliberately absent - the slab
/// reads them when it is its turn.
struct PendingChange {
    scanned: Scanned,
    previously_indexed: bool,
}

/// One file the walk found for a row table, stat'd but not read. `rel` is
/// the row key: domain-relative for an engram attachment, the path inside
/// `reflections/` for the Reflection's rows. The apply reads only the
/// candidates whose stat moved.
#[derive(Debug, Clone)]
struct FileCandidate {
    rel: String,
    abs: PathBuf,
    mtime: i64,
    size: u64,
}

/// A row of one of the file tables, built from a file just read.
enum FileRow {
    Asset(AttachmentRow),
    Reflection(ReflectionRow),
}

impl FileRow {
    /// The checksum of the bytes the row was built from.
    fn sha256(&self) -> &str {
        match self {
            FileRow::Asset(row) => &row.sha256,
            FileRow::Reflection(row) => &row.sha256,
        }
    }
}

/// The file tables the reconcile writes, as one closed set: the asset
/// tables and the reflection table.
#[derive(Debug, Clone, Copy)]
enum FileRows {
    Assets(AssetTable),
    Reflections,
}

impl FileRows {
    /// `(size, modified)` of every row of the domain, the stat prefilter's input.
    async fn stamps<S: Store + ?Sized>(
        self,
        store: &S,
        domain: DomainId,
    ) -> Result<HashMap<String, (u64, String)>> {
        Ok(match self {
            FileRows::Assets(table) => store
                .list_assets(table, domain)
                .await?
                .into_iter()
                .map(|r| (r.path, (r.size, r.modified)))
                .collect(),
            // The stamps only: the page text is no input to the prefilter.
            FileRows::Reflections => store
                .list_reflection_stamps(domain)
                .await?
                .into_iter()
                .map(|r| (r.path, (r.size, r.modified)))
                .collect(),
        })
    }

    /// Read `c` and build its row, or `None` to skip it (a file grown past
    /// the attachment ceiling between the stat and the read). Blocking.
    fn build(self, c: &FileCandidate, modified: String) -> std::io::Result<Option<FileRow>> {
        match self {
            FileRows::Assets(_) => {
                // The size is taken from the bytes that were actually read, so
                // a file that grew past the cap between the stat and the read
                // is caught here rather than stored over the ceiling.
                let (sha256, size) = read_and_measure(&c.abs)?;
                if size > MAX_ATTACHMENT_BYTES {
                    tracing::debug!(path = %c.rel, size, "sync: skipping an attachment over the size cap");
                    return Ok(None);
                }
                let name = c.rel.rsplit('/').next().unwrap_or(&c.rel);
                let Some(mime) = attachment_mime(name) else {
                    return Ok(None);
                };
                Ok(Some(FileRow::Asset(AttachmentRow {
                    path: c.rel.clone(),
                    sha256,
                    mime: mime.to_string(),
                    size,
                    modified,
                })))
            }
            FileRows::Reflections => {
                let content = read_page(&c.abs)?;
                Ok(Some(FileRow::Reflection(ReflectionRow {
                    title: crystalline_core::reflection::title_of(&content, &c.rel),
                    sha256: sha256_hex(content.as_bytes()),
                    size: content.len() as u64,
                    modified,
                    path: c.rel.clone(),
                    content,
                })))
            }
        }
    }

    async fn upsert<S: Store + ?Sized>(
        self,
        store: &S,
        domain: DomainId,
        row: &FileRow,
        chunk_params: &ChunkParams,
    ) -> Result<()> {
        match (self, row) {
            (FileRows::Assets(table), FileRow::Asset(row)) => {
                store.upsert_asset(table, domain, row).await
            }
            // The row and its chunks, through the one funnel (R53).
            (FileRows::Reflections, FileRow::Reflection(row)) => {
                crate::reflection::index_reflection_row(store, domain, row, chunk_params).await
            }
            // `build` answers the row of its own table, so the pairing never
            // crosses; an arm that made it cross is a bug, not a skip.
            (FileRows::Assets(_), FileRow::Reflection(_))
            | (FileRows::Reflections, FileRow::Asset(_)) => {
                unreachable!("a file row reached another table's upsert")
            }
        }
    }

    async fn delete<S: Store + ?Sized>(
        self,
        store: &S,
        domain: DomainId,
        path: &str,
    ) -> Result<bool> {
        match self {
            FileRows::Assets(table) => store.delete_asset(table, domain, path).await,
            FileRows::Reflections => store.delete_reflection(domain, path).await,
        }
    }

    /// The kind a written (or, with `deleted`, a removed) row of this table
    /// is announced as on the change bus; `None` for the engram attachments,
    /// whose changes are no Reflection's.
    fn announced_as(self, deleted: bool) -> Option<ReflectionPathKind> {
        match (self, deleted) {
            (FileRows::Assets(AssetTable::Engram), _) => None,
            (FileRows::Assets(AssetTable::Reflection), _) => Some(ReflectionPathKind::Files),
            (FileRows::Reflections, false) => Some(ReflectionPathKind::Changed),
            (FileRows::Reflections, true) => Some(ReflectionPathKind::Deleted),
        }
    }

    /// The domain-relative path a failure is reported under.
    fn report_path(self, rel: &str) -> String {
        match self {
            FileRows::Assets(AssetTable::Engram) => rel.to_string(),
            FileRows::Assets(AssetTable::Reflection) | FileRows::Reflections => {
                crystalline_core::reflection::domain_rel(rel)
            }
        }
    }
}

/// The Reflection mode of the domain at `root`: `Effective` when its MANIFEST
/// says `reflection: on` and no engram sits under `reflections/`. A switch
/// that arrived while engrams still live there (a plain editor, a pull)
/// stays pending, and pending reads as `Off`, so no sync ever drops an
/// engram from the index because of it.
///
/// What an engram under `reflections/` is, is decided by one predicate,
/// [`crystalline_core::is_pending_engram`] (R48): a file the walk would index
/// as an engram that carries no page marker. It is asked of the `recorded`
/// engram paths under the folder, read from disk; a recorded file that no
/// longer reads as an engram (frontmatter that does not parse, bytes that
/// are not UTF-8) keeps the row of its last good version, so it counts as
/// pending ([`crystalline_core::EngramRow::Recorded`]). A domain the index has
/// synced always records its `MANIFEST.md`, so no recorded path at all means
/// the index holds no row of the domain (a wipe and rebuild, a new machine's
/// first sync, a schema rebuild): then it is asked of every file under
/// `reflections/` instead, so a rebuild keeps a pending switch pending (R1);
/// there a file that does not read as an engram is none, since no row of
/// it exists ([`crystalline_core::EngramRow::Unknown`]).
pub fn reflection_mode(
    root: &Path,
    recorded: impl Iterator<Item = impl AsRef<str>>,
) -> ReflectionMode {
    if crystalline_core::reflection_at(root) != crystalline_core::ReflectionSwitch::On {
        return ReflectionMode::Off;
    }
    if pending_engrams_up_to(root, recorded, Some(1)).is_empty() {
        ReflectionMode::Effective
    } else {
        ReflectionMode::Off
    }
}

/// The engrams under `reflections/` that wait to move out, sorted: every
/// path [`crystalline_core::is_pending_engram`] calls pending, asked of the
/// same files [`reflection_mode`] asks it of (the `recorded` paths, or the
/// files on disk when nothing is recorded). The switch is not read, so the
/// switch-on screen can ask before the switch is on. A folder or an entry the
/// walk cannot read is named as it stands, since it may hold an engram
/// (fail closed, as [`reflection_mode`] fails).
pub fn pending_engrams(
    root: &Path,
    recorded: impl Iterator<Item = impl AsRef<str>>,
) -> Vec<String> {
    let mut pending = pending_engrams_up_to(root, recorded, None);
    pending.sort();
    pending
}

/// [`pending_engrams`] in the order found, stopping after `limit` when one
/// is given. A Reflection folder spelled in two letter cases (a
/// case-sensitive filesystem) holds the switch whatever its files are: the
/// two would land on the same rows, so each spelling is named as a folder,
/// the way a folder the walk cannot read is.
fn pending_engrams_up_to(
    root: &Path,
    recorded: impl Iterator<Item = impl AsRef<str>>,
    limit: Option<usize>,
) -> Vec<String> {
    let folders = crystalline_core::reflection::reflection_folders(root);
    if folders.len() > 1 {
        return folders
            .into_iter()
            .map(|folder| format!("{folder}/"))
            .take(limit.unwrap_or(usize::MAX))
            .collect();
    }
    let mut recorded = recorded.peekable();
    if recorded.peek().is_none() {
        return pending_engrams_on_disk(root, limit);
    }
    let pending = recorded.map(|p| p.as_ref().to_string()).filter(|p| {
        crystalline_core::reflection::is_under_reflections(p)
            && crystalline_core::is_pending_engram(root, p, crystalline_core::EngramRow::Recorded)
    });
    match limit {
        Some(limit) => pending.take(limit).collect(),
        None => pending.collect(),
    }
}

/// The files under `reflections/` on disk that are pending engrams
/// ([`crystalline_core::is_pending_engram`]), up to `limit`. A folder the
/// walk cannot read is named instead of its files, since it may hold one.
fn pending_engrams_on_disk(root: &Path, limit: Option<usize>) -> Vec<String> {
    let mut found = Vec::new();
    let full = |found: &Vec<String>| limit.is_some_and(|limit| found.len() >= limit);
    let folder_rel = format!("{}/", crystalline_core::reflection::REFLECTIONS_DIR);
    let Ok(entries) = std::fs::read_dir(root) else {
        return vec![folder_rel];
    };
    for entry in entries {
        let Ok(entry) = entry else {
            found.push(folder_rel.clone());
            return found;
        };
        // The classifier's case rule: `Reflections/` is the Reflection's folder too.
        let is_folder = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(crystalline_core::reflection::REFLECTIONS_DIR));
        if !is_folder {
            continue;
        }
        // The walk enters what the engram walk enters (no hidden folder, so
        // never the baseline); the predicate classifies each file.
        let off = DomainLayout::bare(ReflectionMode::Off);
        let walk = WalkDir::new(entry.path())
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || off.enters_dir(&slash_rel(root, e.path())));
        for item in walk {
            let item = match item {
                Ok(item) => item,
                Err(e) => {
                    let at = e.path().unwrap_or(&entry.path()).to_path_buf();
                    found.push(slash_rel(root, &at));
                    if full(&found) {
                        return found;
                    }
                    continue;
                }
            };
            let rel = slash_rel(root, item.path());
            if item.file_type().is_file()
                && crystalline_core::is_pending_engram(
                    root,
                    &rel,
                    crystalline_core::EngramRow::Unknown,
                )
            {
                found.push(rel);
                if full(&found) {
                    return found;
                }
            }
        }
    }
    found
}

/// Whether the Reflection's folder of the domain at `root` holds a file that
/// a walk with the Reflection in effect gives a row: a page or the pact that
/// reads as text, or a file under `reflections/assets/` the attachment rules
/// take (its extension, its size, the folder's spelling). Stops at the first
/// one. The rules are the walk's own (`read_page`,
/// `reflection_asset_candidate`), so a folder whose files the walk skips
/// never asks for a walk that would skip them again. An entry that cannot
/// be read is skipped the same way: the walk gives it no row either.
pub fn reflection_files_on_disk(root: &Path) -> bool {
    let layout = DomainLayout::at(root, ReflectionMode::Effective);
    for folder in crystalline_core::reflection::reflection_folders(root) {
        let walk = WalkDir::new(root.join(&folder))
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || layout.enters_dir(&slash_rel(root, e.path())));
        for item in walk.flatten() {
            if !item.file_type().is_file() {
                continue;
            }
            let rel = slash_rel(root, item.path());
            let gets_a_row = match layout.classify(&rel) {
                PathClass::Reflection(ReflectionFile::Page(_) | ReflectionFile::Pact) => {
                    read_page(item.path()).is_ok()
                }
                PathClass::Reflection(ReflectionFile::Asset(_)) => {
                    item.metadata().is_ok_and(|meta| {
                        let fname = item.file_name().to_string_lossy();
                        reflection_asset_candidate(
                            root,
                            &rel,
                            item.path(),
                            &fname,
                            file_mtime(&meta),
                            meta.len(),
                        )
                        .is_some()
                    })
                }
                _ => false,
            };
            if gets_a_row {
                return true;
            }
        }
    }
    false
}

/// The text of the Reflection page or pact at `abs`, as its row is built
/// from it: a page that is not UTF-8 gets no row.
fn read_page(abs: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(abs)
}

/// [`asset_candidate`] for a file under `reflections/assets/`, at the
/// domain-relative `rel`: rooted at the Reflection folder as it is spelled
/// on disk, so its canonical fold and its existence probe work as they do
/// at the domain root, and keyed by its path inside that folder.
fn reflection_asset_candidate(
    root: &Path,
    rel: &str,
    abs: &Path,
    fname: &str,
    mtime: i64,
    size: u64,
) -> Option<FileCandidate> {
    let (folder, inner) = rel.split_once('/').unwrap_or((rel, ""));
    asset_candidate(&root.join(folder), inner, abs, fname, mtime, size)
}

/// Whether the domain at `root` has a Reflection in effect.
pub fn reflection_effective(root: &Path, recorded: impl Iterator<Item = impl AsRef<str>>) -> bool {
    reflection_mode(root, recorded) == ReflectionMode::Effective
}

/// The path inside `reflections/` of a domain-relative path under it, the
/// key the Reflection's rows carry.
fn inner_rel(rel: &str) -> String {
    rel.split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .to_string()
}

/// Sync one domain: walk `root`, reconcile the index and resolve forward refs.
///
/// Chunks are computed with the default parameters (the local model id). Use
/// [`sync_domain_with`] to fingerprint chunks for a specific configured model.
pub async fn sync_domain<S: Store + ?Sized>(
    store: &S,
    name: &str,
    root: &Path,
) -> Result<SyncReport> {
    sync_domain_with(store, name, root, &ChunkParams::default()).await
}

/// Sync one domain, fingerprinting embedding chunks for a specific model.
///
/// After each changed engram is upserted, its body is chunked and the chunk rows
/// are reconciled through [`Store::replace_chunks`], which carries over any
/// embedding whose fingerprint is unchanged. An unchanged file is skipped by the
/// prefilter before this point, so it produces no chunk work at all.
pub async fn sync_domain_with<S: Store + ?Sized>(
    store: &S,
    name: &str,
    root: &Path,
    chunk_params: &ChunkParams,
) -> Result<SyncReport> {
    let domain = store
        .upsert_domain(
            name,
            Some(&root.to_string_lossy()),
            crate::store::DomainKind::File,
        )
        .await?;
    let stamps = store.file_stamps(domain).await?;
    let force = reparse_due(store, domain).await?;
    let scan = scan_domain(name, root, stamps, chunk_params, force).await?;
    apply_scan(store, domain, scan).await
}

/// Whether a domain's rows were derived by an older parser than this one, so
/// its next sync must reparse every engram rather than only the changed ones.
///
/// A file domain does that by walking with `force` set, exactly as
/// `reindex --full` does: every file is re-read and re-parsed and nothing on
/// disk is written, and the apply that commits the walk stamps the current
/// [`PARSE_GENERATION`] in the same transaction (see [`apply_scan`]). A
/// virtual domain does it through [`reparse_stored_domain`]. Either way a run
/// that dies before its commit leaves the old generation standing, so the
/// next sync repeats it, and repeating it is harmless: a reparse of unchanged
/// text writes the rows it already wrote.
pub async fn reparse_due<S: Store + ?Sized>(store: &S, domain: DomainId) -> Result<bool> {
    Ok(store.parse_generation(domain).await? < PARSE_GENERATION)
}

/// Reparse every base engram of a virtual domain from the content the
/// database stores for it, once after a parser change, and stamp the current
/// [`PARSE_GENERATION`]. Answers how many engrams were reparsed, `0` when the
/// domain is already current.
///
/// The stored content is the domain's source of truth and stays byte for byte
/// what it was: each row is rewritten with its own content, the checksum and
/// size read with that content, and its recorded modification time (so that
/// does not move), which rebuilds its observations, relations, links and
/// tags, and its chunks are reconciled through [`Store::replace_chunks`],
/// which keeps every embedding whose text is unchanged. Each write goes
/// through [`Store::update_engram_checked`], so it never inserts: a row that
/// changed since it was read is left to the write that changed it, which
/// already parsed it with this parser; a row deleted or renamed meanwhile
/// stays gone; and a row whose content no longer parses keeps the rows it has.
///
/// One transaction: the stamp commits with the rows, so an interrupted run
/// leaves the old generation and the next sync repeats it. Idempotent, which
/// is what makes it safe on a shared Postgres database: two instances that
/// both find the domain behind write the same rows and the same stamp.
/// Drafts (actor rows) are not touched; each is reparsed on its next write.
pub async fn reparse_stored_domain<S: Store + ?Sized>(
    store: &S,
    name: &str,
    domain: DomainId,
    chunk_params: &ChunkParams,
) -> Result<usize> {
    reparse_stored_domain_with(store, name, domain, chunk_params, &|_| Box::pin(async {})).await
}

/// Where a [`reparse_stored_domain_with`] seam runs: after the stamp map is
/// read and before the contents are, and after both reads before the first
/// write. The two windows another writer can land in.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReparseStep {
    /// The recorded stamps are read; the contents are not yet.
    StampsRead,
    /// Stamps and contents are read; nothing is written yet.
    ContentsRead,
}

/// What a [`reparse_stored_domain_with`] seam answers.
#[doc(hidden)]
pub type ReparseSeam<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

/// [`reparse_stored_domain`] with a seam awaited at each [`ReparseStep`]: a
/// test-only way to put another writer's change exactly between two of the
/// reparse's statements. Production passes a seam that does nothing.
#[doc(hidden)]
pub async fn reparse_stored_domain_with<'a, S: Store + ?Sized>(
    store: &S,
    name: &str,
    domain: DomainId,
    chunk_params: &ChunkParams,
    seam: &'a (dyn Fn(ReparseStep) -> ReparseSeam<'a> + Sync),
) -> Result<usize> {
    if !reparse_due(store, domain).await? {
        return Ok(0);
    }
    store.begin().await?;
    let result = async {
        let stamps = store.file_stamps(domain).await?;
        seam(ReparseStep::StampsRead).await;
        let rows = store.all_engram_contents(domain).await?;
        seam(ReparseStep::ContentsRead).await;
        // Said before the work, not after it: on a large domain this is the
        // one slow step of the first start after an upgrade, and the line is
        // what tells a person why.
        tracing::info!(
            domain = name,
            engrams = rows.len(),
            "reparsing {} engram(s) of '{name}' after a parser change",
            rows.len()
        );
        let mut reparsed = 0usize;
        for row in rows {
            let Ok(engram) = parse_engram(&row.content) else {
                continue;
            };
            // The checksum and size come from the row the content came from,
            // never from the stamp map: the two are separate statements, and
            // an edit committed between them would otherwise leave a row
            // whose checksum describes text it no longer holds, which every
            // later edit would be refused against. The stamp map gives only
            // the modification time, which is not compared against anything.
            let stamp = FileStamp {
                mtime: stamps.get(&row.path).map(|s| s.mtime).unwrap_or(0),
                size: row.content.len() as u64,
                sha256: row.sha256.clone(),
            };
            let mut record = EngramRecord::from_engram(&engram, &row.path, stamp);
            record.content = row.content.clone();
            let id = match store
                .update_engram_checked(domain, &record, &row.sha256)
                .await
            {
                Ok(Some(id)) => id,
                Ok(None) => {
                    tracing::debug!(
                        domain = name,
                        path = row.path.as_str(),
                        "reparse: skipping an engram deleted or renamed since it was read"
                    );
                    continue;
                }
                Err(IndexError::StaleEdit { .. }) => continue,
                Err(e) => return Err(e),
            };
            let chunks = chunk_engram(
                &record.title,
                record.description.as_deref(),
                &record.content,
                chunk_params,
            );
            store.replace_chunks(id, &chunks).await?;
            reparsed += 1;
        }
        store.resolve_pending_relations(domain).await?;
        store.resolve_pending_links(domain).await?;
        store.set_parse_generation(domain, PARSE_GENERATION).await?;
        Ok(reparsed)
    }
    .await;
    match result {
        Ok(n) => {
            store.commit().await?;
            Ok(n)
        }
        Err(e) => {
            let _ = store.rollback().await;
            Err(e)
        }
    }
}

/// The filesystem side of a sync, ready to apply against a store.
///
/// [`scan_domain`] produces this with no store access at all. It carries the
/// classified moves, deletes and changed files, the stamp snapshot they were
/// classified against (so the apply can detect a concurrent writer), the walk
/// root (so a delete can re-stat its file), the chunk parameters the apply
/// chunks with and the partial report (`unchanged` and `failed` counts).
/// [`apply_scan`] consumes it inside one transaction and fills in the remaining
/// report fields.
///
/// It holds no file contents at any size of domain: the changed entries are the
/// paths and stats alone, and each slab of the apply reads its own.
pub struct DomainScan {
    /// Renames: `(from, to)`, identical content moved to a new path in place.
    moves: Vec<(String, String)>,
    /// Recorded paths whose file vanished from disk, to delete from the index.
    deletes: std::collections::HashSet<String>,
    /// New and modified files to read, parse and upsert, slab by slab.
    changed: Vec<PendingChange>,
    /// The chunk fingerprinting parameters the slabs chunk with.
    chunk_params: ChunkParams,
    /// The stamp snapshot the scan classified against, keyed by relative path.
    /// The apply compares the live db stamps against these to spot a concurrent
    /// write and defer the stale change.
    snapshot: HashMap<String, FileStamp>,
    /// The walk root, so the apply can re-stat a delete candidate on disk.
    root: PathBuf,
    /// Attachable files found under `assets/`, stat'd but not yet hashed.
    assets: Vec<FileCandidate>,
    /// Asset paths the scan looked for and did not find on disk. Only a
    /// targeted scan fills this; a full walk reconciles domain-wide instead.
    asset_deletes: Vec<String>,
    /// Whether `assets` is every attachable file in the domain (a full walk),
    /// so the apply may delete every row it did not see.
    assets_complete: bool,
    /// Reflection pages (and the pact) found while the Reflection is in effect.
    reflections: Vec<FileCandidate>,
    /// Reflection paths a targeted scan looked for and did not find.
    reflection_deletes: Vec<String>,
    /// Allowlisted files under `reflections/assets/`, while the Reflection is
    /// effective, keyed by their path inside `reflections/`. Their
    /// completeness is `reflections_complete`.
    reflection_assets: Vec<FileCandidate>,
    /// Reflection file paths a targeted scan looked for and did not find.
    reflection_asset_deletes: Vec<String>,
    /// Whether `reflections` is every reflection of the domain (a full walk
    /// that read everything under the folder), so the apply may delete every
    /// row it did not see - which is also how a switch turned off (or
    /// pending) clears the table.
    reflections_complete: bool,
    /// Whether this complete walk ends an unresolved host move by deleting
    /// the rows it did not see: [`DomainScan::accept_disk`].
    accept_disk: bool,
    /// The instance that claimed the domain's host lock for this walk:
    /// [`DomainScan::walked_by`].
    walker: Option<String>,
    /// The Reflection mode the scan classified with. While it is in effect,
    /// an engram row under `reflections/` the walk no longer saw is deleted
    /// even though its file is on disk: the file is the Reflection's now.
    reflection: ReflectionMode,
    /// Whether this scan is the disk half of a forced rebuild, so the apply's
    /// transaction also clears the domain's rebuild marker and stamps the
    /// current [`PARSE_GENERATION`]: a forced walk re-parsed every file, so the
    /// domain's rows are this parser's. Only a forced [`scan_domain`] sets it:
    /// a targeted watcher pass landing during someone else's rebuild must not
    /// clear a marker whose rebuild never finished, nor stamp a domain it
    /// parsed only a few files of.
    ends_rebuild: bool,
    /// `unchanged` and `failed` from the scan; the apply fills in the rest.
    report: SyncReport,
    /// When the scan began, so the apply can report the total duration.
    started: Instant,
}

impl DomainScan {
    /// Apply this complete walk as an ordinary one even while the domain
    /// carries an unresolved host move: the rows it did not see are deleted
    /// and the move is cleared (`crystalline sync --accept-disk`). Only a
    /// move a walk already counted and reported is ended this way; on a
    /// move marked since, this walk counts and keeps as any other. Without
    /// it, the apply of a complete walk keeps every row it did not see while
    /// a move is unresolved and counts them. A targeted scan is unaffected.
    pub fn accept_disk(&mut self) {
        self.accept_disk = true;
    }

    /// Name the instance that claimed the domain's host lock before this
    /// walk. On a shared database the apply then checks, inside its own
    /// transaction, that the lock still names it, and refuses with
    /// [`IndexError::NotHost`] when another instance took the domain over
    /// while the walk ran: the rows are the new host's to write, and its
    /// host move must not be ended by the old host's disk. Only the instance
    /// that marked a move ends it, so a walk with no walker named keeps a
    /// move standing.
    pub fn walked_by(&mut self, instance_id: &str) {
        self.walker = Some(instance_id.to_string());
    }
}

/// Scan one domain against a stamp snapshot: walk, prefilter, hash and classify,
/// with no store access at all.
///
/// `stamps` is the recorded [`FileStamp`] per relative path the caller read from
/// the store before releasing its lock; the scan classifies every file against
/// it and hands it back inside the [`DomainScan`] so the apply can re-check it.
/// The walk and hash phases run off-thread (the walk on the blocking pool) and
/// never fail fatally: a file that cannot be read lands in `report.failed`, not
/// an error. Only a root that cannot be read is an error.
///
/// `force` is `reindex --full`: the modification-time and size prefilter is
/// skipped, so every file on disk is hashed, and a file whose content is
/// identical to the recorded one is classified as changed rather than
/// unchanged. Everything else is untouched - a new file is still new, a
/// vanished path is still a delete, and a rename is still detected and applied
/// in place rather than re-parsed and re-embedded. The scan also records that
/// it is the disk half of a rebuild, so the apply's transaction clears the
/// domain's rebuild marker as it commits.
pub async fn scan_domain(
    name: &str,
    root: &Path,
    stamps: HashMap<String, FileStamp>,
    chunk_params: &ChunkParams,
    force: bool,
) -> Result<DomainScan> {
    let started = Instant::now();

    // The walk reads the disk entry by entry (a directory listing and a stat
    // each, a read for a threads file), which on a network or synced folder
    // can take a while: it runs on the blocking pool, not on a runtime worker.
    let walked = {
        let name = name.to_string();
        let root = root.to_path_buf();
        let shown = root.display().to_string();
        tokio::task::spawn_blocking(move || walk_domain(&name, &root, stamps))
            .await
            .unwrap_or_else(|e| {
                Err(IndexError::Io {
                    path: shown,
                    source: std::io::Error::other(e.to_string()),
                })
            })?
    };
    let Walked {
        stamps,
        reflection,
        walk,
        current,
        assets,
        reflections,
        reflection_assets,
        unread,
        unread_reflections,
    } = walked;

    // Deleted candidates: recorded files no longer present on disk. A full walk
    // sees every recorded path, so deletion detection is domain-wide here.
    let deleted_paths: Vec<String> = stamps
        .keys()
        .filter(|p| !current.contains_key(*p))
        .cloned()
        .collect();

    let mut scan = classify_changes(
        name,
        root,
        stamps,
        current,
        deleted_paths,
        Vec::new(),
        chunk_params,
        started,
        force,
    )
    .await;
    // A full walk saw every file under `assets/`, so the apply may delete any
    // attachment row it did not see.
    scan.assets = assets;
    scan.assets_complete = true;
    scan.ends_rebuild = force;
    // With the Reflection not in effect the walk finds no reflection, so a
    // complete walk clears the table: that is how switching off, and a
    // pending switch, leave no row behind.
    scan.reflections = reflections;
    scan.reflection_assets = reflection_assets;
    scan.reflections_complete = !unread_reflections;
    scan.reflection = reflection;
    scan.report.unread += unread;
    scan.report.reflection_walk = Some(walk);
    Ok(scan)
}

/// What [`walk_domain`] found on disk, with the stamps it was handed back.
struct Walked {
    stamps: HashMap<String, FileStamp>,
    reflection: ReflectionMode,
    walk: ReflectionWalk,
    current: HashMap<String, Scanned>,
    assets: Vec<FileCandidate>,
    reflections: Vec<FileCandidate>,
    reflection_assets: Vec<FileCandidate>,
    unread: usize,
    unread_reflections: bool,
}

/// The walk half of [`scan_domain`]: every blocking filesystem call it makes
/// before the hash phase, so the caller can run it on the blocking pool. Errs
/// only when the root itself cannot be read.
fn walk_domain(name: &str, root: &Path, stamps: HashMap<String, FileStamp>) -> Result<Walked> {
    // What every path of this domain is: the MANIFEST's in-root artifact
    // folders are pruned with the hidden ones (none when the MANIFEST is
    // absent or unparseable), the reserved `assets/` folder is routed to its
    // rows, and only markdown is an engram. While the Reflection is in
    // effect, `reflections/` is routed to its own rows.
    let reflection = reflection_mode(root, stamps.keys());
    let layout = DomainLayout::at(root, reflection);
    let mut walk = ReflectionWalk {
        effective: reflection == ReflectionMode::Effective,
        ..ReflectionWalk::default()
    };

    // Walk the folder, skipping dot-directories, dot-files and non-markdown.
    let mut current: HashMap<String, Scanned> = HashMap::new();
    let mut assets: Vec<FileCandidate> = Vec::new();
    let mut reflections: Vec<FileCandidate> = Vec::new();
    let mut reflection_assets: Vec<FileCandidate> = Vec::new();
    // Entries the walk could not read: a forced walk that skipped one has not
    // re-parsed the whole domain, so it must not stamp the parser generation.
    let mut unread = 0usize;
    // An entry under `reflections/` the walk could not read: the reflection
    // reconcile must not delete the rows of pages it did not see.
    let mut unread_reflections = false;
    for entry in WalkDir::new(root)
        .into_iter()
        // Prune dot-directories and dot-files, but never the walk root itself
        // (a temp or dotted root would otherwise prune the whole tree), and
        // prune the provisioned artifact folders wholesale.
        .filter_entry(|e| e.depth() == 0 || layout.enters_dir(&slash_rel(root, e.path())))
    {
        let entry = match entry {
            Ok(e) => e,
            // The walk root itself being unreadable is a domain-level
            // failure: scanning on would see zero files and mark every
            // recorded engram deleted, so a denied root errors loudly
            // instead of emptying the index.
            Err(err) if err.depth() == 0 => {
                let msg = err.to_string();
                let source = err
                    .into_io_error()
                    .unwrap_or_else(|| std::io::Error::other(msg));
                return Err(IndexError::Io {
                    path: root.display().to_string(),
                    source,
                });
            }
            Err(err) => {
                unread += 1;
                if err.path().is_some_and(|p| {
                    crystalline_core::reflection::is_under_reflections(&slash_rel(root, p))
                }) {
                    unread_reflections = true;
                }
                warn_unreadable_once(name, err.path(), &err.to_string());
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy();
        let rel = slash_rel(root, entry.path());
        match layout.classify(&rel) {
            // The `assets/` folder is reserved for attachments: nothing under
            // it is ever an engram, whatever its extension, so the branch
            // always ends the iteration. A file that is not attachable is
            // skipped outright. The folder is matched case-insensitively, the
            // same question the engine's write refusals and the daemon's
            // watcher ask, so a directory that differs only in case cannot
            // smuggle an engram in on APFS or NTFS.
            PathClass::EngramAsset => {
                let Ok(meta) = entry.metadata() else { continue };
                let mtime = file_mtime(&meta);
                if let Some(candidate) =
                    asset_candidate(root, &rel, entry.path(), &fname, mtime, meta.len())
                {
                    assets.push(candidate);
                }
            }
            PathClass::Engram => {
                let meta = match entry.metadata() {
                    Ok(meta) => meta,
                    Err(err) => {
                        unread += 1;
                        warn_unreadable_once(name, Some(entry.path()), &err.to_string());
                        continue;
                    }
                };
                let mtime = file_mtime(&meta);
                current.insert(
                    rel.clone(),
                    Scanned {
                        rel,
                        abs: entry.path().to_path_buf(),
                        mtime,
                        size: meta.len(),
                    },
                );
            }
            // Hidden files, artifact folders and files that are not markdown
            // are never engrams. OKF reserves `index.md` and `log.md` at every
            // level: they are navigation surfaces, never concept documents, so
            // they are skipped here the way dot-files are. Nothing downstream
            // needs a filter: they never reach a store row, an embedding or a
            // search hit, and a row left over from before this exclusion
            // disappears on the next scan (absent on disk means deleted).
            PathClass::Hidden | PathClass::Reserved | PathClass::Artifact | PathClass::Other => {}
            // A reflection page and the pact are rows of the Reflection's own
            // index, keyed by their path inside the folder; never engrams.
            PathClass::Reflection(ReflectionFile::Page(_) | ReflectionFile::Pact) => {
                let Ok(meta) = entry.metadata() else {
                    unread += 1;
                    unread_reflections = true;
                    continue;
                };
                walk.pages.insert(inner_rel(&rel));
                reflections.push(FileCandidate {
                    rel: inner_rel(&rel),
                    abs: entry.path().to_path_buf(),
                    mtime: file_mtime(&meta),
                    size: meta.len(),
                });
            }
            // A file under `reflections/assets/` goes through the engram
            // side's own candidate builder, rooted at the Reflection folder as
            // it is spelled on disk, so its canonical fold and its existence
            // probe work as they do at the domain root.
            PathClass::Reflection(ReflectionFile::Asset(_)) => {
                let Ok(meta) = entry.metadata() else {
                    unread += 1;
                    unread_reflections = true;
                    continue;
                };
                if let Some(candidate) = reflection_asset_candidate(
                    root,
                    &rel,
                    entry.path(),
                    &fname,
                    file_mtime(&meta),
                    meta.len(),
                ) {
                    reflection_assets.push(candidate);
                }
            }
            // Threads are files, never rows: read for their checksum, which
            // is all the engine needs to tell what changed. The walk yields
            // regular files only (it never follows a link), so this read
            // cannot block on a pipe.
            PathClass::Reflection(ReflectionFile::Threads { of }) => {
                match std::fs::read(entry.path()) {
                    Ok(bytes) => {
                        walk.threads.insert(of, sha256_hex(&bytes));
                    }
                    Err(_) => {
                        unread += 1;
                        unread_reflections = true;
                    }
                }
            }
            // The baseline is no row; a temp or stray file is nobody's.
            PathClass::Reflection(ReflectionFile::Baseline { .. } | ReflectionFile::Other) => {}
        }
    }

    Ok(Walked {
        stamps,
        reflection,
        walk,
        current,
        assets,
        reflections,
        reflection_assets,
        unread,
        unread_reflections,
    })
}

/// Scan a specific list of relative paths against a stamp snapshot, the
/// path-targeted counterpart of [`scan_domain`] for the file watcher.
///
/// The classification, hashing, move detection and parsing are the identical
/// shared machinery [`scan_domain`] uses; only the candidate set differs. Each
/// given path is classified in isolation: a path that exists and is a markdown
/// file (and passes the same walk filters - not hidden, not inside a provisioned
/// artifact folder) is a change candidate, still prefiltered against `stamps`; a
/// path absent on disk but present in `stamps` is a delete candidate; a path that
/// is neither on disk nor recorded is ignored. Deletion detection is scoped to
/// `paths` - no directory is walked anywhere - so a one-file edit in a large
/// domain costs one stat and one hash, not a full walk of every entry.
///
/// Move detection stays within this one batch, exactly as the full scan: a delete
/// candidate whose stored checksum matches a new candidate's hash is a rename in
/// place rather than a delete plus an add. A rename whose two ends land in
/// different debounce windows cannot be paired here and degrades to a delete plus
/// an add - index-correct, but the rename-in-place optimization is lost and,
/// because `replace_chunks` carries an embedding over only within one engram, the
/// new path's chunks re-embed. That degradation is acceptable: the watcher's full
/// fallback, the startup sync and manual sync all reconcile, so the targeted
/// front only has to be convergent, never perfect.
///
/// The result flows through [`apply_scan`] with the identical TOCTOU guards, so a
/// concurrent writer landing between the snapshot and the apply is deferred and
/// reconciled just as it is for a full scan.
pub async fn scan_paths(
    name: &str,
    root: &Path,
    stamps: HashMap<String, FileStamp>,
    paths: Vec<String>,
    chunk_params: &ChunkParams,
) -> DomainScan {
    let started = Instant::now();

    // The same layout the full walk asks: a targeted scan must index exactly
    // what a full scan would, never a file the walk would have skipped.
    let reflection = reflection_mode(root, stamps.keys());
    let layout = DomainLayout::at(root, reflection);

    let mut current: HashMap<String, Scanned> = HashMap::new();
    let mut deleted_paths: Vec<String> = Vec::new();
    let mut unreadable: Vec<(String, String)> = Vec::new();
    let mut assets: Vec<FileCandidate> = Vec::new();
    let mut asset_deletes: Vec<String> = Vec::new();
    let mut reflections: Vec<FileCandidate> = Vec::new();
    let mut reflection_deletes: Vec<String> = Vec::new();
    let mut reflection_assets: Vec<FileCandidate> = Vec::new();
    let mut reflection_asset_deletes: Vec<String> = Vec::new();
    let mut threads: Vec<ReflectionPathChange> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for rel in paths {
        // The same path can arrive twice in one debounce batch; classify it once.
        if !seen.insert(rel.clone()) {
            continue;
        }
        let abs = root.join(&rel);
        let class = layout.classify(&rel);
        // The Reflection's files are never engrams or engram attachments. A
        // page or the pact present on disk is an upsert of its reflection
        // row, one that is gone a delete; threads, the baseline and stray
        // files are never rows. `symlink_metadata` for the reason the asset
        // branch below gives: the full walk never follows a link.
        if let PathClass::Reflection(file) = &class {
            // A Reflection file, the engram asset branch below rooted at the
            // Reflection folder: rows carry the canonical spelling, and on a
            // case-sensitive filesystem a canonical path that still resolves
            // is a different file that is still there.
            if let ReflectionFile::Asset(canonical) = file {
                let (folder, inner) = rel.split_once('/').unwrap_or((rel.as_str(), ""));
                let fname = inner.rsplit('/').next().unwrap_or(inner);
                match std::fs::symlink_metadata(&abs) {
                    Ok(meta) if meta.is_file() => {
                        if let Some(candidate) = asset_candidate(
                            &root.join(folder),
                            inner,
                            &abs,
                            fname,
                            file_mtime(&meta),
                            meta.len(),
                        ) {
                            reflection_assets.push(candidate);
                        }
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        if canonical.as_str() == inner
                            || !root.join(folder).join(canonical).exists()
                        {
                            reflection_asset_deletes.push(canonical.clone());
                        }
                    }
                    Err(e) => unreadable.push((rel.clone(), e.to_string())),
                }
                continue;
            }
            // A threads file is no row, but the page it belongs to changed
            // for whoever reads it: named by the page, with the checksum of
            // what is on disk now. Threads that went with their page are that
            // page's deletion, which the page's own path reports.
            if let ReflectionFile::Threads { of } = file {
                // A regular file only, as the walk reads: a link is not
                // followed and a pipe is never opened.
                let read = match std::fs::symlink_metadata(&abs) {
                    Ok(meta) if meta.is_file() => std::fs::read(&abs),
                    Ok(_) => continue,
                    Err(e) => Err(e),
                };
                match read {
                    Ok(bytes) => threads.push(ReflectionPathChange {
                        path: of.clone(),
                        kind: ReflectionPathKind::Threads,
                        checksum: Some(sha256_hex(&bytes)),
                    }),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        let page = abs.with_file_name(of.rsplit('/').next().unwrap_or(of.as_str()));
                        if page.exists() {
                            threads.push(ReflectionPathChange {
                                path: of.clone(),
                                kind: ReflectionPathKind::Threads,
                                checksum: None,
                            });
                        }
                    }
                    // A directory or a denied read: nothing to say about it.
                    Err(_) => {}
                }
                continue;
            }
            if matches!(file, ReflectionFile::Page(_) | ReflectionFile::Pact) {
                let inner = inner_rel(&rel);
                match std::fs::symlink_metadata(&abs) {
                    Ok(meta) if meta.is_file() => reflections.push(FileCandidate {
                        rel: inner,
                        abs: abs.clone(),
                        mtime: file_mtime(&meta),
                        size: meta.len(),
                    }),
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        reflection_deletes.push(inner)
                    }
                    Err(e) => unreadable.push((rel, e.to_string())),
                }
            }
            continue;
        }
        // The reserved `assets/` prefix: an attachable file present on disk is
        // an upsert when the layout calls it an attachment (the walk never
        // indexes a hidden, reserved or artifact path), a path under the
        // prefix that is gone is a delete whatever its class (a row indexed
        // before the MANIFEST declared an artifact folder around it goes at
        // once), and nothing under the prefix is ever an engram.
        if crystalline_core::is_under_assets(&rel) {
            // `symlink_metadata`, not `metadata`: the complete walk uses
            // `WalkDir`'s default `follow_links(false)` and indexes nothing
            // through a symlink, so following one here would give a symlinked
            // attachment a row that the next full scan takes away again. The
            // link is neither a file nor a delete candidate, so it falls to the
            // `Ok(_)` arm and is left alone.
            match std::fs::symlink_metadata(&abs) {
                Ok(meta) if meta.is_file() && class == PathClass::EngramAsset => {
                    let fname = rel.rsplit('/').next().unwrap_or(rel.as_str());
                    let mtime = file_mtime(&meta);
                    if let Some(candidate) =
                        asset_candidate(root, &rel, &abs, fname, mtime, meta.len())
                    {
                        assets.push(candidate);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Rows carry the folder's canonical spelling, so a delete
                    // has to name that key. If the canonical path still
                    // resolves, this is a case-sensitive filesystem where the
                    // two spellings are different files and the recorded one is
                    // still there, so nothing is deleted.
                    let canonical =
                        crystalline_core::canonical_asset_path(&rel).unwrap_or_else(|| rel.clone());
                    if canonical == rel || !root.join(&canonical).exists() {
                        asset_deletes.push(canonical);
                    }
                }
                // Unreadable rather than gone: leave the row alone, exactly as
                // the engram branch does, and let the next pass retry it.
                Err(e) => unreadable.push((rel, e.to_string())),
            }
            continue;
        }
        match std::fs::metadata(&abs) {
            // An existing markdown file, filtered exactly as the walk filters
            // (not hidden, not an OKF reserved name, not in an artifact
            // folder): a change candidate, prefiltered against the stamps
            // downstream.
            Ok(meta) if meta.is_file() && class == PathClass::Engram => {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let size = meta.len();
                current.insert(
                    rel.clone(),
                    Scanned {
                        rel,
                        abs,
                        mtime,
                        size,
                    },
                );
            }
            // Exists but is not an indexable markdown file (a directory, a
            // non-markdown file, a hidden, reserved or artifact path): ignore
            // it, matching the walk which never indexes it either.
            Ok(_) => {}
            // Not found on disk: a recorded path is a delete candidate (scoped
            // to the given paths, no walk), an unrecorded one is a no-op.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if stamps.contains_key(&rel) {
                    deleted_paths.push(rel);
                }
            }
            // Any other metadata error (a denied parent, an io fault) means the
            // path is unreadable, not gone: reporting it as failed keeps the row
            // instead of dropping an engram that is still there but momentarily
            // unreadable, the targeted counterpart of the walk-root guard.
            Err(e) => unreadable.push((rel, e.to_string())),
        }
    }

    let mut scan = classify_changes(
        name,
        root,
        stamps,
        current,
        deleted_paths,
        unreadable,
        chunk_params,
        started,
        false,
    )
    .await;
    // A targeted pass saw only the given paths, so the apply reconciles exactly
    // those rows and never deletes an attachment it was not asked about.
    scan.assets = assets;
    scan.asset_deletes = asset_deletes;
    scan.reflections = reflections;
    scan.reflection_deletes = reflection_deletes;
    scan.reflection_assets = reflection_assets;
    scan.reflection_asset_deletes = reflection_asset_deletes;
    scan.reflection = reflection;
    scan.report.reflection_changes = threads;
    scan
}

/// The classification core shared by [`scan_domain`] and [`scan_paths`].
///
/// Given the markdown files found on disk (`current`, keyed by relative path) and
/// the recorded paths whose file is gone (`deleted_paths`), it prefilters against
/// `stamps`, hashes the survivors, detects moves within this batch (a vanished
/// path whose stored checksum matches a new file's hash is a rename, not a delete
/// plus an add) and assembles the [`DomainScan`]. The walk front and the path-list
/// front differ only in how they build `current` and `deleted_paths`; everything
/// from here is identical, so the two stay in step by construction.
/// `report.unchanged` counts only paths actually examined - the whole domain for a
/// walk, only the given paths for a targeted scan - because it is only ever
/// bumped for an entry in `current`.
///
/// Move detection and the delete set are whole-batch here, before a single row is
/// written, which is what lets the apply slab the changed files afterwards
/// without changing the end state.
#[allow(clippy::too_many_arguments)]
async fn classify_changes(
    name: &str,
    root: &Path,
    stamps: HashMap<String, FileStamp>,
    current: HashMap<String, Scanned>,
    deleted_paths: Vec<String>,
    unreadable: Vec<(String, String)>,
    chunk_params: &ChunkParams,
    started: Instant,
    force: bool,
) -> DomainScan {
    // Prefilter: unchanged files (same mtime and size) are skipped entirely.
    let mut report = SyncReport {
        domain: name.to_string(),
        // Paths that could not be stat'd (a denied parent, an io fault) are
        // failures the caller already collected: fold them in up front, then
        // the hashing phase and the apply's slabs append their own read and
        // parse failures to the same list.
        failed: unreadable,
        ..SyncReport::default()
    };
    let mut to_hash: Vec<Scanned> = Vec::new();
    for (rel, scanned) in &current {
        match stamps.get(rel) {
            // A forced pass skips the prefilter entirely: the whole point is to
            // catch a file whose content moved without its stamp moving with it.
            Some(stamp) if !force && stamp.mtime == scanned.mtime && stamp.size == scanned.size => {
                report.unchanged += 1;
            }
            _ => to_hash.push(Scanned {
                rel: scanned.rel.clone(),
                abs: scanned.abs.clone(),
                mtime: scanned.mtime,
                size: scanned.size,
            }),
        }
    }

    // Hash the survivors off-thread with bounded concurrency. Only the checksum
    // comes back: classification needs nothing else, and holding every changed
    // file's contents here is exactly the peak the slabbed apply removes.
    let hashed = hash_files(to_hash, &mut report).await;

    // Index deleted files by checksum for move detection.
    let mut deleted_by_hash: HashMap<String, Vec<String>> = HashMap::new();
    for p in &deleted_paths {
        if let Some(stamp) = stamps.get(p) {
            deleted_by_hash
                .entry(stamp.sha256.clone())
                .or_default()
                .push(p.clone());
        }
    }
    let mut deleted_remaining: HashSet<String> = deleted_paths.iter().cloned().collect();

    // Classify each hashed file. The bool records whether the engram was
    // already indexed, so the apply phase can tell added from updated.
    let mut moves: Vec<(String, String)> = Vec::new();
    let mut changed: Vec<PendingChange> = Vec::new();
    for (scanned, sha256) in hashed {
        let is_new = !stamps.contains_key(&scanned.rel);
        if is_new {
            // A new file whose checksum matches a vanished file is a move.
            if let Some(candidates) = deleted_by_hash.get_mut(&sha256)
                && let Some(from) = candidates
                    .iter()
                    .find(|p| deleted_remaining.contains(*p))
                    .cloned()
            {
                deleted_remaining.remove(&from);
                moves.push((from, scanned.rel.clone()));
                continue;
            }
            changed.push(PendingChange {
                scanned,
                previously_indexed: false,
            });
        } else {
            let stamp = stamps.get(&scanned.rel);
            let same = stamp.map(|s| s.sha256 == sha256).unwrap_or(false);
            if same && !force {
                // Touched but identical content: nothing to reindex. A forced
                // pass re-upserts it anyway, which is what backfills a column a
                // migration added, and costs no embedding work: the chunk text
                // is unchanged, so `replace_chunks` carries every vector over.
                report.unchanged += 1;
            } else {
                changed.push(PendingChange {
                    scanned,
                    previously_indexed: true,
                });
            }
        }
    }

    DomainScan {
        moves,
        deletes: deleted_remaining,
        changed,
        chunk_params: chunk_params.clone(),
        snapshot: stamps,
        root: root.to_path_buf(),
        // The attachment side is classified by the caller, which knows whether
        // it walked the whole domain or only a list of paths.
        assets: Vec::new(),
        asset_deletes: Vec::new(),
        assets_complete: false,
        reflections: Vec::new(),
        reflection_deletes: Vec::new(),
        reflection_assets: Vec::new(),
        reflection_asset_deletes: Vec::new(),
        reflections_complete: false,
        accept_disk: false,
        walker: None,
        reflection: ReflectionMode::Off,
        // The caller decides: only a forced full scan ends a rebuild.
        ends_rebuild: false,
        report,
        started,
    }
}

/// Apply a [`DomainScan`] to the store in one transaction: moves, deletes,
/// upserts with their chunks, forward-reference resolution and the sync stamp.
///
/// The whole batch commits together. Duplicate-permalink upserts are collected in
/// `failed` and do not abort the batch (they are pre-checked so no failing
/// statement runs); any other error rolls the batch back.
///
/// The changed files are read, parsed and upserted in slabs of
/// `SYNC_SLAB_FILES`, so the apply of a huge domain holds one slab of contents
/// at a time - see the module-level memory note. Use [`apply_scan_with_slab`] to
/// pick another slab size.
///
/// A concurrent writer can move the index between the scan's snapshot and this
/// apply. The apply re-reads the live stamps once, inside the transaction, and
/// defers any classified change whose live db stamp no longer matches the
/// snapshot it was classified against - see the module-level convergence note.
pub async fn apply_scan<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    scan: DomainScan,
) -> Result<SyncReport> {
    apply_scan_with_slab(store, domain, scan, SYNC_SLAB_FILES).await
}

/// [`apply_scan`] with an explicit slab size, for tests that want to exercise the
/// slab boundaries on a handful of files. `slab_files` only changes how the work
/// is cut up, never the end state; production uses `SYNC_SLAB_FILES`.
pub async fn apply_scan_with_slab<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    scan: DomainScan,
    slab_files: usize,
) -> Result<SyncReport> {
    let DomainScan {
        moves,
        deletes,
        changed,
        chunk_params,
        snapshot,
        root,
        assets,
        asset_deletes,
        assets_complete,
        reflections,
        reflection_deletes,
        reflection_assets,
        reflection_asset_deletes,
        reflections_complete,
        accept_disk,
        walker,
        reflection,
        ends_rebuild,
        mut report,
        started,
    } = scan;

    store.begin().await?;
    // A walker that lost the domain's host lock while it walked writes
    // nothing: checked inside this transaction, before any row.
    if let Some(walker) = walker.as_deref()
        && store.shares_database()
    {
        let holder = match store.domain_host_held(domain).await {
            Ok(holder) => holder,
            Err(e) => {
                let _ = store.rollback().await;
                return Err(e);
            }
        };
        if holder.as_ref().map(|h| h.instance_id.as_str()) != Some(walker) {
            let _ = store.rollback().await;
            let by = holder.map_or_else(
                || "no instance holds its lock now".to_string(),
                |h| format!("instance {} hosts it now", h.label),
            );
            return Err(IndexError::NotHost(format!(
                "the walk of '{}' was not applied: {by}",
                report.domain
            )));
        }
    }
    // A complete walk (only a full walk sees every attachment) of a domain
    // that moved to this instance on a shared database keeps every row it
    // does not see while the move is unresolved: this disk may lack files
    // the previous host wrote. The move is read here, inside the apply's
    // own transaction, so a second walk that started while it was set
    // keeps those rows too, and it is cleared only below, in the same
    // transaction, by a walk that saw every row or was told to accept the
    // disk.
    // The move's text as stored: the write below compares against exactly
    // this, so a mark in any shape matches the read of it.
    let read_text = if assets_complete && store.shares_database() {
        match store.host_move_text(domain).await {
            Ok(text) => text,
            Err(e) => {
                let _ = store.rollback().await;
                return Err(e);
            }
        }
    } else {
        None
    };
    let host_move = read_text.as_deref().and_then(crate::store::parse_host_move);
    // Accepting the disk drops only rows a walk already reported: a move
    // marked since the last walk (a take-over sync run with --accept-disk)
    // is counted and reported first, and the next --accept-disk drops them.
    // Only the instance whose claim marked the move ends it; a walk by
    // another, or by one that named no walker, keeps and counts.
    let claimant = host_move
        .as_ref()
        .is_none_or(|m| m.claimant.is_none() || m.claimant.as_deref() == walker.as_deref());
    let accepted = claimant && accept_disk && host_move.as_ref().is_some_and(|m| m.walked);
    let keep_unseen = host_move.is_some() && !accepted;
    #[cfg(test)]
    if let Some(walker) = walker.as_deref() {
        apply_seam::hold(walker).await;
    }
    let mut elsewhere = crate::OnlyElsewhere::default();
    // Whether this apply changed a row, so the sync stamp is written only
    // then: a sync that found nothing new writes nothing at all.
    let mut wrote = false;
    let apply = apply_changes(
        store,
        domain,
        moves,
        deletes,
        changed,
        &chunk_params,
        &snapshot,
        &root,
        reflection,
        keep_unseen,
        &mut elsewhere.engrams,
        &mut report,
        slab_files.max(1),
    )
    .await;
    if let Err(e) = apply {
        let _ = store.rollback().await;
        return Err(e);
    }

    match reconcile_rows(
        store,
        domain,
        FileRows::Assets(AssetTable::Engram),
        assets,
        asset_deletes,
        Completeness::of(assets_complete, keep_unseen),
        &chunk_params,
        false,
        false,
        &mut report,
    )
    .await
    {
        Ok(reconciled) => {
            wrote |= reconciled.changed > 0;
            elsewhere.attachments = reconciled.unseen;
        }
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    }

    match reconcile_rows(
        store,
        domain,
        FileRows::Reflections,
        reflections,
        reflection_deletes,
        Completeness::of(reflections_complete, keep_unseen),
        &chunk_params,
        ends_rebuild,
        reflection == ReflectionMode::Effective,
        &mut report,
    )
    .await
    {
        Ok(reconciled) => {
            report.reflections_changed += reconciled.changed;
            elsewhere.pages = reconciled.unseen;
        }
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    }

    // The Reflection's files, through the code the attachment rows use and
    // counted with the pages, so a file a plain editor or a pull drops into
    // `reflections/assets/` marks the backup like a page does.
    match reconcile_rows(
        store,
        domain,
        FileRows::Assets(AssetTable::Reflection),
        reflection_assets,
        reflection_asset_deletes,
        Completeness::of(reflections_complete, keep_unseen),
        &chunk_params,
        false,
        reflection == ReflectionMode::Effective,
        &mut report,
    )
    .await
    {
        Ok(reconciled) => {
            report.reflections_changed += reconciled.changed;
            elsewhere.files = reconciled.unseen;
        }
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    }

    // The move ends with a walk that saw every row (a person copied the
    // folder over) or one told to accept the disk; otherwise it stays, with
    // what this walk did not see. A walk that could not read all of the
    // Reflection's folder does not know what it missed there, so it ends
    // nothing.
    if let Some(mut host_move) = host_move {
        elsewhere.previous_host = host_move.only_elsewhere.previous_host.take();
        let resolved = accepted || (claimant && elsewhere.total() == 0 && reflections_complete);
        // Only the claimant's walk writes the move: another walk keeps and
        // counts, and leaves the mark as the claimant set it.
        if claimant {
            let next = if resolved {
                None
            } else {
                host_move.walked = true;
                host_move.only_elsewhere = elsewhere.clone();
                Some(host_move)
            };
            match store
                .swap_host_move(domain, read_text.as_deref(), next.as_ref())
                .await
            {
                Ok(true) => wrote = true,
                Ok(false) => {
                    let _ = store.rollback().await;
                    return Err(IndexError::HostMoveChanged(format!(
                        "the walk of '{}' was not applied: another walk changed its host move meanwhile; the next sync applies",
                        report.domain
                    )));
                }
                Err(e) => {
                    let _ = store.rollback().await;
                    return Err(e);
                }
            }
        }
        if !resolved && elsewhere.total() > 0 {
            report.only_elsewhere = Some(elsewhere);
        }
        report.host_move_resolved = resolved;
    }

    let resolved = match store.resolve_pending_relations(domain).await {
        Ok(n) => n,
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    };
    report.relations_resolved = resolved;

    let links_resolved = match store.resolve_pending_links(domain).await {
        Ok(n) => n,
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    };
    report.links_resolved = links_resolved;

    // Refresh the derived tag-alias map from the (now-current) MANIFEST, inside
    // the same transaction. Unconditional, which is what delivers
    // populate-on-next-sync: a domain that has never synced under this feature
    // gains its aliases on its first sync, and a removed section clears them.
    // A map that already says what the MANIFEST says is not written.
    match refresh_tag_aliases(store, domain).await {
        Ok(replaced) => wrote |= replaced,
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    }

    // The rebuild this scan was the disk half of is complete: clear its marker
    // in the same transaction that commits its rows, so the marker is set
    // exactly while the rebuild is unfinished and a run that died before this
    // point leaves it standing.
    if ends_rebuild && let Err(e) = store.end_rebuild(domain).await {
        let _ = store.rollback().await;
        return Err(e);
    }
    // Every file was re-parsed, so the domain's rows are this parser's: the
    // stamp commits with them, and a run that died before here leaves the old
    // generation for the next sync to repeat.
    // A file the walk or the read could not reach kept the rows an older
    // parser derived, so the domain is not stamped until a run reads it.
    if ends_rebuild
        && report.unread == 0
        && let Err(e) = store.set_parse_generation(domain, PARSE_GENERATION).await
    {
        let _ = store.rollback().await;
        return Err(e);
    }

    // A sync that changed nothing leaves the stamp as it is, unless the
    // domain has none yet or it is older than the refresh interval, so an
    // idle domain synced on every watcher flush writes it at most once per
    // interval; see `Store::record_sync`.
    wrote |= !report.changes.is_empty()
        || report.added + report.updated + report.deleted + report.moved > 0
        || report.reflections_changed > 0
        || report.relations_resolved + report.links_resolved > 0
        || ends_rebuild;
    let now = chrono::Utc::now();
    let refresh_before = (now - SYNC_STAMP_REFRESH).to_rfc3339();
    if let Err(e) = store
        .record_sync(domain, &now.to_rfc3339(), wrote, &refresh_before)
        .await
    {
        let _ = store.rollback().await;
        return Err(e);
    }
    store.commit().await?;

    report.duration_ms = duration_ms(started.elapsed());
    Ok(report)
}

/// How old a domain's sync stamp may grow before a sync that changed nothing
/// writes it again: one hour. A sync that changed nothing writes nothing else,
/// so this bounds an idle domain to one small write per hour, and keeps the
/// stamp a reader without the engine's memory sees (a daemonless `status`, a
/// domain another instance hosts) at most this stale after a sync.
pub const SYNC_STAMP_REFRESH: chrono::TimeDelta = chrono::TimeDelta::hours(1);

/// A hold inside the apply, after the walker check and the read of the host
/// move and before any row is written, for the tests that land a claim or a
/// mark write in that window. Keyed by walker, so tests side by side never
/// meet it. A reindex also holds under `reindex:<domain>` once its first
/// window (the claim and the rebuild marker) is done, before the walk.
#[cfg(test)]
pub(crate) mod apply_seam {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

    type Gate = (Arc<Notify>, Arc<Notify>);
    static HOLDS: Mutex<Option<HashMap<String, Gate>>> = Mutex::new(None);

    /// Arm the hold for the next apply walked by `walker`: it signals the
    /// first `Notify` there and waits for the second.
    // Only the postgres-gated host tests arm a hold.
    #[cfg(feature = "postgres")]
    pub(crate) fn arm(walker: &str) -> Gate {
        let gate = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        HOLDS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(walker.to_string(), gate.clone());
        gate
    }

    pub(super) async fn hold(walker: &str) {
        let gate = HOLDS
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|h| h.remove(walker));
        if let Some((entered, release)) = gate {
            entered.notify_one();
            release.notified().await;
        }
    }
}

/// Per-domain callbacks a multi-domain driver runs for its caller.
///
/// [`reindex_domains`] owns the loop, and the two callers of it need different
/// things around each domain: the daemon claims the file-host lock before it
/// touches a domain in collaboration mode and refreshes the generated index
/// files after a domain changed, while the daemonless CLI does neither. Both
/// hooks default to doing nothing, so a caller with no per-domain business
/// passes [`NoReindexHooks`] and reads the driver as a plain loop.
///
/// The `before_domain` hook runs with the store lock held and is handed the
/// locked store, because the claim it exists for is itself a store write that
/// belongs in the same window as the domain's upsert; the tokio mutex is not
/// reentrant, so a hook must never try to take the lock itself. `after_apply`
/// runs with no lock held, after the domain's transaction has committed.
#[async_trait::async_trait]
pub trait ReindexHooks: Send + Sync {
    /// Called in the first lock window of each domain, before its row is
    /// resolved. Returning `false` skips the domain entirely: it is neither
    /// scanned nor applied, and it is not handed to the final cross-domain
    /// resolution pass.
    async fn before_domain(&self, _store: &dyn Store, _name: &str, _root: &Path) -> Result<bool> {
        Ok(true)
    }

    /// Called after a domain's apply committed, with no store lock held.
    async fn after_apply(&self, _name: &str, _report: &SyncReport) {}

    /// Called, with no store lock held, when a walk of `name` was not
    /// applied; `why` says why. `host_lost` when another instance took the
    /// domain over while it ran ([`IndexError::NotHost`]), not when another
    /// walk only changed its host move ([`IndexError::HostMoveChanged`]).
    /// The run goes on.
    async fn not_applied(&self, _name: &str, _why: &str, _host_lost: bool) {}

    /// The instance that claims the domains this run walks, when it claims
    /// any: each walk is applied only while the claim holds
    /// ([`DomainScan::walked_by`]).
    fn walker(&self) -> Option<String> {
        None
    }
}

/// The do-nothing [`ReindexHooks`]: every domain is claimed by nobody and
/// nothing follows an apply. What a daemonless reindex passes.
pub struct NoReindexHooks;

#[async_trait::async_trait]
impl ReindexHooks for NoReindexHooks {}

/// Reindex a list of file domains through one loop, the shared driver behind
/// both `crystalline reindex` and the daemon's `ctl reindex`.
///
/// `force` is `reindex --full`: every file is re-read, re-parsed and
/// re-upserted rather than prefiltered against its recorded stamp, so a file
/// whose content changed without its modification time or size moving is
/// rewritten too, and a column a migration added is backfilled on every row.
/// Nothing is cleared first, on either path. A domain keeps its previous
/// complete rows until its own rebuild commits, so an interruption anywhere
/// leaves a valid index rather than an empty one, stale rows are pruned by the
/// scan's own delete detection exactly as a plain sync prunes them, and every
/// chunk whose text is unchanged keeps the embedding it already had instead of
/// being re-embedded from scratch.
///
/// The shape per domain is the two-lock-window one the rest of the sync engine
/// uses - resolve the domain and snapshot its stamps under the lock, walk and
/// hash with no lock held, apply transactionally in a second window - so a
/// large domain's rebuild never blocks concurrent readers behind the mutex.
/// `rebuild` says which verb is running and whether this is a forced run at
/// all: `None` is an ordinary incremental pass, `Some` forces every file to be
/// re-read and also stamps each domain's rebuild marker in the first window,
/// which the apply's own transaction clears, so a rebuild that never finished
/// says so afterwards instead of reporting its rows as freshly rebuilt ones.
/// The marker carries the verb with it, because an interrupted
/// [`RebuildKind::Full`] left the complete rows from before it while an
/// interrupted [`RebuildKind::Wipe`] destroyed them and every embedding before
/// it began, and a reader must not describe one as the other.
///
/// The run ends with [`resolve_forward_refs`] over the domains it applied and
/// one [`Store::checkpoint_wal`], for both callers: a reindex is a
/// snapshot-preparation verb whichever process runs it.
pub async fn reindex_domains(
    store: &tokio::sync::Mutex<dyn Store>,
    targets: &[(String, PathBuf)],
    chunk_params: &ChunkParams,
    rebuild: Option<RebuildKind>,
    hooks: &dyn ReindexHooks,
) -> Result<Vec<SyncReport>> {
    let force = rebuild.is_some();
    // Each domain this run applied, paired with the report its apply produced,
    // for the final cross-domain resolution pass. A domain a hook skipped wrote
    // nothing and is not in the list at all.
    let mut applied: Vec<(DomainId, SyncReport)> = Vec::new();

    for (name, root) in targets {
        let Some((domain, snapshot, force_domain, stamped)) = ({
            let store = store.lock().await;
            let claimed = hooks
                .before_domain(&*store, name, root)
                .await
                .map_err(|e| in_domain("reindex", name, e))?;
            if claimed {
                let domain = store
                    .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
                    .await
                    .map_err(|e| in_domain("reindex", name, e))?;
                let stamped = match rebuild {
                    Some(kind) => {
                        let now = chrono::Utc::now().to_rfc3339();
                        store
                            .begin_rebuild(domain, &now, kind)
                            .await
                            .map_err(|e| in_domain("reindex", name, e))?;
                        Some(now)
                    }
                    None => None,
                };
                // The real stamps, not an empty map: delete detection is the
                // recorded paths absent from the walk, so a forced run that
                // cleared first could not prune anything at all - it would see
                // nothing recorded and call every file on disk new.
                let snapshot = store
                    .file_stamps(domain)
                    .await
                    .map_err(|e| in_domain("reindex", name, e))?;
                // A domain an older parser derived is walked whole even on an
                // incremental reindex, once, as a sync would walk it.
                let due = reparse_due(&*store, domain)
                    .await
                    .map_err(|e| in_domain("reindex", name, e))?;
                Some((domain, snapshot, force || due, stamped))
            } else {
                None
            }
        }) else {
            continue;
        };
        // The marker is stamped and the lock is released: this is the state a
        // reader sees for most of a rebuild, and the only state in which the
        // marker is observable at all. Held open on request so a test can
        // observe it instead of racing it - see `rebuild_hold`.
        if force && let Some(hold) = rebuild_hold() {
            tokio::time::sleep(hold).await;
        }
        #[cfg(test)]
        apply_seam::hold(&format!("reindex:{name}")).await;
        let mut scan = scan_domain(name, root, snapshot, chunk_params, force_domain)
            .await
            .map_err(|e| in_domain("reindex", name, e))?;
        if let Some(walker) = hooks.walker() {
            scan.walked_by(&walker);
        }
        let report = {
            let store = store.lock().await;
            match apply_scan(&*store, domain, scan).await {
                Ok(report) => report,
                // Another instance took the domain over while this run walked
                // it, or another walk changed its host move: nothing of the
                // walk was written, the run goes on with the next domain, and
                // the rebuild marker this run stamped is cleared unless a run
                // stamped a newer one meanwhile.
                Err(e @ (IndexError::NotHost(_) | IndexError::HostMoveChanged(_))) => {
                    if let Some(started) = &stamped {
                        store
                            .end_rebuild_stamped(domain, started)
                            .await
                            .map_err(|e| in_domain("reindex", name, e))?;
                    }
                    drop(store);
                    let host_lost = matches!(e, IndexError::NotHost(_));
                    hooks.not_applied(name, &e.to_string(), host_lost).await;
                    continue;
                }
                Err(e) => return Err(in_domain("reindex", name, e)),
            }
        };
        hooks.after_apply(name, &report).await;
        applied.push((domain, report));
    }

    // A reindex rebuilds every domain in one run, so it has exactly the
    // forward-reference problem a full sweep has: the first domain is applied
    // while the last one holds none of its targets yet.
    let store = store.lock().await;
    resolve_forward_refs(&*store, &mut applied)
        .await
        .map_err(|e| IndexError::Db(format!("resolving forward references failed: {e}")))?;
    // Any reindex, full or incremental, is a snapshot-preparation verb: a
    // downstream pipeline may ship index.db as a single file (sidecars
    // deleted), so whatever this run just wrote must not sit stranded in the
    // WAL. Merge and shrink it now rather than leaving it to grow until the
    // next natural checkpoint. A no-op on Postgres (no local WAL file).
    //
    // Best effort, as the trait documents it: every domain has committed by
    // now, so a checkpoint that could not truncate is a housekeeping miss and
    // not a reason to report a completed rebuild as failed. It is also new work
    // for the daemon, which never checkpointed here at all, and a new failure
    // mode is not what moving it into the shared driver was for.
    if let Err(e) = store.checkpoint_wal().await {
        tracing::debug!("reindex: the tail WAL checkpoint did not run: {e}");
    }
    Ok(applied.into_iter().map(|(_, report)| report).collect())
}

/// How long a forced rebuild holds between stamping a domain's marker and
/// walking its files, from `CRYSTALLINE_TEST_REBUILD_HOLD_MS`, or `None`.
///
/// A test-only seam, named `TEST` for the same reason
/// `CRYSTALLINE_TEST_POSTGRES_URL` is: it is not a knob an install is meant to
/// set and nothing documents it as one. It exists because the window this
/// opens - the marker stamped, the store lock released, the walk not yet
/// finished - is the whole point of the design and is invisible from outside
/// the process that is rebuilding. The test that proves a daemon keeps
/// answering during a rebuild, and that `ctl status` shows the marker while it
/// does, has to observe that window from a third process; without a hold it
/// samples a window that a small corpus can close inside one polling lap, and
/// a test that observes a race is a test that flakes.
///
/// Read once per process and cached, so the ordinary path pays one relaxed
/// load and no environment lookup per domain. An unset, empty or unparsable
/// value is `None`, so a malformed knob holds nothing rather than failing a
/// rebuild, and a hold only ever happens under `force`, where a marker exists
/// to be observed.
fn rebuild_hold() -> Option<Duration> {
    static HOLD: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *HOLD.get_or_init(|| {
        std::env::var("CRYSTALLINE_TEST_REBUILD_HOLD_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis)
    })
}

/// Name the domain a multi-domain run failed in, keeping the failure itself as
/// the source.
fn in_domain(operation: &str, domain: &str, source: IndexError) -> IndexError {
    IndexError::InDomain {
        operation: operation.to_string(),
        domain: domain.to_string(),
        source: Box::new(source),
    }
}

/// The final cross-domain resolution pass of a multi-domain run: resolve every
/// forward reference that was still pending when its own domain finished,
/// because the domain it points into had not been indexed yet.
///
/// [`apply_scan`] resolves a domain's references as that domain commits, so a
/// reference into a domain later in the run cannot resolve there: the target
/// does not exist, and the target domain may have no row at all. Without this
/// pass such a reference stays unresolved until some later sync happens to
/// re-run the per-domain resolution, and until then a reader sees
/// `"resolved": false` on a link that is not broken. Running the same two store
/// methods once more, after every domain in the run is indexed, settles them in
/// the run that created them; order of registration stops mattering.
///
/// `applied` pairs each domain with the report its own apply produced: the
/// counts land in [`SyncReport::relations_resolved_late`] and
/// [`SyncReport::links_resolved_late`] of the report beside the domain they were
/// counted for, kept apart from the per-domain counters so the late pass is
/// visible rather than folded away. One structure rather than two parallel
/// slices, so a caller cannot drift the two apart and silently mis-attribute or
/// drop counts. Returns the run totals `(relations, links)`.
///
/// A run of fewer than two domains is a no-op: a single domain has already had
/// its references resolved against everything this run indexed, so a second
/// pass could only re-run the same statement over the same rows.
///
/// Callers are the multi-domain drivers - the daemon's sweep, `crystalline sync`
/// and `crystalline reindex` - and all of them pass only the domains they
/// actually applied, never one that was skipped or failed.
pub async fn resolve_forward_refs<S: Store + ?Sized>(
    store: &S,
    applied: &mut [(DomainId, SyncReport)],
) -> Result<(u64, u64)> {
    if applied.len() < 2 {
        return Ok((0, 0));
    }

    store.begin().await?;
    let pass = async {
        let mut counts = Vec::with_capacity(applied.len());
        for (domain, _) in applied.iter() {
            let relations = store.resolve_pending_relations(*domain).await?;
            let links = store.resolve_pending_links(*domain).await?;
            counts.push((relations, links));
        }
        Ok::<Vec<(u64, u64)>, IndexError>(counts)
    }
    .await;
    let counts = match pass {
        Ok(counts) => {
            store.commit().await?;
            counts
        }
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    };

    let mut total_relations = 0;
    let mut total_links = 0;
    for ((_, report), (relations, links)) in applied.iter_mut().zip(counts) {
        report.relations_resolved_late = relations;
        report.links_resolved_late = links;
        total_relations += relations;
        total_links += links;
    }
    if total_relations > 0 || total_links > 0 {
        tracing::info!(
            relations = total_relations,
            links = total_links,
            domains = applied.len(),
            "sync: resolved forward references across domains after the last domain was indexed"
        );
    }
    Ok((total_relations, total_links))
}

/// Reconcile one file table with what the walk found: the stat prefilter
/// skips a row whose size and modification instant match, a changed file is
/// read and its row replaced, and the rows the walk did not see are deleted.
/// Answers how many rows it wrote or deleted. One reconcile for every file
/// table (the engram attachments and the Reflection's rows), so a fix to it
/// reaches each.
///
/// The rows are not engrams, so this is deliberately not the engram pipeline:
/// there is no move detection (a row carries no permalink, so a rename is a
/// delete plus an add and costs one read). A reflection row is chunked with
/// `chunk_params` as it is written (R53); an asset row is not. What it does
/// share is the stat prefilter: a candidate whose recorded row still carries
/// the same size and modification instant cannot have changed content without
/// one of them moving, so it is skipped without reading a byte, unless `force`
/// is set.
///
/// A full walk reconciles domain-wide, deleting every row it did not see,
/// unless it keeps them for an unresolved host move and only counts them; a
/// targeted pass deletes only the paths it was asked about and looked for in
/// vain, so a one-file watcher event never touches another row.
///
/// With `announce`, each row of the Reflection's tables it writes or deletes
/// is named in `report.reflection_changes` for the change bus. The caller
/// passes it false for the rows a walk clears because the Reflection is not
/// in effect; the engine decides whether a walk that fills the table is the
/// switch coming on.
#[allow(clippy::too_many_arguments)]
async fn reconcile_rows<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    rows: FileRows,
    found: Vec<FileCandidate>,
    deletes: Vec<String>,
    completeness: Completeness,
    chunk_params: &ChunkParams,
    force: bool,
    announce: bool,
    report: &mut SyncReport,
) -> Result<Reconciled> {
    let complete = completeness != Completeness::Partial;
    if !complete && found.is_empty() && deletes.is_empty() {
        return Ok(Reconciled::default());
    }
    let existing = rows.stamps(store, domain).await?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut changed = 0;
    for candidate in found {
        let modified = rfc3339_from_mtime(candidate.mtime);
        seen.insert(candidate.rel.clone());
        // A forced walk re-reads pages, so `reindex --full` re-chunks them
        // (R58); the asset arms are never forced.
        if !force
            && existing
                .get(&candidate.rel)
                .is_some_and(|(size, at)| *size == candidate.size && *at == modified)
        {
            continue;
        }
        let read = {
            let candidate = candidate.clone();
            tokio::task::spawn_blocking(move || rows.build(&candidate, modified))
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())))
        };
        match read {
            Ok(Some(row)) => {
                rows.upsert(store, domain, &row, chunk_params).await?;
                changed += 1;
                if announce && let Some(kind) = rows.announced_as(false) {
                    report.reflection_changes.push(ReflectionPathChange {
                        path: candidate.rel.clone(),
                        kind,
                        checksum: Some(row.sha256().to_string()),
                    });
                }
            }
            Ok(None) => {}
            // The file vanished between the scan and here; its removal has its
            // own pass queued, so leave the row for that one.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %candidate.rel, "sync: skipping a file that vanished mid-sync");
            }
            Err(e) => report
                .failed
                .push((rows.report_path(&candidate.rel), e.to_string())),
        }
    }
    let gone: Vec<String> = match completeness {
        Completeness::Complete => existing.into_keys().filter(|p| !seen.contains(p)).collect(),
        Completeness::KeepUnseen => {
            let unseen = existing.keys().filter(|p| !seen.contains(*p)).count();
            return Ok(Reconciled {
                changed,
                unseen: unseen as u64,
            });
        }
        Completeness::Partial => deletes,
    };
    for path in gone {
        if rows.delete(store, domain, &path).await? {
            changed += 1;
            if announce && let Some(kind) = rows.announced_as(true) {
                report.reflection_changes.push(ReflectionPathChange {
                    path,
                    kind,
                    checksum: None,
                });
            }
        }
    }
    Ok(Reconciled { changed, unseen: 0 })
}

/// How much of a file table a walk saw, for [`reconcile_rows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Completeness {
    /// A targeted pass: only the paths it names.
    Partial,
    /// A full walk: every row it did not see goes.
    Complete,
    /// A full walk of a domain whose host move is unresolved: every row it
    /// did not see stays, and is counted.
    KeepUnseen,
}

impl Completeness {
    fn of(complete: bool, keep_unseen: bool) -> Completeness {
        match (complete, keep_unseen) {
            (false, _) => Completeness::Partial,
            (true, false) => Completeness::Complete,
            (true, true) => Completeness::KeepUnseen,
        }
    }
}

/// What [`reconcile_rows`] did: the rows it wrote or deleted, and the rows
/// it kept unseen.
#[derive(Debug, Default)]
struct Reconciled {
    changed: usize,
    unseen: u64,
}

/// Refresh a domain's derived tag-alias rows from its MANIFEST. Reads the stored
/// `MANIFEST.md` content, folds its `## Tag Aliases` declarations to `(alias,
/// canonical)` pairs and replaces the domain's rows with them. A missing or
/// unparseable MANIFEST folds to no pairs, so the rows are cleared and a removed
/// section takes effect on the next sync. The store never parses markdown: the
/// pairs are folded here in the format layer and handed over already lowercased.
///
/// Answers whether it wrote: rows that already match the MANIFEST are left
/// alone.
pub async fn refresh_tag_aliases<S: Store + ?Sized>(store: &S, domain: DomainId) -> Result<bool> {
    let pairs = match store.engram_content(domain, "MANIFEST.md").await? {
        Some(content) => crystalline_core::tag_alias_pairs(&content),
        None => Vec::new(),
    };
    store.replace_tag_aliases(domain, &pairs).await
}

#[allow(clippy::too_many_arguments)]
async fn apply_changes<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    moves: Vec<(String, String)>,
    deletes: std::collections::HashSet<String>,
    changed: Vec<PendingChange>,
    chunk_params: &ChunkParams,
    snapshot: &HashMap<String, FileStamp>,
    root: &Path,
    reflection: ReflectionMode,
    keep_unseen: bool,
    unseen: &mut u64,
    report: &mut SyncReport,
    slab_files: usize,
) -> Result<()> {
    // The live stamps guard against a writer that moved the index between the
    // scan's snapshot and now. Read them once, inside the transaction and only
    // when there is something to apply, so the warm no-change pass adds no query.
    let live = if moves.is_empty() && deletes.is_empty() && changed.is_empty() {
        HashMap::new()
    } else {
        store.file_stamps(domain).await?
    };
    let domain_name = report.domain.clone();
    // The permalinks a subscriber keys its pages on, read in at most two
    // listings per apply rather than one domain scan per path: every row's
    // permalink before the moves and deletes, and once more after the moves,
    // since the store decides whether a permalink follows its path.
    let before = if moves.is_empty() && deletes.is_empty() {
        HashMap::new()
    } else {
        permalinks(store, &domain_name).await?
    };
    let mut renamed: Vec<usize> = Vec::new();

    for (from, to) in moves {
        // A move is a delete of `from` plus an add of `to`; if either end's db
        // stamp moved since the snapshot the classification is stale, so leave
        // both ends for the next pass rather than renaming over a fresh write.
        if live.get(&from) != snapshot.get(&from) || live.get(&to) != snapshot.get(&to) {
            report.deferred += 1;
            tracing::debug!(from = %from, to = %to, "sync: deferring a move whose db stamp moved mid-scan");
            continue;
        }
        store.rename_engram(domain, &from, &to).await?;
        renamed.push(report.changes.len());
        report.changes.push(PathChange {
            kind: PathChangeKind::Moved,
            path: to.clone(),
            from: Some(from.clone()),
            from_permalink: before.get(&from).cloned(),
            permalink: None,
            checksum: snapshot.get(&from).map(|stamp| stamp.sha256.clone()),
        });
        report.moved += 1;
    }
    if !renamed.is_empty() {
        let after = permalinks(store, &domain_name).await?;
        for index in renamed {
            let change = &mut report.changes[index];
            change.permalink = after.get(&change.path).cloned();
        }
    }
    for path in deletes {
        // The row was rewritten mid-scan: someone indexed newer state at this
        // path, so dropping it would discard their write.
        if live.get(&path) != snapshot.get(&path) {
            report.deferred += 1;
            tracing::debug!(path = %path, "sync: deferring a delete whose db stamp moved mid-scan");
            continue;
        }
        // The file vanished during the scan but is back on disk now; the watcher
        // event for that recreation is already queued, so leave the row for it.
        // A reserved name is the exception: a file called `index.md` or `log.md`
        // is never an engram, so its presence on disk must not resurrect a row
        // recorded before the exclusion existed. Nor is a file under
        // `reflections/` while the Reflection is in effect: a page indexed as
        // an engram while the switch was off goes when the switch takes effect.
        let never_engram = crystalline_core::is_reserved_path(&path)
            || (reflection == ReflectionMode::Effective
                && crystalline_core::reflection::is_under_reflections(&path));
        if !never_engram && root.join(&path).exists() {
            report.deferred += 1;
            tracing::debug!(path = %path, "sync: deferring a delete whose file reappeared on disk");
            continue;
        }
        // A domain whose host move is unresolved keeps the row: the file may
        // be on the previous host's disk only. A move above is applied as on
        // any walk, since its content was seen at the new path.
        if keep_unseen && !never_engram {
            *unseen += 1;
            continue;
        }
        store.delete_engram(domain, &path).await?;
        report.changes.push(PathChange {
            kind: PathChangeKind::Deleted,
            path: path.clone(),
            from: None,
            from_permalink: None,
            permalink: before.get(&path).cloned(),
            checksum: None,
        });
        report.deleted += 1;
    }
    // The changed files, slab by slab. The stale-stamp guard runs before a slab
    // is read, so a deferred path costs no read and no parse at all.
    let mut queue = changed;
    while !queue.is_empty() {
        let take = queue.len().min(slab_files);
        let mut slab: Vec<PendingChange> = queue.drain(..take).collect();
        slab.retain(|c| {
            let path = c.scanned.rel.as_str();
            // The db stamp for this path moved since the snapshot: a concurrent
            // writer indexed newer state, so this change is stale. Applying it
            // would clobber the newer state, so defer and let the next pass
            // reconcile.
            if live.get(path) != snapshot.get(path) {
                report.deferred += 1;
                tracing::debug!(path = %path, "sync: deferring a change whose db stamp moved mid-scan");
                return false;
            }
            true
        });
        parse_and_apply_slab(store, domain, slab, chunk_params, report).await?;
    }
    Ok(())
}

/// Every row's permalink in one domain, keyed by its exact path: one listing,
/// the same rows the engine's save receipt reads its permalink back from.
async fn permalinks<S: Store + ?Sized>(store: &S, domain: &str) -> Result<HashMap<String, String>> {
    Ok(store
        .list_engrams(domain, None, None)
        .await?
        .into_iter()
        .map(|row| (row.path, row.permalink))
        .collect())
}

/// Read, parse, chunk and upsert one slab of changed files.
///
/// Every file in the slab is read and parsed off-thread with the same bounded
/// concurrency the hashing phase uses, and each result is upserted the moment it
/// arrives, so the contents of at most one slab are ever live. Read and parse
/// failures are reported, not fatal; a file that vanished between the scan and
/// its slab is deferred, exactly as a change whose db stamp moved, because the
/// pass that removed it has its own event queued.
async fn parse_and_apply_slab<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    slab: Vec<PendingChange>,
    chunk_params: &ChunkParams,
    report: &mut SyncReport,
) -> Result<()> {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set: JoinSet<ParseOutcome> = JoinSet::new();
    // The relative path moves into its task (it lands in the `ParseOutcome`), so
    // a task that panics outright would lose it. Keep a task-id to path map so a
    // panicked task is still attributable in `failed`.
    let mut ids: HashMap<tokio::task::Id, String> = HashMap::new();
    for change in slab {
        let sem = sem.clone();
        let rel = change.scanned.rel.clone();
        // Chunking is two small fields, cloned per task so it moves into the
        // blocking closure alongside the read and the parse.
        let chunk_params = chunk_params.clone();
        let handle = set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore open");
            let PendingChange {
                scanned,
                previously_indexed,
            } = change;
            // Read, parse and chunk in one blocking task. The checksum is taken
            // from these very bytes, so the stored stamp always describes the
            // content that landed, exactly as when the read fed the hash phase.
            let parsed = tokio::task::spawn_blocking(move || {
                let bytes = match std::fs::read(&scanned.abs) {
                    Ok(b) => b,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return ParseOutcome::Vanished(scanned.rel);
                    }
                    Err(e) => return ParseOutcome::Unread(scanned.rel, e.to_string()),
                };
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                let sha256 = crate::hex_lower(&hasher.finalize());
                let Ok(content) = String::from_utf8(bytes) else {
                    return ParseOutcome::Failed(
                        scanned.rel,
                        "file is not valid UTF-8".to_string(),
                    );
                };
                let stamp = FileStamp {
                    mtime: scanned.mtime,
                    size: scanned.size,
                    sha256,
                };
                // The index holds LF only. The stamp above stays the one of
                // the bytes on disk, which a sync never rewrites: a CRLF file
                // is indexed as LF and turns LF on disk on its next write.
                let content = crystalline_core::to_lf(&content);
                match crystalline_core::parse_engram(&content) {
                    Ok(engram) => {
                        let record = EngramRecord::from_engram(&engram, &scanned.rel, stamp);
                        let chunks = chunk_engram(
                            &record.title,
                            record.description.as_deref(),
                            &record.content,
                            &chunk_params,
                        );
                        ParseOutcome::Ok(Box::new(record), chunks, previously_indexed)
                    }
                    Err(e) => ParseOutcome::Failed(scanned.rel, e.to_string()),
                }
            })
            .await;
            match parsed {
                Ok(outcome) => outcome,
                Err(e) => ParseOutcome::Unread(String::new(), e.to_string()),
            }
        });
        ids.insert(handle.id(), rel);
    }

    while let Some(joined) = set.join_next_with_id().await {
        match joined {
            Ok((id, ParseOutcome::Ok(record, chunks, previously_indexed))) => {
                ids.remove(&id);
                match store.upsert_engram(domain, &record).await {
                    Ok(engram_id) => {
                        // Apply the chunk rows computed in the same off-thread
                        // task, right after the upsert returns the id.
                        // replace_chunks keeps the embedding of any chunk whose
                        // fingerprint is unchanged, so an edit only re-embeds the
                        // paragraphs that changed; the fingerprint folds in only
                        // the model id and text, so where the chunks were computed
                        // changes nothing about the carry-over.
                        store.replace_chunks(engram_id, &chunks).await?;
                        report.changes.push(PathChange {
                            kind: if previously_indexed {
                                PathChangeKind::Modified
                            } else {
                                PathChangeKind::Added
                            },
                            path: record.path.clone(),
                            from: None,
                            from_permalink: None,
                            permalink: Some(record.permalink.clone()),
                            checksum: Some(record.stamp.sha256.clone()),
                        });
                        if previously_indexed {
                            report.updated += 1;
                        } else {
                            report.added += 1;
                        }
                    }
                    Err(IndexError::Constraint(msg)) => {
                        report.failed.push((record.path.clone(), msg));
                    }
                    Err(other) => return Err(other),
                }
            }
            Ok((id, ParseOutcome::Vanished(path))) => {
                ids.remove(&id);
                report.deferred += 1;
                tracing::debug!(path = %path, "sync: deferring a change whose file vanished mid-sync");
            }
            Ok((id, ParseOutcome::Failed(path, err))) => {
                ids.remove(&id);
                report.failed.push((path, err));
            }
            Ok((id, ParseOutcome::Unread(path, err))) => {
                // A blocking task that panicked outright loses the path with it,
                // so fall back to the task-id map to keep the failure attributable.
                let mapped = ids.remove(&id);
                let path = if path.is_empty() {
                    mapped.unwrap_or_else(|| "unknown".to_string())
                } else {
                    path
                };
                report.failed.push((path, err));
                report.unread += 1;
            }
            Err(join_err) => {
                let rel = ids
                    .remove(&join_err.id())
                    .unwrap_or_else(|| "unknown".to_string());
                report
                    .failed
                    .push((rel, format!("task panicked: {join_err}")));
                report.unread += 1;
            }
        }
    }
    Ok(())
}

async fn hash_files(files: Vec<Scanned>, report: &mut SyncReport) -> Vec<(Scanned, String)> {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set: JoinSet<(Scanned, std::io::Result<String>)> = JoinSet::new();
    // The file identity moves into its task, so a task that panics outright
    // would otherwise vanish without a trace. Keep a task-id to relative-path
    // map so a panicked task is still attributable in `failed`.
    let mut ids: HashMap<tokio::task::Id, String> = HashMap::new();
    for scanned in files {
        let sem = sem.clone();
        let rel = scanned.rel.clone();
        let handle = set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore open");
            let abs = scanned.abs.clone();
            let res = tokio::task::spawn_blocking(move || read_and_hash(&abs))
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
            (scanned, res)
        });
        ids.insert(handle.id(), rel);
    }
    let mut out = Vec::new();
    while let Some(joined) = set.join_next_with_id().await {
        match joined {
            Ok((id, (scanned, Ok(sha256)))) => {
                ids.remove(&id);
                out.push((scanned, sha256));
            }
            Ok((id, (scanned, Err(e)))) => {
                ids.remove(&id);
                // Unreadable file: report the failure and leave the path out of
                // the changed set, so its row (if any) stays untouched and the
                // next pass retries it.
                report.failed.push((scanned.rel, e.to_string()));
                report.unread += 1;
            }
            Err(join_err) => {
                let rel = ids
                    .remove(&join_err.id())
                    .unwrap_or_else(|| "unknown".to_string());
                report
                    .failed
                    .push((rel, format!("task panicked: {join_err}")));
                report.unread += 1;
            }
        }
    }
    out
}

/// Hash a candidate file, keeping nothing but its checksum: the contents are
/// dropped here and read again by the slab that parses the file, which is what
/// keeps a scan of a huge domain flat in memory.
fn read_and_hash(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(crate::hex_lower(&hasher.finalize()))
}

/// SHA-256 hex of `bytes`, the digest `read_and_hash` computes for a file.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    crate::hex_lower(&hasher.finalize())
}

/// Hash an attachment and report the length of the very bytes that were hashed,
/// so the stored row can never describe a size the checksum does not cover.
fn read_and_measure(path: &Path) -> std::io::Result<(String, u64)> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok((crate::hex_lower(&hasher.finalize()), bytes.len() as u64))
}

/// A file's modification time in whole seconds since the epoch, 0 when the
/// platform or the filesystem does not report one.
fn file_mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// An mtime in whole seconds as an RFC 3339 instant, the form an attachment row
/// records. A timestamp outside the representable range falls back to the epoch,
/// which keeps the prefilter comparing like with like rather than erroring.
fn rfc3339_from_mtime(mtime: i64) -> String {
    chrono::DateTime::from_timestamp(mtime, 0)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc3339()
}

/// Classify one path under the reserved `assets/` prefix. `None` means the file
/// is not attachable - a path rule, the extension allowlist or the size cap
/// refused it - and it is skipped without a row and without a failure.
fn asset_candidate(
    root: &Path,
    rel: &str,
    abs: &Path,
    fname: &str,
    mtime: i64,
    size: u64,
) -> Option<FileCandidate> {
    // A folder whose name differs from `assets` only in case is one and the
    // same directory on APFS and NTFS, so the bytes an upload wrote as
    // `assets/x.png` are walked back as `Assets/x.png`. Keying the row by the
    // canonical spelling is what stops the two writers from taking turns: the
    // scan would otherwise see no `assets/x.png` on disk and delete the row the
    // upload just wrote, and the next read would mint it again.
    //
    // The `exists` probe is what keeps that fold honest on a case-sensitive
    // filesystem, where `Assets` and `assets` really are different directories:
    // the canonical path does not resolve there, so the oddly cased folder is
    // left unindexed - reserved, exactly as the engram side already treats it.
    let rel = match crystalline_core::canonical_asset_path(rel) {
        Some(canonical) if canonical != rel => {
            if !root.join(&canonical).exists() {
                tracing::debug!(
                    path = %rel,
                    "sync: skipping a file under a reserved folder spelled in another case"
                );
                return None;
            }
            canonical
        }
        _ => rel.to_string(),
    };
    let rel = rel.as_str();
    if let Err(e) = validate_asset_path(rel) {
        tracing::debug!(path = %rel, reason = %e, "sync: skipping a file under assets/");
        return None;
    }
    // Validation already refused an extension off the allowlist; the mime is
    // resolved again from the name when the row is built.
    attachment_mime(fname)?;
    if size > MAX_ATTACHMENT_BYTES {
        tracing::debug!(path = %rel, size, "sync: skipping an attachment over the size cap");
        return None;
    }
    Some(FileCandidate {
        rel: rel.to_string(),
        abs: abs.to_path_buf(),
        mtime,
        size,
    })
}

/// What one slab task produced: a parsed record with its chunks and the
/// added-versus-updated flag, a reported failure (an empty path means the
/// blocking task itself panicked and the caller maps the task id back), or a file
/// that vanished between the scan and its slab.
enum ParseOutcome {
    Ok(Box<EngramRecord>, Vec<NewChunk>, bool),
    /// Read, but not an engram: not UTF-8, or the parser refused it.
    Failed(String, String),
    /// Not read at all (an I/O error or a task that died), so its rows were
    /// derived by nothing this run.
    Unread(String, String),
    Vanished(String),
}

/// Say once per path and process that a walk entry cannot be read: the
/// domain it is in stays behind the parser generation and every sync reparses
/// it whole, and without this line nothing would say why. Remembered for the
/// life of the process, so a watcher's full fallback over a directory that
/// stays unreadable does not repeat it on every pass; the line comes back
/// after a restart, which is also when the next reparse is tried.
fn warn_unreadable_once(domain: &str, path: Option<&Path>, reason: &str) {
    static SAID: std::sync::OnceLock<std::sync::Mutex<HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    let path = path.map(Path::to_path_buf).unwrap_or_default();
    let first = SAID
        .get_or_init(Default::default)
        .lock()
        .map(|mut said| said.insert(path.clone()))
        .unwrap_or(true);
    if first {
        tracing::warn!(
            domain,
            path = %path.display(),
            "'{}' in domain '{domain}' cannot be read ({reason}); the domain is reparsed \
             in full on every sync until it can be",
            path.display()
        );
    }
}

fn duration_ms(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}

/// The upgrade path from a database an older binary built, shared by the two
/// backends' migration tests: they build the old schema their own way, with a
/// domain row `d` already in it, open the store over it (which runs the
/// migration that adds the generation column) and hand it here.
#[cfg(test)]
pub(crate) mod upgrade_fixture {
    use std::path::Path;

    use super::*;

    const WRAPPED: &str = "---\ntype: engram\ntitle: Wrapped\npermalink: wrapped\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Wrapped\n\nThe pump rules, written the way people write them.\n\n- [fact] the purge runs before every mix swap,\n  which is why the swap waits for night #purge\n";

    async fn purge_observations(store: &dyn Store) -> i64 {
        store
            .vocabulary(Some("d"), None)
            .await
            .unwrap()
            .tags
            .iter()
            .find(|t| t.name == "purge")
            .map(|t| t.observations)
            .unwrap_or(0)
    }

    /// The row the migration left at 0 is behind, so the first plain sync
    /// reparses a file whose stamp says nothing changed. The engram's rows are
    /// staged as an older parser left them, under the exact stamp of the file
    /// on disk, so only the generation can make the sync look again.
    pub(crate) async fn first_sync_after_the_upgrade_reparses(store: &dyn Store, root: &Path) {
        let domain = store.domain_id("d").await.unwrap().expect("the old row");
        assert_eq!(
            store.parse_generation(domain).await.unwrap(),
            0,
            "a domain row from before the migration reads generation 0"
        );
        std::fs::create_dir_all(root).unwrap();
        let path = root.join("wrapped.md");
        std::fs::write(&path, WRAPPED).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let stamp = FileStamp {
            mtime: file_mtime(&meta),
            size: meta.len(),
            sha256: {
                let mut h = Sha256::new();
                h.update(WRAPPED.as_bytes());
                crate::hex_lower(&h.finalize())
            },
        };
        let engram = parse_engram(WRAPPED).unwrap();
        let mut record = EngramRecord::from_engram(&engram, "wrapped.md", stamp);
        record.observations[0].content = "the purge runs before every mix swap,".to_string();
        record.observations[0].tags.clear();
        store.upsert_engram(domain, &record).await.unwrap();
        assert_eq!(purge_observations(store).await, 0, "staged as a fragment");

        let report = sync_domain_with(store, "d", root, &ChunkParams::default())
            .await
            .unwrap();
        assert_eq!(report.updated, 1, "{report:?}");
        assert_eq!(purge_observations(store).await, 1);
        assert_eq!(
            store.parse_generation(domain).await.unwrap(),
            PARSE_GENERATION
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), WRAPPED);
    }
}
