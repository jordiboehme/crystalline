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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use walkdir::WalkDir;

use crate::embed::{ChunkParams, chunk_engram};
use crate::error::{IndexError, Result};
use crate::store::{EngramRecord, FileStamp, Store};

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
    /// Files that could not be read, parsed or upserted, with the reason.
    pub failed: Vec<(String, String)>,
    /// Forward references resolved at the end of this sync.
    pub relations_resolved: u64,
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
    let started = Instant::now();
    let domain = store
        .upsert_domain(
            name,
            Some(&root.to_string_lossy()),
            crate::store::DomainKind::File,
        )
        .await?;
    let existing = store.file_stamps(domain).await?;

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

    // Prefilter: unchanged files (same mtime and size) are skipped entirely.
    let mut report = SyncReport {
        domain: name.to_string(),
        ..SyncReport::default()
    };
    let mut to_hash: Vec<Scanned> = Vec::new();
    for (rel, scanned) in &current {
        match existing.get(rel) {
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

    // Deleted candidates: recorded files no longer present on disk.
    let deleted_paths: Vec<String> = existing
        .keys()
        .filter(|p| !current.contains_key(*p))
        .cloned()
        .collect();
    // Index deleted files by checksum for move detection.
    let mut deleted_by_hash: HashMap<String, Vec<String>> = HashMap::new();
    for p in &deleted_paths {
        if let Some(stamp) = existing.get(p) {
            deleted_by_hash
                .entry(stamp.sha256.clone())
                .or_default()
                .push(p.clone());
        }
    }
    let mut deleted_remaining: std::collections::HashSet<String> =
        deleted_paths.iter().cloned().collect();

    // Classify each hashed file. The bool records whether the engram was
    // already indexed, so the apply phase can tell added from updated.
    let mut moves: Vec<(String, String)> = Vec::new();
    let mut changed: Vec<(Scanned, Hashed, bool)> = Vec::new();
    for (scanned, hashed) in hashed {
        let is_new = !existing.contains_key(&scanned.rel);
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
            let stamp = existing.get(&scanned.rel);
            let same = stamp.map(|s| s.sha256 == hashed.sha256).unwrap_or(false);
            if same {
                // Touched but identical content: nothing to reindex.
                report.unchanged += 1;
            } else {
                changed.push((scanned, hashed, true));
            }
        }
    }

    // Parse the changed files off-thread. Read failures and parse failures are
    // reported, not fatal.
    let parsed = parse_files(changed, &mut report).await;

    // Apply everything in one transaction. Duplicate-permalink upserts are
    // collected in `failed` and do not abort the batch (they are pre-checked so
    // no failing statement runs). Other errors roll the batch back.
    store.begin().await?;
    let apply = apply_changes(
        store,
        domain,
        moves,
        deleted_remaining,
        parsed,
        chunk_params,
        &mut report,
    )
    .await;
    match apply {
        Ok(()) => {}
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    }

    let resolved = match store.resolve_pending_relations(domain).await {
        Ok(n) => n,
        Err(e) => {
            let _ = store.rollback().await;
            return Err(e);
        }
    };
    report.relations_resolved = resolved;

    let now = chrono::Utc::now().to_rfc3339();
    if let Err(e) = store.record_sync(domain, &now).await {
        let _ = store.rollback().await;
        return Err(e);
    }
    store.commit().await?;

    report.duration_ms = duration_ms(started.elapsed());
    Ok(report)
}

async fn apply_changes<S: Store + ?Sized>(
    store: &S,
    domain: crate::store::DomainId,
    moves: Vec<(String, String)>,
    deleted: std::collections::HashSet<String>,
    parsed: Vec<Parsed>,
    chunk_params: &ChunkParams,
    report: &mut SyncReport,
) -> Result<()> {
    for (from, to) in moves {
        store.rename_engram(domain, &from, &to).await?;
        report.moved += 1;
    }
    for path in deleted {
        store.delete_engram(domain, &path).await?;
        report.deleted += 1;
    }
    for p in parsed {
        let existed = p.previously_indexed;
        match store.upsert_engram(domain, &p.record).await {
            Ok(id) => {
                // Recompute the engram's chunk rows. replace_chunks keeps the
                // embedding of any chunk whose fingerprint is unchanged, so an
                // edit only re-embeds the paragraphs that changed.
                let chunks = chunk_engram(
                    &p.record.title,
                    p.record.description.as_deref(),
                    &p.record.content,
                    chunk_params,
                );
                store.replace_chunks(id, &chunks).await?;
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

/// A parsed change ready to upsert.
struct Parsed {
    record: EngramRecord,
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
            let parsed = tokio::task::spawn_blocking(move || {
                crystalline_core::parse_engram(&content)
                    .map(|engram| EngramRecord::from_engram(&engram, &rel, stamp))
                    .map_err(|e| e.to_string())
            })
            .await;
            match parsed {
                Ok(Ok(record)) => ParseOutcome::Ok(Box::new(record), previously_indexed),
                Ok(Err(e)) => ParseOutcome::Failed(scanned.rel, e),
                Err(e) => ParseOutcome::Failed(scanned.rel, e.to_string()),
            }
        });
        ids.insert(handle.id(), rel);
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next_with_id().await {
        match joined {
            Ok((id, ParseOutcome::Ok(record, previously_indexed))) => {
                ids.remove(&id);
                out.push(Parsed {
                    previously_indexed,
                    record: *record,
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
    Ok(Box<EngramRecord>, bool),
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
