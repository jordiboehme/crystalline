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
//! recorded (`verify_base`); (h) which `CRYSTALLINE_*` environment variables
//! are active, purely informational; (i) for Claude Code and Codex, whether
//! either coding-harness integration `crystalline install` wires up leaves
//! any trace on disk and, when it does, whether its settings/hooks file
//! parses and carries the `SessionStart` and `Stop` hooks and how many of
//! the four managed skills are installed or locally modified (against the
//! install receipt when one exists) and whether a receipt version skew or
//! retired leftovers await the next session-start refresh - filesystem
//! only, with no shell-out to the harness's own CLI, so this check stays
//! fast and works offline; (j) when at least one registered domain declares
//! a `## Provisioning` section, every declaring domain's decision and
//! shipped artifact counts (plus, for a team domain with an out-of-subtree
//! declaration, whether its artifact mirror has been pulled down), every
//! installed harness's drift, locally edited, orphaned and missing counts
//! against the provisioning receipt, and every domain still awaiting a
//! decision -
//! entirely read-only, straight off `crystalline_core::provision::status`,
//! never reconciling anything itself. `--fix` removes orphan rows and stale
//! service artifacts; the rest, including the whole GitHub, environment,
//! harnesses and provisioning sections, are report-only, and every finding
//! that has a fix points at the right next command.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Result, anyhow};
use crystalline_core::config::{self, DomainEntry, GlobalConfig, OriginConfig};
use crystalline_core::provision;
use crystalline_core::verify::{self, VerifyOptions};
use crystalline_core::{HarnessKind, harness_paths};
use crystalline_index::{Store, configured_model_id};
use crystalline_remote::TokenStore;
use crystalline_remote::github::auth::auth_base;
use crystalline_remote::state::{OriginState, verify_base};
use crystalline_service::EnvOverlay;
use crystalline_service::instance;
use serde::Serialize;

use crate::cmd;
use crate::install;
use crate::receipt;

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
    /// Whether this team domain is defined by an environment variable. An
    /// env-defined domain with no origin state yet is not a problem: it
    /// bootstraps itself when the daemon connects, so `remaining_problems`
    /// skips it where a config-file domain would count.
    pub env_defined: bool,
}

/// GitHub collaboration diagnostics, present only when `github.enabled` is
/// true (`doctor` skips the whole section rather than showing it empty
/// otherwise, matching how `embeddings` is `None` with no index yet).
#[derive(Debug, Clone, Default, Serialize)]
pub struct GithubDoctor {
    /// Whether a GitHub token is on file for this machine (or supplied by
    /// `CRYSTALLINE_GITHUB_TOKEN`).
    pub connected: bool,
    /// The connected user's login. `None` when not connected, and also for
    /// the environment token store, whose synthesized identity has no login
    /// attached (see `StoredToken::user_display`).
    pub user: Option<String>,
    /// Which backend holds (or would hold) the token: `"keyring"`, `"file"`
    /// or `"environment"`, from `TokenStore::kind`. Reported regardless of
    /// whether a token is actually saved yet, mirroring `origin_status`.
    pub token_store: String,
    /// Diagnostics for every domain connected to an origin (filtered by
    /// `--domain` like every other section).
    pub origins: Vec<OriginDoctor>,
}

/// One flat setting override from the environment, `(variable, key, value)`
/// reshaped into a record. Sourced from [`EnvOverlay::active_overrides`],
/// filtered to drop `domain.*` and `github.token` rows: those get the richer
/// dedicated [`EnvironmentDoctor::domains`] and
/// [`EnvironmentDoctor::github_token`] fields instead, so the flat list never
/// duplicates them. `database.url` already arrives masked as `"(set)"`.
#[derive(Debug, Clone, Serialize)]
pub struct EnvOverride {
    /// The environment variable, for example `CRYSTALLINE_DATABASE_BACKEND`.
    pub var: String,
    /// The settings registry key it overrides, for example
    /// `database.backend`.
    pub key: String,
    /// The overridden value, masked to `"(set)"` for `database.url`.
    pub value: String,
}

