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
//! A sync is two phases: [`scan_domain`] is pure filesystem and CPU (walk, hash,
//! parse, chunk) and takes the stamp snapshot as input rather than reading the
//! store, so a caller runs it with no store lock held; [`apply_scan`] is the
//! transactional apply and touches the store only. [`sync_domain_with`] composes
//! the two for callers that do not manage the lock. Splitting them keeps the
//! store mutex off the long walk-hash-parse pass of a large domain.
//!
//! [`scan_paths`] is a second front on the same classification machinery for the
//! file watcher: its candidates come from a given list of relative paths instead
//! of a full walk, so a one-file edit in a large domain costs one stat and one
//! hash rather than walking every entry. Both fronts feed the identical
//! [`apply_scan`], so the targeted pass inherits every TOCTOU guard unchanged.
//! The watcher's full fallback, the startup sync and manual sync all reconcile
//! any gap, so the targeted front only has to be convergent, never perfect.
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

use crate::embed::{ChunkParams, chunk_engram};
use crate::error::{IndexError, Result};
use crate::store::{DomainId, EngramRecord, FileStamp, NewChunk, Store};

/// Maximum concurrent hashing or parsing tasks.
const CONCURRENCY: usize = 8;

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

/// The result of hashing (and reading) a candidate file.
struct Hashed {
    sha256: String,
    /// The file contents, or `None` when the bytes are not valid UTF-8.
    content: Option<String>,
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
/// classified moves, deletes and parsed-with-chunks changes, the stamp snapshot
/// they were classified against (so the apply can detect a concurrent writer),
/// the walk root (so a delete can re-stat its file) and the partial report
/// (`unchanged` and `failed` counts). [`apply_scan`] consumes it inside one
/// transaction and fills in the remaining report fields.
pub struct DomainScan {
    /// Renames: `(from, to)`, identical content moved to a new path in place.
    moves: Vec<(String, String)>,
    /// Recorded paths whose file vanished from disk, to delete from the index.
    deletes: std::collections::HashSet<String>,
    /// Parsed new and modified engrams with their chunks computed off-thread.
    parsed: Vec<Parsed>,
    /// The stamp snapshot the scan classified against, keyed by relative path.
    /// The apply compares the live db stamps against these to spot a concurrent
    /// write and defer the stale change.
    snapshot: HashMap<String, FileStamp>,
    /// The walk root, so the apply can re-stat a delete candidate on disk.
    root: PathBuf,
    /// `unchanged` and `failed` from the scan; the apply fills in the rest.
    report: SyncReport,
    /// When the scan began, so the apply can report the total duration.
    started: Instant,
}

/// Scan one domain against a stamp snapshot: walk, prefilter, hash, classify and
/// parse, with no store access at all.
///
/// `stamps` is the recorded [`FileStamp`] per relative path the caller read from
/// the store before releasing its lock; the scan classifies every file against
/// it and hands it back inside the [`DomainScan`] so the apply can re-check it.
/// The walk, hash and parse phases run off-thread and never fail fatally: a file
/// that cannot be read or parsed lands in `report.failed`, not an error.
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
        if is_hidden(&fname) || !fname.to_lowercase().ends_with(".md") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let rel = rel_path(root, entry.path());
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

