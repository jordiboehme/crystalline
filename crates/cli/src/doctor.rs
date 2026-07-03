//! `crystalline doctor`: diagnose the index, registered domains and service
//! state, optionally repairing what can be repaired automatically.
//!
//! Checks: (a) DB orphans, an indexed file whose path no longer exists on
//! disk; (b) files on disk that are not yet indexed; (c) encoding problems
//! (BOM or null bytes), which reuses `verify`'s `E006` rule rather than
//! re-implementing the check; (d) stale service artifacts, a lock file with a
//! dead pid or a socket file left behind by a killed daemon; (e) config
//! sanity, a registered domain whose path is missing or lacks a
//! `MANIFEST.md`; (f) an embedding staleness summary, the stored model
//! against the configured one. `--fix` removes orphan rows and stale service
//! artifacts; the rest are report-only, each pointing at the right next
//! command.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, anyhow};
use crystalline_core::config::{self, DomainEntry, GlobalConfig};
use crystalline_core::verify::{self, VerifyOptions};
use crystalline_index::{Store, configured_model_id};
use crystalline_service::instance;
use serde::Serialize;

use crate::cmd;

/// One domain's diagnostics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DomainDoctor {
    /// The domain name.
    pub name: String,
    /// The domain kind, `file` or `virtual`.
    pub kind: String,
    /// The domain's resolved root path, or `(virtual)` for a virtual domain.
    pub path: String,
    /// Whether this is a virtual (database-backed) domain, whose on-disk checks
    /// do not apply.
    pub is_virtual: bool,
    /// The database engram count, reported for a virtual domain in place of the
    /// on-disk orphan and unindexed checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engrams: Option<i64>,
    /// Whether the path exists on disk.
    pub path_exists: bool,
    /// Whether `MANIFEST.md` is present at the root.
    pub manifest_present: bool,
    /// Indexed paths whose file no longer exists on disk.
    pub orphans: Vec<String>,
    /// How many of `orphans` were removed by `--fix`.
    pub orphans_removed: usize,
    /// On-disk `.md` files not yet present in the index.
    pub unindexed: Vec<String>,
    /// Encoding problems, sourced from `verify`'s `E006` rule.
    pub encoding_issues: Vec<EncodingIssue>,
}

/// One `E006` encoding finding, reported by `doctor`, fixed by `verify`.
#[derive(Debug, Clone, Serialize)]
pub struct EncodingIssue {
    /// The file path.
    pub path: String,
    /// The source line, when known.
    pub line: Option<usize>,
    /// The human message from `verify`.
    pub message: String,
}

/// Service-level (not per-domain) diagnostics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ServiceDoctor {
    /// Whether a lock file is present.
    pub lock_present: bool,
    /// The pid recorded in the lock file, when parseable.
    pub lock_pid: Option<u32>,
    /// Whether the lock is stale (present but not held by a live process).
    pub lock_stale: bool,
    /// Whether `--fix` removed the stale lock file.
    pub lock_removed: bool,
    /// Whether a socket (or, on Windows, pipe-backed) file is present.
    pub socket_present: bool,
    /// Whether the socket file is orphaned (present but no live owner).
    pub socket_orphaned: bool,
    /// Whether `--fix` removed the orphaned socket file.
    pub socket_removed: bool,
}

/// The full `doctor` report.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DoctorReport {
    /// Per-domain diagnostics.
    pub domains: Vec<DomainDoctor>,
    /// Service lock and socket diagnostics.
    pub service: ServiceDoctor,
    /// Embedding staleness summary, `None` when there is no index yet.
    pub embeddings: Option<serde_json::Value>,
    /// Whether this report was produced with `--fix`.
    pub fix: bool,
}

impl DoctorReport {
    /// Problems still unresolved after any `--fix` pass. `doctor` exits 1
    /// when this is nonzero, 0 otherwise.
    pub fn remaining_problems(&self) -> usize {
        let mut n = 0;
        for d in &self.domains {
            if !d.path_exists || !d.manifest_present {
                n += 1;
            }
            n += d.orphans.len().saturating_sub(d.orphans_removed);
            n += d.unindexed.len();
            n += d.encoding_issues.len();
        }
        if self.service.lock_stale && !self.service.lock_removed {
            n += 1;
        }
        if self.service.socket_orphaned && !self.service.socket_removed {
            n += 1;
        }
        n
    }
}