/// One env-defined domain, from [`EnvOverlay::env_domains`].
#[derive(Debug, Clone, Serialize)]
pub struct EnvDomainReport {
    /// The variable that defined the domain, `CRYSTALLINE_DOMAIN_<NAME>`.
    pub var: String,
    /// The mapped domain name.
    pub name: String,
    /// The domain's root path.
    pub path: String,
    /// The attached GitHub origin, rendered `owner/repo[/subpath]@branch`,
    /// when a matching `_ORIGIN` variable was present.
    pub origin: Option<String>,
}

/// Which `CRYSTALLINE_*` environment variables are active, surfaced purely
/// for visibility: never counted as a problem, and never present at all
/// (`None`) when the environment overlay carries nothing (mirroring how
/// [`DoctorReport::github`] is absent when collaboration is off). No value
/// here is a secret: `database.url` and the GitHub token are masked exactly
/// as [`EnvOverlay::active_overrides`] masks them, and the token itself is
/// reduced to a boolean.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnvironmentDoctor {
    /// The `CRYSTALLINE_CONFIG` value, when set. A path, never a secret.
    pub config_path_var: Option<String>,
    /// Every active setting override, `domain.*` and `github.token` rows
    /// excluded (see [`EnvDomainReport`] and `github_token` below).
    pub overrides: Vec<EnvOverride>,
    /// Every env-defined domain.
    pub domains: Vec<EnvDomainReport>,
    /// Whether `CRYSTALLINE_GITHUB_TOKEN` is set. The value itself never
    /// appears anywhere in this report.
    pub github_token: bool,
}

/// One coding harness's onboarding trace: whether its settings/hooks file
/// exists, parses, carries our two managed hooks and how many of the four
/// managed skills are installed at its skills folder. Checked purely from
/// the filesystem, reusing `install`'s own presence predicate and skill
/// list, with no shell-out to the harness's own CLI (`claude` or `codex`),
/// so this stays fast and works offline; user scope only, since doctor
/// reports on the ambient environment rather than any one repository's
/// `--project` setup.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HarnessDoctor {
    /// The harness's stable identifier, `"claude-code"` or `"codex"` - the
    /// same spelling `crystalline install <name>` takes.
    pub name: String,
    /// The settings/hooks file this harness reads (`settings.json` for
    /// Claude Code, `hooks.json` for Codex).
    pub settings_path: String,
    /// Whether the settings file exists on disk.
    pub settings_present: bool,
    /// The parse error, when the file is present but is not valid JSON or
    /// not a JSON object. `None` when the file is absent or parses cleanly -
    /// the only field on this struct that
    /// [`DoctorReport::remaining_problems`] counts, since a harness that was
    /// simply never installed is not itself a problem.
    pub settings_parse_error: Option<String>,
    /// Whether the `SessionStart` routing hook is present, matcher-insensitive
    /// (a hand-written recipe counts, not only one `crystalline install`
    /// wrote).
    pub session_start_hook: bool,
    /// Whether the `Stop` capture-nudge hook is present.
    pub stop_hook: bool,
    /// How many of the four managed skills have a `SKILL.md` at this
    /// harness's skills folder, whether or not its content still matches the
    /// embedded copy.
    pub skills_installed: usize,
    /// How many of the skills counted in `skills_installed` were locally
    /// modified: present, but matching neither the embedded copy nor the
    /// install receipt's recorded hash for that name.
    pub skills_modified: usize,
    /// The binary version that last reconciled this harness's user-scope
    /// install, from the install receipt. `None` when no receipt entry
    /// exists (never installed, or installed before receipts existed).
    pub receipt_version: Option<String>,
    /// Leftover folders of skills this binary no longer ships: names the
    /// receipt or the retired list knows that still have a `SKILL.md` on
    /// disk. A re-run of `crystalline install` retires them.
    pub retired_leftovers: Vec<String>,
}