    Ok(classify_and_parse(
        name,
        root,
        stamps,
        current,
        deleted_paths,
        Vec::new(),
        chunk_params,
        started,
    )
    .await)
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
    let mut seen: HashSet<String> = HashSet::new();
    for rel in paths {
        // The same path can arrive twice in one debounce batch; classify it once.
        if !seen.insert(rel.clone()) {
            continue;
        }
        let abs = root.join(&rel);
        let hidden = rel.split('/').any(is_hidden);
        let is_md = rel.to_lowercase().ends_with(".md");
        match std::fs::metadata(&abs) {
            // An existing markdown file, filtered exactly as the walk filters:
            // a change candidate, prefiltered against the stamps downstream.
            Ok(meta) if meta.is_file() && is_md && !hidden && !is_excluded(&abs, &excluded) => {
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

    classify_and_parse(
        name,
        root,
        stamps,
        current,
        deleted_paths,
        unreadable,
        chunk_params,
        started,
    )
    .await
}

/// The classification and parse core shared by [`scan_domain`] and [`scan_paths`].
///
/// Given the markdown files found on disk (`current`, keyed by relative path) and
/// the recorded paths whose file is gone (`deleted_paths`), it prefilters against
/// `stamps`, hashes the survivors, detects moves within this batch (a vanished
/// path whose stored checksum matches a new file's hash is a rename, not a delete
/// plus an add), parses the genuinely changed files off-thread and assembles the
/// [`DomainScan`]. The walk front and the path-list front differ only in how they
/// build `current` and `deleted_paths`; everything from here is identical, so the
/// two stay in step by construction. `report.unchanged` counts only paths
/// actually examined - the whole domain for a walk, only the given paths for a
/// targeted scan - because it is only ever bumped for an entry in `current`.
#[allow(clippy::too_many_arguments)]
async fn classify_and_parse(
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
        // the hashing and parsing phases append their own read and parse
        // failures to the same list.
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

    // Hash (and read) the survivors off-thread with bounded concurrency.
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
    let mut changed: Vec<(Scanned, Hashed, bool)> = Vec::new();
    for (scanned, hashed) in hashed {
        let is_new = !stamps.contains_key(&scanned.rel);
        if is_new {
            // A new file whose checksum matches a vanished file is a move.
            if let Some(candidates) = deleted_by_hash.get_mut(&hashed.sha256)
                && let Some(from) = candidates
                    .iter()
                    .find(|p| deleted_remaining.contains(*p))
                    .cloned()
            {
                deleted_remaining.remove(&from);
                moves.push((from, scanned.rel.clone()));
                continue;
            }
            changed.push((scanned, hashed, false));
        } else {
            let stamp = stamps.get(&scanned.rel);
            let same = stamp.map(|s| s.sha256 == hashed.sha256).unwrap_or(false);
            if same {
                // Touched but identical content: nothing to reindex.
                report.unchanged += 1;
            } else {
                changed.push((scanned, hashed, true));
            }
        }
    }

    // Parse the changed files off-thread, computing each engram's chunks in the
    // same off-thread task so the write transaction never runs the chunker. Read
    // failures and parse failures are reported, not fatal.
    let parsed = parse_files(changed, chunk_params, &mut report).await;

    DomainScan {
        moves,
        deletes: deleted_remaining,
        parsed,
        snapshot: stamps,
        root: root.to_path_buf(),
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
/// A concurrent writer can move the index between the scan's snapshot and this
/// apply. The apply re-reads the live stamps once, inside the transaction, and
/// defers any classified change whose live db stamp no longer matches the
/// snapshot it was classified against - see the module-level convergence note.
pub async fn apply_scan<S: Store + ?Sized>(
    store: &S,
    domain: DomainId,
    scan: DomainScan,
) -> Result<SyncReport> {
    let DomainScan {
        moves,
        deletes,
        parsed,
        snapshot,
        root,
        mut report,
        started,
    } = scan;

    store.begin().await?;
    let apply = apply_changes(
        store,
        domain,
        moves,
        deletes,
        parsed,
        &snapshot,
        &root,
        &mut report,
    )
    .await;
    if let Err(e) = apply {
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
    parsed: Vec<Parsed>,
    snapshot: &HashMap<String, FileStamp>,
    root: &Path,
    report: &mut SyncReport,
) -> Result<()> {
    // The live stamps guard against a writer that moved the index between the
    // scan's snapshot and now. Read them once, inside the transaction and only
    // when there is something to apply, so the warm no-change pass adds no query.
    let live = if moves.is_empty() && deletes.is_empty() && parsed.is_empty() {
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
        if root.join(&path).exists() {
            report.deferred += 1;
            tracing::debug!(path = %path, "sync: deferring a delete whose file reappeared on disk");
            continue;
        }
        store.delete_engram(domain, &path).await?;
        report.deleted += 1;
    }
    for p in parsed {
        // The db stamp for this path moved since the snapshot: a concurrent
        // writer indexed newer state, so the parsed content is stale. Applying it
        // would clobber the newer state, so defer and let the next pass reconcile.
        let path = p.record.path.as_str();
        if live.get(path) != snapshot.get(path) {
            report.deferred += 1;
            tracing::debug!(path = %path, "sync: deferring a change whose db stamp moved mid-scan");
            continue;
        }
        let existed = p.previously_indexed;
        match store.upsert_engram(domain, &p.record).await {
            Ok(id) => {
                // Apply the chunk rows computed off-thread in parse_files, right
                // after the upsert returns the id. replace_chunks keeps the
                // embedding of any chunk whose fingerprint is unchanged, so an
                // edit only re-embeds the paragraphs that changed; the fingerprint
                // folds in only the model id and text, so computing the chunks
                // before the transaction changes nothing about the carry-over.
                store.replace_chunks(id, &p.chunks).await?;
                if existed {
                    report.updated += 1;
                } else {
                    report.added += 1;
                }
            }
            Err(IndexError::Constraint(msg)) => {
                report.failed.push((p.record.path.clone(), msg));
            }
            Err(other) => return Err(other),
        }
    }
    Ok(())
}

/// A parsed change ready to upsert, with its embedding chunks already computed
/// off-thread so the write transaction never runs the chunker.
struct Parsed {
    record: EngramRecord,
    chunks: Vec<NewChunk>,
    previously_indexed: bool,
}

async fn hash_files(files: Vec<Scanned>, report: &mut SyncReport) -> Vec<(Scanned, Hashed)> {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set: JoinSet<(Scanned, std::io::Result<Hashed>)> = JoinSet::new();
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
            Ok((id, (scanned, Ok(hashed)))) => {
                ids.remove(&id);
                out.push((scanned, hashed));
            }
            Ok((id, (scanned, Err(_)))) => {
                ids.remove(&id);
                // Unreadable file: surface it as a change with no content so the
                // parse phase reports the failure.
                out.push((
                    scanned,
                    Hashed {
                        sha256: String::new(),
                        content: None,
                    },
                ));
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

fn read_and_hash(path: &Path) -> std::io::Result<Hashed> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let sha256 = crate::hex_lower(&hasher.finalize());
    let content = String::from_utf8(bytes).ok();
    Ok(Hashed { sha256, content })
}

async fn parse_files(
    changed: Vec<(Scanned, Hashed, bool)>,
    chunk_params: &ChunkParams,
    report: &mut SyncReport,
) -> Vec<Parsed> {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set: JoinSet<ParseOutcome> = JoinSet::new();
    // The relative path moves into its task (it lands in the `ParseOutcome`), so
    // a task that panics outright would lose it. Keep a task-id to path map so a
    // panicked task is still attributable in `failed`.
    let mut ids: HashMap<tokio::task::Id, String> = HashMap::new();
    for (scanned, hashed, previously_indexed) in changed {
        let sem = sem.clone();
        let rel = scanned.rel.clone();
        // Chunking is two small fields, cloned per task so it moves into the
        // blocking closure alongside the parse.
        let chunk_params = chunk_params.clone();
        let handle = set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore open");
            let Some(content) = hashed.content else {
                return ParseOutcome::Failed(scanned.rel, "file is not valid UTF-8".to_string());
            };
            let rel = scanned.rel.clone();
            let stamp = FileStamp {
                mtime: scanned.mtime,
                size: scanned.size,
                sha256: hashed.sha256,
            };
            // Parse and chunk in one blocking task: both are pure CPU work, and
            // computing the chunks here keeps the chunker out of the write
            // transaction the apply phase holds.
            let parsed = tokio::task::spawn_blocking(move || {
                crystalline_core::parse_engram(&content)
                    .map(|engram| {
                        let record = EngramRecord::from_engram(&engram, &rel, stamp);
                        let chunks = chunk_engram(
                            &record.title,
                            record.description.as_deref(),
                            &record.content,
                            &chunk_params,
                        );
                        (record, chunks)
                    })
                    .map_err(|e| e.to_string())
            })
            .await;
            match parsed {
                Ok(Ok((record, chunks))) => {
                    ParseOutcome::Ok(Box::new(record), chunks, previously_indexed)
                }
                Ok(Err(e)) => ParseOutcome::Failed(scanned.rel, e),
                Err(e) => ParseOutcome::Failed(scanned.rel, e.to_string()),
            }
        });
        ids.insert(handle.id(), rel);
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next_with_id().await {
        match joined {
            Ok((id, ParseOutcome::Ok(record, chunks, previously_indexed))) => {
                ids.remove(&id);
                out.push(Parsed {
                    previously_indexed,
                    record: *record,
                    chunks,
                });
            }
            Ok((id, ParseOutcome::Failed(path, err))) => {
                ids.remove(&id);
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
    out
}

enum ParseOutcome {
    Ok(Box<EngramRecord>, Vec<NewChunk>, bool),
    Failed(String, String),
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
