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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use walkdir::WalkDir;

use crystalline_core::{MAX_ATTACHMENT_BYTES, attachment_mime, validate_asset_path};

use crate::embed::{ChunkParams, chunk_engram};
use crate::error::{IndexError, Result};
use crate::store::{AttachmentRow, DomainId, EngramRecord, FileStamp, NewChunk, Store};

/// Maximum concurrent hashing or parsing tasks.
const CONCURRENCY: usize = 8;

/// Changed files read, parsed and upserted per slab of the apply phase. The
/// pipeline holds one slab of contents at most, so this bounds a sync's peak
/// memory independently of the domain's size.
const SYNC_SLAB_FILES: usize = 256;

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
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
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

/// A file under the reserved `assets/` prefix that may be stored as an
/// attachment: its stat and the mime the allowlist resolved, with no bytes read
/// yet. The apply reads and hashes only the candidates whose stat moved.
struct AssetCandidate {
    rel: String,
    abs: PathBuf,
    mtime: i64,
    size: u64,
    mime: &'static str,
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
    let scan = scan_domain(name, root, stamps, chunk_params).await?;
    apply_scan(store, domain, scan).await
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
    assets: Vec<AssetCandidate>,
    /// Asset paths the scan looked for and did not find on disk. Only a
    /// targeted scan fills this; a full walk reconciles domain-wide instead.
    asset_deletes: Vec<String>,
    /// Whether `assets` is every attachable file in the domain (a full walk),
    /// so the apply may delete every row it did not see.
    assets_complete: bool,
    /// `unchanged` and `failed` from the scan; the apply fills in the rest.
    report: SyncReport,
    /// When the scan began, so the apply can report the total duration.
    started: Instant,
}