/// One domain's provisioning diagnostics, straight off
/// [`provision::status`]: its decision, how many artifacts of each kind it
/// ships and, for a team domain with at least one out-of-subtree
/// `Provisioning` declaration, whether its artifact mirror has been pulled
/// down from the origin yet. Only domains that declare a `Provisioning`
/// section at all appear here - a domain with nothing to ship has nothing to
/// report.
#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningDomainDoctor {
    /// The domain name.
    pub name: String,
    /// `"allowed"`, `"denied"` or `"undecided"`.
    pub decision: String,
    /// [`crystalline_core::manifest::ArtifactType::id`] to how many
    /// artifacts of that kind the domain ships.
    pub counts: BTreeMap<String, usize>,
    /// Whether the artifact mirror is present at this team domain's origin
    /// state directory. `None` unless this is a team domain with at least
    /// one out-of-subtree declaration (a `../`-climbing path) - the only
    /// case a mirror is ever expected. The mirror itself is populated by the
    /// same poller that keeps the domain's engrams current, never by doctor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirror_present: Option<bool>,
}

/// One installed harness's provisioning diagnostics: installed counts read
/// straight from the receipt, drift and orphaned counts compared against
/// the harness's current desired set, and locally edited and missing counts
/// compared against the installed files themselves. Gated to installed
/// harnesses only, the same [`provision::installed_harnesses`] gate `apply`
/// and `status` use - a harness that was never onboarded has nothing to
/// compare against.
#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningHarnessDoctor {
    /// The harness's stable identifier.
    pub harness: String,
    /// How many files the receipt records as installed for this harness.
    pub installed_files: usize,
    /// How many MCP servers the receipt records as installed for this
    /// harness.
    pub installed_mcps: usize,
    /// How many recorded rows (files and mcps together) have drifted: their
    /// domain now ships different bytes than the receipt last recorded, so
    /// the next reconcile would update them. Counted only, never
    /// reconciled here.
    pub drift: usize,
    /// How many installed files differ locally from the receipt's hash - a
    /// user's own edit since the last reconcile, left alone by design.
    pub edited: usize,
    /// How many recorded rows (files and mcps together) are orphaned:
    /// recorded but no longer part of what any opted-in domain would ship,
    /// whether that domain opted out, was removed from the config entirely
    /// or its manifest stopped declaring the artifact. The next reconcile
    /// retires them.
    pub orphaned: usize,
    /// How many installed files the receipt records but that are no longer
    /// on disk at the harness - deleted by hand since the last reconcile,
    /// which reinstalls them.
    pub missing: usize,
}

/// One domain still awaiting a provisioning decision, named with the counts
/// it would ship.
#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningPendingDoctor {
    /// The domain name.
    pub domain: String,
    /// [`crystalline_core::manifest::ArtifactType::id`] to how many
    /// artifacts of that kind the domain would ship.
    pub counts: BTreeMap<String, usize>,
}

/// Provisioning diagnostics: every declaring domain's decision and shipped
/// counts, every installed harness's drift/edited/orphaned/missing counts
/// against the provisioning receipt, and every domain still awaiting a
/// decision. Read-only throughout, straight off [`provision::status`]:
/// never writes the receipt, never touches a harness's own config directory
/// and never spawns a harness CLI.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProvisioningDoctor {
    /// Every domain that declares a `Provisioning` section.
    pub domains: Vec<ProvisioningDomainDoctor>,
    /// Every installed harness's diagnostics.
    pub harnesses: Vec<ProvisioningHarnessDoctor>,
    /// Domains still awaiting a decision.
    pub pending: Vec<ProvisioningPendingDoctor>,
}