/// Run every check, applying fixes when `fix` is set.
pub async fn run(
    domain_filter: Option<&str>,
    fix: bool,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<DoctorReport> {
    let cfg = cmd::load_config(&cmd::config_path(config_override)?)?;
    let targets = select_domains(&cfg, domain_filter)?;
    let db = cmd::db_path(db_override)?;

    let store = if db.is_file() {
        Some(
            crystalline_index::open_store(&cfg.database(), Some(&db), false)
                .await
                .map_err(|e| anyhow!("could not open the index at {}: {e}", db.display()))?,
        )
    } else {
        None
    };
    // Lock once for the whole diagnostic pass: a one-shot CLI command has no
    // concurrent store users, and the helpers take a plain `&dyn Store`.
    let guard = match &store {
        Some(s) => Some(s.lock().await),
        None => None,
    };
    let store_ref: Option<&dyn Store> = guard.as_ref().map(|g| &**g as &dyn Store);

    let mut domains = Vec::with_capacity(targets.len());
    for (name, entry) in &targets {
        domains.push(check_domain(name, entry, store_ref, fix).await?);
    }

    let service = check_service(fix)?;

    let embeddings = match store_ref {
        Some(store) => Some(embedding_summary(store, &cfg).await?),
        None => None,
    };

    Ok(DoctorReport {
        domains,
        service,
        embeddings,
        fix,
    })
}

fn select_domains(cfg: &GlobalConfig, only: Option<&str>) -> Result<Vec<(String, DomainEntry)>> {
    match only {
        Some(name) => {
            let entry = cfg
                .domains
                .get(name)
                .ok_or_else(|| anyhow!("no domain named '{name}' is registered"))?;
            Ok(vec![(name.to_string(), entry.clone())])
        }
        None => Ok(cfg
            .domains
            .iter()
            .map(|(n, e)| (n.clone(), e.clone()))
            .collect()),
    }
}

async fn check_domain(
    name: &str,
    entry: &DomainEntry,
    store: Option<&dyn Store>,
    fix: bool,
) -> Result<DomainDoctor> {
    // A virtual domain has no filesystem, so the on-disk checks (path, MANIFEST,
    // orphans, unindexed, encoding) do not apply. Report its database engram
    // count instead.
    if entry.is_virtual() {
        let mut d = DomainDoctor {
            name: name.to_string(),
            kind: "virtual".to_string(),
            path: "(virtual)".to_string(),
            is_virtual: true,
            path_exists: true,
            manifest_present: true,
            ..Default::default()
        };
        if let Some(store) = store {
            let count = store
                .list_engrams(name, None, None)
                .await
                .map(|e| e.len() as i64)
                .unwrap_or(0);
            d.engrams = Some(count);
        }
        return Ok(d);
    }

    let path = cmd::resolve_domain_path(entry).unwrap_or_default();
    let path_exists = path.is_dir();
    let manifest_present = path_exists && path.join("MANIFEST.md").is_file();

    let mut d = DomainDoctor {
        name: name.to_string(),
        kind: "file".to_string(),
        path: path.display().to_string(),
        is_virtual: false,
        engrams: None,
        path_exists,
        manifest_present,
        ..Default::default()
    };

    if !path_exists {
        return Ok(d);
    }

    // (c) Encoding problems: delegate to verify's E006 rather than
    // re-implementing BOM/null-byte detection.
    if let Ok(report) = verify::verify_paths([&path], &VerifyOptions::default()) {
        d.encoding_issues = report
            .issues
            .into_iter()
            .filter(|i| i.rule == "E006")
            .map(|i| EncodingIssue {
                path: i.path.display().to_string(),
                line: i.line,
                message: i.message,
            })
            .collect();
    }

    // (a) + (b): DB orphans and unindexed files.
    if let Some(store) = store {
        let domain_id = store
            .upsert_domain(
                name,
                Some(&path.to_string_lossy()),
                crystalline_index::DomainKind::File,
            )
            .await
            .map_err(|e| anyhow!("could not read domain '{name}': {e}"))?;
        let stamps = store
            .file_stamps(domain_id)
            .await
            .map_err(|e| anyhow!("could not read file stamps for '{name}': {e}"))?;
        let on_disk = markdown_rel_paths(&path);
        let disk_set: HashSet<&str> = on_disk.iter().map(String::as_str).collect();
        let db_set: HashSet<&str> = stamps.keys().map(String::as_str).collect();

        let mut orphans: Vec<String> = stamps
            .keys()
            .filter(|p| !disk_set.contains(p.as_str()))
            .cloned()
            .collect();
        orphans.sort();
        let mut unindexed: Vec<String> = on_disk
            .into_iter()
            .filter(|p| !db_set.contains(p.as_str()))
            .collect();
        unindexed.sort();

        if fix {
            for p in &orphans {
                store.delete_engram(domain_id, p).await?;
                d.orphans_removed += 1;
            }
        }
        d.orphans = orphans;
        d.unindexed = unindexed;
    }

    Ok(d)
}

/// Every `.md` file under `root`, relative and forward-slashed, skipping
/// dot-directories and dot-files. Mirrors the sync engine's own walk so
/// orphan and unindexed detection line up with what a sync would compute.
fn markdown_rel_paths(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(&e.file_name().to_string_lossy()));
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy();
        if is_hidden(&fname) || !fname.to_lowercase().ends_with(".md") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push(rel);
    }
    out
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

