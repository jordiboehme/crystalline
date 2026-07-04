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
//! against the configured one; (g) when `github.enabled`, whether this
//! machine is connected to GitHub and, per team domain, whether its local
//! origin state is present and its base snapshot still matches what was
//! recorded (`verify_base`). `--fix` removes orphan rows and stale service
//! artifacts; the rest, including the whole GitHub section, are report-only,
//! each pointing at the right next command.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, anyhow};
use crystalline_core::config::{self, DomainEntry, GlobalConfig};
use crystalline_core::verify::{self, VerifyOptions};
use crystalline_index::{Store, configured_model_id};
use crystalline_remote::TokenStore;
use crystalline_remote::github::auth::auth_base;
use crystalline_remote::state::{OriginState, verify_base};
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
    /// The instance currently hosting this file domain in a shared database, or
    /// `None` when unhosted (single-instance deployments, and virtual domains,
    /// which never take a host lock).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_instance_id: Option<String>,
    /// The host's last heartbeat, RFC 3339, when hosted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_heartbeat_at: Option<String>,
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

/// One team domain's origin diagnostics: whether its local origin state is
/// present and, when it is, whether the base snapshot `verify_base` checks
/// against still matches what was recorded.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OriginDoctor {
    /// The domain name.
    pub name: String,
    /// The GitHub repository this domain tracks, `owner/name`.
    pub repo: String,
    /// Whether `state.json` is present for this domain. Absent means the
    /// origin state was lost or the domain was never fully connected.
    pub state_present: bool,
    /// Base snapshot paths that are missing or no longer match their
    /// recorded checksum, from `verify_base`. Empty when the base tree is
    /// fully intact, or when `state_present` is false (nothing to check).
    pub base_mismatches: Vec<String>,
}

/// GitHub collaboration diagnostics, present only when `github.enabled` is
/// true (`doctor` skips the whole section rather than showing it empty
/// otherwise, matching how `embeddings` is `None` with no index yet).
#[derive(Debug, Clone, Default, Serialize)]
pub struct GithubDoctor {
    /// Whether a GitHub token is on file for this machine.
    pub connected: bool,
    /// The connected user's login, `None` when not connected.
    pub user: Option<String>,
    /// Which backend holds (or would hold) the token: `"keyring"` or
    /// `"file"`, from `TokenStore::kind`. Reported regardless of whether a
    /// token is actually saved yet, mirroring `origin_status`.
    pub token_store: String,
    /// Diagnostics for every domain connected to an origin (filtered by
    /// `--domain` like every other section).
    pub origins: Vec<OriginDoctor>,
}

/// The full `doctor` report.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DoctorReport {
    /// Per-domain diagnostics.
    pub domains: Vec<DomainDoctor>,
    /// Service lock and socket diagnostics.
    pub service: ServiceDoctor,
    /// GitHub collaboration diagnostics, `None` when `github.enabled` is
    /// false.
    pub github: Option<GithubDoctor>,
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
        // Not being connected to GitHub is not itself a problem (an
        // unconnected machine is a normal, expected state); missing or
        // corrupt origin state for an already-connected team domain is.
        if let Some(g) = &self.github {
            for o in &g.origins {
                if !o.state_present {
                    n += 1;
                }
                n += o.base_mismatches.len();
            }
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

    let github = if cfg.github_enabled() {
        Some(check_github(&cfg, &targets)?)
    } else {
        None
    };

    let embeddings = match store_ref {
        Some(store) => Some(embedding_summary(store, &cfg).await?),
        None => None,
    };

    Ok(DoctorReport {
        domains,
        service,
        github,
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

        // Ownership: who hosts this file domain in a shared database. Unhosted
        // (single-instance) domains leave this `None`.
        if let Ok(Some(host)) = store.domain_host(domain_id).await {
            d.host_instance_id = Some(host.instance_id);
            d.host_heartbeat_at = Some(host.heartbeat_at);
        }
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

/// GitHub collaboration diagnostics: this machine's connection and, per team
/// domain in `targets`, whether its local origin state is present and its
/// base snapshot still matches what was recorded. Read-only: resolving the
/// token store and calling `verify_base` never write anything, so this runs
/// the same whether or not `--fix` is set.
fn check_github(cfg: &GlobalConfig, targets: &[(String, DomainEntry)]) -> Result<GithubDoctor> {
    let api_url = cfg.github.as_ref().and_then(|g| g.api_url.clone());
    let host = cmd::bare_host(&auth_base(api_url.as_deref()));
    let state_base = config::origins_state_dir()
        .map_err(|e| anyhow!("could not resolve the origins state directory: {e}"))?;
    let store = TokenStore::resolve(host.as_deref(), &state_base);
    let token = store
        .load()
        .map_err(|e| anyhow!("could not read the saved GitHub token: {e}"))?;

    let mut origins = Vec::new();
    for (name, entry) in targets {
        let Some(origin) = &entry.origin else {
            continue;
        };
        let dir = config::origin_state_dir(name)
            .map_err(|e| anyhow!("could not resolve origin state for '{name}': {e}"))?;
        let state = OriginState::load(&dir)
            .map_err(|e| anyhow!("could not read origin state for '{name}': {e}"))?;
        let (state_present, base_mismatches) = match &state {
            Some(s) => {
                let bad = verify_base(&dir, &s.files)
                    .map_err(|e| anyhow!("could not verify the base snapshot for '{name}': {e}"))?;
                (true, bad)
            }
            None => (false, Vec::new()),
        };
        origins.push(OriginDoctor {
            name: name.clone(),
            repo: origin.repo.clone(),
            state_present,
            base_mismatches,
        });
    }

    Ok(GithubDoctor {
        connected: token.is_some(),
        user: token.as_ref().map(|t| t.user.clone()),
        token_store: store.kind().to_string(),
        origins,
    })
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
        // Ownership in a shared database: who hosts this file domain. Unhosted
        // domains print nothing extra.
        if let Some(host) = &d.host_instance_id {
            let hb = d
                .host_heartbeat_at
                .as_deref()
                .map(|h| format!(" (last heartbeat {h})"))
                .unwrap_or_default();
            let _ = writeln!(out, "  hosted by instance {host}{hb}");
        }
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

    if let Some(g) = &report.github {
        let _ = writeln!(out, "github:");
        if g.connected {
            let _ = writeln!(
                out,
                "  connected as {} ({} token store)",
                g.user.as_deref().unwrap_or("?"),
                g.token_store
            );
        } else {
            let _ = writeln!(
                out,
                "  not connected ({} token store). Run: crystalline connect github",
                g.token_store
            );
        }
        if g.origins.is_empty() {
            let _ = writeln!(out, "  no team domains connected to an origin");
        }
        for o in &g.origins {
            if !o.state_present {
                let _ = writeln!(
                    out,
                    "  [problem] {} ({}): no origin state on disk; add the domain from its origin first",
                    o.name, o.repo
                );
            } else if !o.base_mismatches.is_empty() {
                let _ = writeln!(
                    out,
                    "  [problem] {} ({}): {} base snapshot file(s) missing or modified: {}",
                    o.name,
                    o.repo,
                    o.base_mismatches.len(),
                    o.base_mismatches.join(", ")
                );
            } else {
                let _ = writeln!(out, "  {} ({}): ok", o.name, o.repo);
            }
        }
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