/// The full `doctor` report.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DoctorReport {
    /// Per-domain diagnostics.
    pub domains: Vec<DomainDoctor>,
    /// Service lock and socket diagnostics.
    pub service: ServiceDoctor,
    /// Which `CRYSTALLINE_*` environment variables are active, `None` when
    /// none are.
    pub environment: Option<EnvironmentDoctor>,
    /// GitHub collaboration diagnostics, `None` when `github.enabled` is
    /// false.
    pub github: Option<GithubDoctor>,
    /// Embedding staleness summary, `None` when there is no index yet.
    pub embeddings: Option<serde_json::Value>,
    /// Onboarding trace for the Claude Code and Codex integrations
    /// `crystalline install` wires up. `None` when neither harness leaves
    /// any trace on disk at all: no settings/hooks file and no managed
    /// skill installed.
    pub harnesses: Option<Vec<HarnessDoctor>>,
    /// Provisioning diagnostics. `None` when no registered domain declares a
    /// `Provisioning` section at all.
    pub provisioning: Option<ProvisioningDoctor>,
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
                // An env-defined team domain with no origin state bootstraps
                // itself when the daemon connects, so it is not a problem; a
                // config-file domain with no state genuinely is.
                if !o.state_present && !o.env_defined {
                    n += 1;
                }
                n += o.base_mismatches.len();
            }
        }
        // A harness that was never installed is not a problem; an
        // unparseable settings/hooks file for one that was is. A receipt
        // version skew and a retired leftover skill are never counted here:
        // both self-heal, the skew at the next session start and the
        // leftover at the next `crystalline install`, so neither should fail
        // doctor's exit code on a machine that fixes itself.
        if let Some(harnesses) = &self.harnesses {
            n += harnesses
                .iter()
                .filter(|h| h.settings_parse_error.is_some())
                .count();
        }
        // Provisioning never contributes here, the same stance environment
        // takes: an undecided domain is a normal state awaiting a person's
        // answer, and drift, edited and orphaned rows all self-heal at the
        // next `crystalline provision` (edited rows are left alone by
        // design, never "fixed").
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
    // The single load chokepoint: the effective config drives every target and
    // the db factory. The whole `LoadedConfig` stays in scope so a later
    // milestone can surface the environment overlay in the report.
    let loaded = cmd::load(config_override)?;
    let cfg = &loaded.effective;
    let targets = select_domains(cfg, domain_filter)?;
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

    let environment = check_environment(&loaded.overlay);

    let github = if cfg.github_enabled() {
        Some(check_github(cfg, &loaded.overlay, &targets)?)
    } else {
        None
    };

    let embeddings = match store_ref {
        Some(store) => Some(embedding_summary(store, cfg).await?),
        None => None,
    };

    let harnesses = check_harnesses();

    let provisioning = check_provisioning(cfg, &loaded.overlay, &targets)?;

    Ok(DoctorReport {
        domains,
        service,
        environment,
        github,
        embeddings,
        harnesses,
        provisioning,
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
    // The record's primary home is `service.json`; a still-present pre-split
    // daemon's record sitting in the lock file itself counts as present too
    // (see `instance::read_lock_info`'s legacy fallback), so an upgraded
    // doctor still flags and cleans up an old-format leftover.
    let info_path = config::service_info_path()
        .map_err(|e| anyhow!("could not resolve the service record path: {e}"))?;
    let legacy_path = config::service_lock_path()
        .map_err(|e| anyhow!("could not resolve the service lock path: {e}"))?;
    let sock_path = config::service_sock_path()
        .map_err(|e| anyhow!("could not resolve the service socket path: {e}"))?;

    let lock_present = info_path.is_file() || legacy_path.is_file();
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
            let info_removed = std::fs::remove_file(&info_path).is_ok();
            let legacy_removed = std::fs::remove_file(&legacy_path).is_ok();
            s.lock_removed = info_removed || legacy_removed;
        }
        if s.socket_orphaned {
            s.socket_removed = std::fs::remove_file(&sock_path).is_ok();
        }
    }
    Ok(s)
}

/// Which `CRYSTALLINE_*` environment variables are active, straight off the
/// parsed overlay. `None` when the overlay is empty, the same "omit the
/// section rather than show it empty" rule [`check_github`] follows. Purely
/// informational: nothing here ever feeds `remaining_problems`.
fn check_environment(overlay: &EnvOverlay) -> Option<EnvironmentDoctor> {
    if overlay.is_empty() {
        return None;
    }

    let overrides = overlay
        .active_overrides()
        .into_iter()
        .filter(|(_, key, _)| !key.starts_with("domain.") && key != "github.token")
        .map(|(var, key, value)| EnvOverride { var, key, value })
        .collect();

    let domains = overlay
        .env_domains()
        .map(|(name, env_domain)| EnvDomainReport {
            var: env_domain.var.clone(),
            name: name.clone(),
            path: env_domain
                .entry
                .file_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            origin: env_domain.entry.origin.as_ref().map(render_origin),
        })
        .collect();

    Some(EnvironmentDoctor {
        config_path_var: overlay.config_path().map(|p| p.display().to_string()),
        overrides,
        domains,
        github_token: overlay.github_token().is_some(),
    })
}