fn check_service(fix: bool) -> Result<ServiceDoctor> {
    let lock_path = config::service_lock_path()
        .map_err(|e| anyhow!("could not resolve the service lock path: {e}"))?;
    let sock_path = config::service_sock_path()
        .map_err(|e| anyhow!("could not resolve the service socket path: {e}"))?;

    let lock_present = lock_path.is_file();
    let info = instance::read_lock_info();
    let pid = info.as_ref().map(|i| i.pid);
    let alive = info
        .as_ref()
        .is_some_and(|i| instance::process_alive(i.pid));
    let lock_stale = lock_present && !alive;

    let socket_present = sock_path.exists();
    let socket_orphaned = socket_present && !(lock_present && alive);

    let mut s = ServiceDoctor {
        lock_present,
        lock_pid: pid,
        lock_stale,
        lock_removed: false,
        socket_present,
        socket_orphaned,
        socket_removed: false,
    };

    if fix {
        if s.lock_stale {
            s.lock_removed = std::fs::remove_file(&lock_path).is_ok();
        }
        if s.socket_orphaned {
            s.socket_removed = std::fs::remove_file(&sock_path).is_ok();
        }
    }
    Ok(s)
}

async fn embedding_summary(store: &dyn Store, cfg: &GlobalConfig) -> Result<serde_json::Value> {
    let coverage = store
        .embedding_coverage()
        .await
        .map_err(|e| anyhow!("could not read embedding coverage: {e}"))?;
    let configured = configured_model_id(cfg.embeddings.as_ref());
    let embedded_with_configured = coverage.embedded_for(&configured);
    let stale_chunks: usize = coverage
        .models
        .iter()
        .filter(|m| m.model != configured)
        .map(|m| m.count)
        .sum();
    Ok(serde_json::json!({
        "configured_model": configured,
        "total_chunks": coverage.total_chunks,
        "embedded_with_configured_model": embedded_with_configured,
        "stale_chunks": stale_chunks,
        "models": coverage.models,
    }))
}

/// Render a report for a human.
pub fn render_human(report: &DoctorReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    for d in &report.domains {
        let _ = writeln!(out, "{} ({})", d.name, d.path);
        if d.is_virtual {
            let _ = writeln!(
                out,
                "  ok (virtual, {} engram(s) in the database)",
                d.engrams.unwrap_or(0)
            );
            continue;
        }
        if !d.path_exists {
            let _ = writeln!(out, "  [problem] domain path does not exist");
            continue;
        }
        if !d.manifest_present {
            let _ = writeln!(out, "  [problem] no MANIFEST.md at the domain root");
        }
        if !d.orphans.is_empty() {
            if d.orphans_removed > 0 {
                let _ = writeln!(
                    out,
                    "  removed {} orphan row(s): {}",
                    d.orphans_removed,
                    d.orphans.join(", ")
                );
            } else {
                let _ = writeln!(
                    out,
                    "  [problem] {} orphan row(s) (file missing on disk), rerun with --fix to remove: {}",
                    d.orphans.len(),
                    d.orphans.join(", ")
                );
            }
        }
        if !d.unindexed.is_empty() {
            let _ = writeln!(
                out,
                "  [problem] {} file(s) not indexed yet, run: crystalline sync --domain {}: {}",
                d.unindexed.len(),
                d.name,
                d.unindexed.join(", ")
            );
        }
        if !d.encoding_issues.is_empty() {
            let _ = writeln!(
                out,
                "  [problem] {} encoding issue(s), see verify rule E006:",
                d.encoding_issues.len()
            );
            for e in &d.encoding_issues {
                let _ = writeln!(out, "    {}: {}", e.path, e.message);
            }
        }
        if d.manifest_present
            && d.orphans.is_empty()
            && d.unindexed.is_empty()
            && d.encoding_issues.is_empty()
        {
            let _ = writeln!(out, "  ok");
        }
    }

    let s = &report.service;
    let _ = writeln!(out, "service:");
    if s.lock_stale {
        if s.lock_removed {
            let _ = writeln!(out, "  removed stale lock file (dead pid {:?})", s.lock_pid);
        } else {
            let _ = writeln!(
                out,
                "  [problem] stale lock file (dead pid {:?}), rerun with --fix to remove",
                s.lock_pid
            );
        }
    }
    if s.socket_orphaned {
        if s.socket_removed {
            let _ = writeln!(out, "  removed orphaned socket file");
        } else {
            let _ = writeln!(
                out,
                "  [problem] orphaned socket file, rerun with --fix to remove"
            );
        }
    }
    if !s.lock_stale && !s.socket_orphaned {
        let _ = writeln!(out, "  ok");
    }

    if let Some(e) = &report.embeddings {
        let _ = writeln!(
            out,
            "embeddings: {}/{} chunks embedded with '{}' ({} stale chunk(s) from a different model)",
            e["embedded_with_configured_model"],
            e["total_chunks"],
            e["configured_model"].as_str().unwrap_or_default(),
            e["stale_chunks"]
        );
    } else {
        let _ = writeln!(out, "embeddings: no index yet");
    }

    let remaining = report.remaining_problems();
    let _ = writeln!(out, "{remaining} problem(s) remaining");
    out
}