/// Scan one domain against a stamp snapshot: walk, prefilter, hash and classify,
/// with no store access at all.
///
/// `stamps` is the recorded [`FileStamp`] per relative path the caller read from
/// the store before releasing its lock; the scan classifies every file against
/// it and hands it back inside the [`DomainScan`] so the apply can re-check it.
/// The walk and hash phases run off-thread and never fail fatally: a file that
/// cannot be read lands in `report.failed`, not an error.
pub async fn scan_domain(
    name: &str,
    root: &Path,
    stamps: HashMap<String, FileStamp>,
    chunk_params: &ChunkParams,
) -> Result<DomainScan> {
    let started = Instant::now();

    // Folders the MANIFEST provisions from inside this root hold deployable
    // artifacts, not engrams, so they are pruned from the walk. Empty whenever
    // the MANIFEST is absent or unparseable, so nothing is excluded then.
    let excluded = crystalline_core::in_root_artifact_dirs(root);

    // Walk the folder, skipping dot-directories, dot-files and non-markdown.
    let mut current: HashMap<String, Scanned> = HashMap::new();
    let mut assets: Vec<AssetCandidate> = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        // Prune dot-directories and dot-files, but never the walk root itself
        // (a temp or dotted root would otherwise prune the whole tree), and
        // prune the provisioned artifact folders wholesale.
        .filter_entry(|e| {
            e.depth() == 0
                || (!is_hidden(e.file_name().to_string_lossy().as_ref())
                    && !is_excluded(e.path(), &excluded))
        })
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
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy();
        if is_hidden(&fname) {
            continue;
        }
        let rel = rel_path(root, entry.path());
        // The `assets/` folder is reserved for attachments: nothing under it is
        // ever an engram, whatever its extension, so the branch always ends the
        // iteration. A file that is not attachable is skipped outright. The
        // folder is matched case-insensitively, the same question the engine's
        // write refusals and the daemon's watcher ask, so a directory that
        // differs only in case cannot smuggle an engram in on APFS or NTFS.
        if crystalline_core::is_under_assets(&rel) {
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = file_mtime(&meta);
            if let Some(candidate) =
                asset_candidate(root, &rel, entry.path(), &fname, mtime, meta.len())
            {
                assets.push(candidate);
            }
            continue;
        }
        // OKF reserves `index.md` and `log.md` at every level: they are
        // navigation surfaces, never concept documents, so they are skipped
        // here the way dot-files are. Nothing downstream needs a filter: they
        // never reach a store row, an embedding or a search hit, and a row
        // left over from before this exclusion disappears on the next scan
        // (absent on disk means deleted).
        if crystalline_core::is_reserved_file(&fname) || !fname.to_lowercase().ends_with(".md") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
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
    )
    .await;
    // A full walk saw every file under `assets/`, so the apply may delete any
    // attachment row it did not see.
    scan.assets = assets;
    scan.assets_complete = true;
    Ok(scan)
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

    // The same artifact folders the full walk prunes: a targeted scan must index
    // exactly what a full scan would, never a file the walk would have skipped.
    let excluded = crystalline_core::in_root_artifact_dirs(root);

    let mut current: HashMap<String, Scanned> = HashMap::new();
    let mut deleted_paths: Vec<String> = Vec::new();
    let mut unreadable: Vec<(String, String)> = Vec::new();
    let mut assets: Vec<AssetCandidate> = Vec::new();
    let mut asset_deletes: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for rel in paths {
        // The same path can arrive twice in one debounce batch; classify it once.
        if !seen.insert(rel.clone()) {
            continue;
        }
        let abs = root.join(&rel);
        let hidden = rel.split('/').any(is_hidden);
        // The reserved `assets/` prefix, classified exactly as the walk does it:
        // an attachable file present on disk is an upsert, a recorded path that
        // is gone is a delete, and nothing under the prefix is ever an engram.
        if crystalline_core::is_under_assets(&rel) {
            // `symlink_metadata`, not `metadata`: the complete walk uses
            // `WalkDir`'s default `follow_links(false)` and indexes nothing
            // through a symlink, so following one here would give a symlinked
            // attachment a row that the next full scan takes away again. The
            // link is neither a file nor a delete candidate, so it falls to the
            // `Ok(_)` arm and is left alone.
            match std::fs::symlink_metadata(&abs) {
                Ok(meta) if meta.is_file() && !hidden && !is_excluded(&abs, &excluded) => {
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
        // The OKF reserved names the full walk skips (see `scan_domain`), so a
        // targeted pass indexes exactly what a full pass would.
        let reserved = crystalline_core::is_reserved_path(&rel);
        let is_md = rel.to_lowercase().ends_with(".md");
        match std::fs::metadata(&abs) {
            // An existing markdown file, filtered exactly as the walk filters:
            // a change candidate, prefiltered against the stamps downstream.
            Ok(meta)
                if meta.is_file()
                    && is_md
                    && !hidden
                    && !reserved
                    && !is_excluded(&abs, &excluded) =>
            {
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
            // non-markdown file, a hidden or artifact path): ignore it, matching
            // the walk which never indexes it either.
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
    )
    .await;
    // A targeted pass saw only the given paths, so the apply reconciles exactly
    // those rows and never deletes an attachment it was not asked about.
    scan.assets = assets;
    scan.asset_deletes = asset_deletes;
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
            Some(stamp) if stamp.mtime == scanned.mtime && stamp.size == scanned.size => {
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
            if same {
                // Touched but identical content: nothing to reindex.
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
/// [`SYNC_SLAB_FILES`], so the apply of a huge domain holds one slab of contents
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
/// is cut up, never the end state; production uses [`SYNC_SLAB_FILES`].
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
        mut report,
        started,
    } = scan;

    store.begin().await?;
    let apply = apply_changes(
        store,
        domain,
        moves,
        deletes,
        changed,
        &chunk_params,
        &snapshot,
        &root,
        &mut report,
        slab_files.max(1),
    )
    .await;
    if let Err(e) = apply {
        let _ = store.rollback().await;
        return Err(e);
    }

    if let Err(e) = apply_attachments(
        store,
        domain,
        assets,
        asset_deletes,
        assets_complete,
        &mut report,
    )
    .await
    {
        let _ = store.rollback().await;
        return Err(e);
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
    if let Err(e) = refresh_tag_aliases(store, domain).await {
        let _ = store.rollback().await;
        return Err(e);
    }

    let now = chrono::Utc::now().to_rfc3339();
    if let Err(e) = store.record_sync(domain, &now).await {
        let _ = store.rollback().await;
        return Err(e);
    }
    store.commit().await?;

    report.duration_ms = duration_ms(started.elapsed());
    Ok(report)
}

/// Reconcile a domain's attachment rows with the assets the scan found.
///
/// The rows are metadata only, so this is deliberately not the engram pipeline:
/// there is no move detection (an asset carries no permalink, so a rename is a
/// delete plus an add and costs one hash), no chunking and no embedding. What it
/// does share is the stat prefilter: a candidate whose recorded row still
/// carries the same size and modification instant cannot have changed content
/// without one of them moving, so it is skipped without reading a byte.
///
/// A full walk (`complete`) reconciles domain-wide, deleting every row it did
/// not see; a targeted pass deletes only the paths it was asked about and looked
/// for in vain, so a one-file watcher event never touches another row.
async fn apply_attachments<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    assets: Vec<AssetCandidate>,
    deletes: Vec<String>,
    complete: bool,
    report: &mut SyncReport,
) -> Result<()> {
    if !complete && assets.is_empty() && deletes.is_empty() {
        return Ok(());
    }
    let existing: HashMap<String, AttachmentRow> = store
        .list_attachments(domain)
        .await?
        .into_iter()
        .map(|row| (row.path.clone(), row))
        .collect();

    let mut seen: HashSet<String> = HashSet::new();
    for asset in assets {
        let AssetCandidate {
            rel,
            abs,
            mtime,
            size: stat_size,
            mime,
        } = asset;
        let modified = rfc3339_from_mtime(mtime);
        seen.insert(rel.clone());
        if let Some(row) = existing.get(&rel)
            && row.size == stat_size
            && row.modified == modified
        {
            continue;
        }
        let read = tokio::task::spawn_blocking(move || read_and_measure(&abs))
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())));
        let (sha256, size) = match read {
            Ok(measured) => measured,
            // The file vanished between the scan and here; its removal has its
            // own pass queued, so leave the row for that one.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(path = %rel, "sync: skipping an attachment that vanished mid-sync");
                continue;
            }
            Err(e) => {
                report.failed.push((rel, e.to_string()));
                continue;
            }
        };
        // The size is taken from the bytes that were actually read, so a file
        // that grew past the cap between the stat and the read is caught here
        // rather than stored over the ceiling.
        if size > MAX_ATTACHMENT_BYTES {
            tracing::debug!(path = %rel, size, "sync: skipping an attachment over the size cap");
            continue;
        }
        let row = AttachmentRow {
            path: rel,
            sha256,
            mime: mime.to_string(),
            size,
            modified,
        };
        store.upsert_attachment(domain, &row).await?;
    }

    if complete {
        for path in existing.keys() {
            if !seen.contains(path) {
                store.delete_attachment(domain, path).await?;
            }
        }
    } else {
        for path in deletes {
            store.delete_attachment(domain, &path).await?;
        }
    }
    Ok(())
}

/// Refresh a domain's derived tag-alias rows from its MANIFEST. Reads the stored
/// `MANIFEST.md` content, folds its `## Tag Aliases` declarations to `(alias,
/// canonical)` pairs and replaces the domain's rows with them. A missing or
/// unparseable MANIFEST folds to no pairs, so the rows are cleared and a removed
/// section takes effect on the next sync. The store never parses markdown: the
/// pairs are folded here in the format layer and handed over already lowercased.
pub async fn refresh_tag_aliases<S: Store + ?Sized>(store: &S, domain: DomainId) -> Result<()> {
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
        report.moved += 1;
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
        // recorded before the exclusion existed.
        if !crystalline_core::is_reserved_path(&path) && root.join(&path).exists() {
            report.deferred += 1;
            tracing::debug!(path = %path, "sync: deferring a delete whose file reappeared on disk");
            continue;
        }
        store.delete_engram(domain, &path).await?;
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
                    Err(e) => return ParseOutcome::Failed(scanned.rel, e.to_string()),
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
                Err(e) => ParseOutcome::Failed(String::new(), e.to_string()),
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
                // A blocking task that panicked outright loses the path with it,
                // so fall back to the task-id map to keep the failure attributable.
                let mapped = ids.remove(&id);
                let path = if path.is_empty() {
                    mapped.unwrap_or_else(|| "unknown".to_string())
                } else {
                    path
                };
                report.failed.push((path, err));
            }
            Err(join_err) => {
                let rel = ids
                    .remove(&join_err.id())
                    .unwrap_or_else(|| "unknown".to_string());
                report
                    .failed
                    .push((rel, format!("task panicked: {join_err}")));
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
            }
            Err(join_err) => {
                let rel = ids
                    .remove(&join_err.id())
                    .unwrap_or_else(|| "unknown".to_string());
                report
                    .failed
                    .push((rel, format!("task panicked: {join_err}")));
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
) -> Option<AssetCandidate> {
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
    // Validation already refused an extension off the allowlist, so this only
    // resolves the mime the row records.
    let mime = attachment_mime(fname)?;
    if size > MAX_ATTACHMENT_BYTES {
        tracing::debug!(path = %rel, size, "sync: skipping an attachment over the size cap");
        return None;
    }
    Some(AssetCandidate {
        rel: rel.to_string(),
        abs: abs.to_path_buf(),
        mtime,
        size,
        mime,
    })
}

/// What one slab task produced: a parsed record with its chunks and the
/// added-versus-updated flag, a reported failure (an empty path means the
/// blocking task itself panicked and the caller maps the task id back), or a file
/// that vanished between the scan and its slab.
enum ParseOutcome {
    Ok(Box<EngramRecord>, Vec<NewChunk>, bool),
    Failed(String, String),
    Vanished(String),
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

/// Whether `path` is one of the excluded artifact folders or lives inside one.
fn is_excluded(path: &Path, excluded: &[PathBuf]) -> bool {
    excluded.iter().any(|dir| path.starts_with(dir))
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn duration_ms(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}