/// Renders an [`OriginConfig`] as `owner/repo[/subpath]@branch`, the same
/// grammar `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN` parses, so the report reads as
/// something an operator could paste back into that variable.
fn render_origin(origin: &OriginConfig) -> String {
    let mut s = origin.repo.clone();
    if let Some(path) = &origin.path {
        s.push('/');
        s.push_str(path);
    }
    s.push('@');
    s.push_str(origin.branch());
    s
}

/// Filesystem-only diagnostics for each coding harness `crystalline
/// install` wires up (`claude-code`, `codex` and `copilot`): whether a
/// settings/hooks file exists, whether it parses, whether it carries our two
/// managed hooks and how many of the four managed skills are present. No
/// shell-out to any harness CLI, so this stays fast and works offline; user
/// scope only, reusing `install`'s own presence predicate and skill list
/// rather than duplicating either. `None` when no harness leaves any trace
/// at all, the same "omit the section rather than show it empty" rule
/// [`check_environment`] and [`check_github`] follow.
///
/// The install receipt is loaded once here rather than inside
/// [`check_one_harness`], since every harness's user-scope entry lives in
/// the same file: a missing or corrupt receipt reads as the empty one
/// (nothing recorded), exactly like `install` itself treats it as disposable
/// derived state.
fn check_harnesses() -> Option<Vec<HarnessDoctor>> {
    let book = receipt::receipt_path()
        .ok()
        .and_then(|p| receipt::load(&p).ok())
        .unwrap_or_default();
    let harnesses: Vec<HarnessDoctor> = [
        HarnessKind::ClaudeCode,
        HarnessKind::Codex,
        HarnessKind::Copilot,
    ]
    .into_iter()
    .map(|kind| check_one_harness(kind, book.find(kind.id(), "user", None)))
    .collect();
    let any_trace = harnesses
        .iter()
        .any(|h| h.settings_present || h.skills_installed > 0);
    any_trace.then_some(harnesses)
}

/// One harness's diagnostics: read its settings/hooks file read-only, check
/// both managed hooks via [`install::harness_hook_present`] (which knows
/// each harness's file shape and session start command) and count how many
/// of [`install::MANAGED_SKILLS`] are present (and, of those, how many were
/// locally modified against either the embedded copy or `entry`'s recorded
/// hash) at its skills folder. `entry` is this harness's user-scope install
/// receipt record, `None` when it was never installed or predates receipts.
fn check_one_harness(
    harness: HarnessKind,
    entry: Option<&receipt::InstallRecord>,
) -> HarnessDoctor {
    let paths = harness_paths(harness, false);
    let settings_present = paths.settings.is_file();

    let (session_start_hook, stop_hook, settings_parse_error) =
        match install::read_settings(&paths.settings) {
            Ok(root) => (
                install::harness_hook_present(
                    harness,
                    &root,
                    "SessionStart",
                    install::session_start_command(harness),
                ),
                install::harness_hook_present(harness, &root, "Stop", install::STOP_COMMAND),
                None,
            ),
            Err(e) => (false, false, Some(e.to_string())),
        };

    let recorded_hash: std::collections::HashMap<&str, &str> = entry
        .map(|e| {
            e.skills
                .iter()
                .map(|s| (s.name.as_str(), s.sha256.as_str()))
                .collect()
        })
        .unwrap_or_default();

    let mut skills_installed = 0;
    let mut skills_modified = 0;
    for &(name, content) in install::MANAGED_SKILLS {
        let path = paths.skills_dir.join(name).join("SKILL.md");
        if let Ok(existing) = std::fs::read(&path) {
            skills_installed += 1;
            let matches_embedded = existing == content.as_bytes();
            let matches_receipt = matches_embedded
                || recorded_hash
                    .get(name)
                    .is_some_and(|&h| h == receipt::sha256_hex(&existing));
            if !matches_receipt {
                skills_modified += 1;
            }
        }
    }

    // Leftovers: every name the receipt or the static retired list still
    // remembers, deduplicated, that is not a currently managed skill and
    // whose `SKILL.md` still sits on disk. Mirrors the retirement logic in
    // `install::reconcile_skill_set` without shelling out to it.
    let managed: HashSet<&str> = install::MANAGED_SKILLS.iter().map(|&(n, _)| n).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut retired_leftovers = Vec::new();
    let candidate_names = entry
        .into_iter()
        .flat_map(|e| e.skills.iter().map(|s| s.name.as_str()))
        .chain(install::RETIRED_SKILLS.iter().copied());
    for name in candidate_names {
        if managed.contains(name) || !seen.insert(name) {
            continue;
        }
        // Receipt names are attacker-writable local state; this check is
        // read-only here, but the same guard keeps a hostile name out of the
        // report rather than have doctor point a person at retiring it by
        // hand. See `install::is_plain_skill_name`.
        if !install::is_plain_skill_name(name) {
            continue;
        }
        if paths.skills_dir.join(name).join("SKILL.md").is_file() {
            retired_leftovers.push(name.to_string());
        }
    }

    HarnessDoctor {
        name: harness.id().to_string(),
        settings_path: paths.settings.display().to_string(),
        settings_present,
        settings_parse_error,
        session_start_hook,
        stop_hook,
        skills_installed,
        skills_modified,
        receipt_version: entry.map(|e| e.version.clone()),
        retired_leftovers,
    }
}

/// Provisioning diagnostics: every declaring domain's decision and counts,
/// every installed harness's drift/edited/orphaned/missing counts and every
/// domain still awaiting a decision. `None` when no domain declares a
/// `Provisioning` section at all, the same "omit rather than show empty"
/// rule [`check_environment`], [`check_github`] and [`check_harnesses`]
/// follow. The domain-keyed lists (`domains`, `pending`) are filtered by
/// `--domain` through `targets`, matching how the per-domain and github
/// sections behave; the harness rollup stays whole-machine on purpose, since
/// the provisioning receipt is shared across every domain, the same way
/// `apply` reconciles them. Calls straight into [`provision::status`], which
/// only scans the filesystem and reads the provisioning receipt - it never
/// writes the receipt, never touches a harness's config directory and never
/// spawns a harness CLI, so this stays as read-only as the rest of doctor.
fn check_provisioning(
    cfg: &GlobalConfig,
    overlay: &EnvOverlay,
    targets: &[(String, DomainEntry)],
) -> Result<Option<ProvisioningDoctor>> {
    if !provision::any_domain_declares(cfg) {
        return Ok(None);
    }

    let receipt_path = provision::receipt_path()
        .map_err(|e| anyhow!("could not resolve the provisioning receipt path: {e}"))?;
    let install_receipt_path = provision::install_receipt_path()
        .map_err(|e| anyhow!("could not resolve the install receipt path: {e}"))?;
    let harnesses = provision::installed_harnesses(&install_receipt_path);
    // Named so an env-defined domain never surfaces in `pending`: its
    // decision can never be recorded, see `provision::apply`'s doc comment.
    let env_domains: HashSet<&str> = overlay
        .env_domains()
        .map(|(name, _)| name.as_str())
        .collect();
    let report = provision::status(cfg, &receipt_path, &harnesses, &env_domains)
        .map_err(|e| anyhow!("could not read provisioning status: {e}"))?;

    let selected: HashSet<&str> = targets.iter().map(|(name, _)| name.as_str()).collect();

    let domains = report
        .domains
        .iter()
        .filter(|d| d.declares && selected.contains(d.domain.as_str()))
        .map(|d| ProvisioningDomainDoctor {
            name: d.domain.clone(),
            decision: provisioning_decision_label(d.decision).to_string(),
            counts: d.counts.clone(),
            mirror_present: mirror_present(cfg, &d.domain),
        })
        .collect();

    let harnesses = report
        .harnesses
        .iter()
        .map(|h| ProvisioningHarnessDoctor {
            harness: h.harness.id().to_string(),
            installed_files: h.installed_files,
            installed_mcps: h.installed_mcps,
            drift: h.drift,
            edited: h.edited,
            orphaned: h.orphaned,
            missing: h.missing,
        })
        .collect();

    let pending = report
        .pending
        .iter()
        .filter(|p| selected.contains(p.domain.as_str()))
        .map(|p| ProvisioningPendingDoctor {
            domain: p.domain.clone(),
            counts: p.counts.clone(),
        })
        .collect();

    Ok(Some(ProvisioningDoctor {
        domains,
        harnesses,
        pending,
    }))
}

/// A stable human label for one [`provision::Decision`] variant, the same
/// spelling `crystalline provision status` already uses.
fn provisioning_decision_label(decision: provision::Decision) -> &'static str {
    use provision::Decision::*;
    match decision {
        Allowed => "allowed",
        Denied => "denied",
        Undecided => "undecided",
    }
}

/// Whether `name`'s artifact mirror is present at its origin state
/// directory, for a team domain with at least one out-of-subtree
/// `Provisioning` declaration. `None` when `name` is not a team domain, or is
/// one but declares no out-of-subtree path - a mirror is never expected in
/// either case. A resolved source root under `<origin_state_dir>/artifacts`
/// is exactly how [`provision::resolve_source_roots`] documents an
/// out-of-subtree decl resolving for a team domain, so its presence there is
/// the gate.
fn mirror_present(cfg: &GlobalConfig, name: &str) -> Option<bool> {
    let entry = cfg.domains.get(name)?;
    entry.origin.as_ref()?;
    let roots = provision::resolve_source_roots(name, entry);
    let mirror_root = config::origin_state_dir(name).ok()?.join("artifacts");
    roots
        .iter()
        .any(|(_, root)| root.starts_with(&mirror_root))
        .then(|| mirror_root.is_dir())
}

/// GitHub collaboration diagnostics: this machine's connection and, per team
/// domain in `targets`, whether its local origin state is present and its
/// base snapshot still matches what was recorded. Read-only: resolving the
/// token store and calling `verify_base` never write anything, so this runs
/// the same whether or not `--fix` is set. When the overlay carries
/// `CRYSTALLINE_GITHUB_TOKEN`, that store is used directly instead of probing
/// the keychain or the token file, so a headless node's diagnostics never
/// touch a credential store that variable makes irrelevant.
fn check_github(
    cfg: &GlobalConfig,
    overlay: &EnvOverlay,
    targets: &[(String, DomainEntry)],
) -> Result<GithubDoctor> {
    let api_url = cfg.github.as_ref().and_then(|g| g.api_url.clone());
    let host = cmd::bare_host(&auth_base(api_url.as_deref()));
    let (store, token) = match overlay.github_token() {
        Some(token) => {
            let store = TokenStore::env(token, host.as_deref());
            let stored = store
                .load()
                .map_err(|e| anyhow!("could not read the saved GitHub token: {e}"))?;
            (store, stored)
        }
        // The non-env doctor read fuses the backend probe and the token load
        // into a single keychain access (down from two), the same one-read
        // path the engine uses.
        None => {
            let state_base = config::origins_state_dir()
                .map_err(|e| anyhow!("could not resolve the origins state directory: {e}"))?;
            TokenStore::resolve_and_load(host.as_deref(), &state_base)
                .map_err(|e| anyhow!("could not read the saved GitHub token: {e}"))?
        }
    };

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
            env_defined: overlay.env_domain(name).is_some(),
        });
    }

    Ok(GithubDoctor {
        connected: token.is_some(),
        user: token
            .as_ref()
            .and_then(|t| t.user_display())
            .map(str::to_string),
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

    if let Some(e) = &report.environment {
        let _ = writeln!(out, "environment:");
        if let Some(path) = &e.config_path_var {
            let _ = writeln!(out, "  CRYSTALLINE_CONFIG points at {path}");
        }
        for o in &e.overrides {
            let _ = writeln!(out, "  {} overrides {} = {}", o.var, o.key, o.value);
        }
        for d in &e.domains {
            match &d.origin {
                Some(origin) => {
                    let _ = writeln!(
                        out,
                        "  {} defines domain '{}' at {} (origin {origin})",
                        d.var, d.name, d.path
                    );
                }
                None => {
                    let _ = writeln!(out, "  {} defines domain '{}' at {}", d.var, d.name, d.path);
                }
            }
        }
        if e.github_token {
            let _ = writeln!(
                out,
                "  CRYSTALLINE_GITHUB_TOKEN provides the GitHub token (read-only)"
            );
        }
    }

    if let Some(g) = &report.github {
        let _ = writeln!(out, "github:");
        if g.connected && g.token_store == "environment" {
            let _ = writeln!(
                out,
                "  connected via CRYSTALLINE_GITHUB_TOKEN (environment token store)"
            );
        } else if g.connected {
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
            if !o.state_present && o.env_defined {
                let _ = writeln!(
                    out,
                    "  {} ({}): env-defined team domain, bootstraps itself when the daemon connects",
                    o.name, o.repo
                );
            } else if !o.state_present {
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

    if let Some(harnesses) = &report.harnesses {
        let _ = writeln!(out, "harnesses:");
        for h in harnesses {
            let _ = writeln!(out, "  {} ({})", h.name, h.settings_path);
            if let Some(err) = &h.settings_parse_error {
                let _ = writeln!(out, "    [problem] settings file is not valid JSON: {err}");
            } else if !h.settings_present {
                let _ = writeln!(out, "    not installed (no settings/hooks file yet)");
            } else {
                let _ = writeln!(
                    out,
                    "    SessionStart hook: {}",
                    if h.session_start_hook {
                        "present"
                    } else {
                        "absent"
                    }
                );
                let _ = writeln!(
                    out,
                    "    Stop hook: {}",
                    if h.stop_hook { "present" } else { "absent" }
                );
                if h.session_start_hook != h.stop_hook {
                    let _ = writeln!(
                        out,
                        "    partial setup - run: crystalline install {}",
                        h.name
                    );
                }
            }
            if h.skills_installed > 0 {
                let modified = if h.skills_modified > 0 {
                    format!(", {} locally modified", h.skills_modified)
                } else {
                    String::new()
                };
                let _ = writeln!(
                    out,
                    "    skills: {}/{} installed{modified}",
                    h.skills_installed,
                    install::MANAGED_SKILLS.len()
                );
            } else {
                let _ = writeln!(out, "    skills: none installed");
            }
            if let Some(v) = &h.receipt_version
                && v != env!("CARGO_PKG_VERSION")
            {
                let _ = writeln!(
                    out,
                    "    installed by {v}, this binary is {} (refreshes at next session start)",
                    env!("CARGO_PKG_VERSION")
                );
            }
            for name in &h.retired_leftovers {
                let _ = writeln!(
                    out,
                    "    leftover retired skill: {name} (crystalline install removes it)"
                );
            }
        }
    }

    if let Some(p) = &report.provisioning {
        let _ = writeln!(out, "provisioning:");
        for d in &p.domains {
            let _ = writeln!(
                out,
                "  {}: {}, {}",
                d.name,
                d.decision,
                render_provision_counts(&d.counts)
            );
            if let Some(mirror) = d.mirror_present {
                let _ = writeln!(
                    out,
                    "    artifact mirror: {}",
                    if mirror {
                        "present"
                    } else {
                        "not pulled down yet"
                    }
                );
            }
        }
        for h in &p.harnesses {
            let _ = writeln!(
                out,
                "  {}: {} file(s) installed, {} mcp(s) installed, {} drifted, {} edited, {} orphaned, {} missing",
                h.harness,
                h.installed_files,
                h.installed_mcps,
                h.drift,
                h.edited,
                h.orphaned,
                h.missing
            );
        }
        if !p.pending.is_empty() {
            let _ = writeln!(out, "  awaiting a decision:");
            for pd in &p.pending {
                let _ = writeln!(
                    out,
                    "    {}: {} - run `crystalline provision allow {}`.",
                    pd.domain,
                    render_provision_counts(&pd.counts),
                    pd.domain
                );
            }
        }
    }

    let remaining = report.remaining_problems();
    let _ = writeln!(out, "{remaining} problem(s) remaining");
    out
}

/// Render an artifact-kind-to-count map as `"2 skills, 1 mcps"`, or `"no
/// artifacts"` when empty - the doctor-local twin of `cmd::format_counts`,
/// operating on the typed map `provision::status` returns rather than a JSON
/// value.
fn render_provision_counts(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "no artifacts".to_string();
    }
    counts
        .iter()
        .map(|(kind, n)| format!("{n} {kind}"))
        .collect::<Vec<_>>()
        .join(", ")
}
