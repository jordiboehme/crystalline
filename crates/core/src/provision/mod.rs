//! Provisioning: turning a domain's declared artifact folders into installed
//! skills, commands, agents and MCP configs inside an AI harness's own
//! config directory.
//!
//! `model` reads a domain's `MANIFEST.md` `## Provisioning` section into
//! resolved source roots, scans those roots into a hashed [`model::DomainArtifacts`]
//! set, and projects every domain's artifacts through one harness's support
//! matrix into a [`model::DesiredSet`] - the keys a reconcile engine (M5) will
//! diff against a harness's live directory. `receipt` is the on-disk memory
//! of what a previous reconcile installed, so a later run can tell "still
//! current", "changed upstream" and "user-edited, leave it" apart. `reconcile`
//! is the engine that finally acts on that diff: writing, updating, adopting
//! and retiring files inside a harness's config directory and registering MCP
//! servers through a runner trait.
//!
//! `model` and `receipt` only ever read the filesystem and hash bytes;
//! `reconcile` is the one place that writes into a harness's config directory.
//! Even there, process work (registering an MCP server through a harness CLI)
//! stays behind the [`reconcile::McpRunner`] trait, so this crate keeps its
//! promise never to spawn a child or depend on an async runtime.

pub mod model;
pub mod receipt;
pub mod reconcile;
pub(crate) mod translate;

pub use model::{
    ArtifactFile, DesiredFile, DesiredMcp, DesiredPayload, DesiredSet, DomainArtifacts,
    McpArtifact, desired_set, harness_supports, is_plain_component, resolve_source_roots,
    scan_domain,
};
pub use receipt::{
    DomainSources, HarnessState, InstalledFile, InstalledMcp, ProvisionReceipt, SourceStamp, load,
    plain_rel_key, receipt_path, save, sha256_hex,
};
pub use reconcile::{
    ActionStatus, ArtifactAction, DeferringMcpRunner, McpOutcome, McpRunner, reconcile_harness,
};
pub use translate::{Agent, Command};

// --- orchestration -------------------------------------------------------
//
// Everything below is the decision layer every surface (the cli's `provision`
// commands, the MCP tool, the session-start auto-reconcile) calls through:
// [`installed_harnesses`] reads which harnesses a user has actually onboarded,
// [`apply`] scans every opted-in domain and reconciles it into those
// harnesses in one pass, [`status`] reports the same picture without writing
// anything, and [`any_domain_declares`] is the cheap check that gates a
// harness's visibility for domains that have not declared anything to
// provision at all.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{self, DomainEntry, GlobalConfig};
use crate::harness::{HarnessKind, artifact_base};
use crate::manifest::{ArtifactType, Manifest};
use crate::parse::parse_engram;

// --- installed-harness gate -----------------------------------------------

/// The install receipt's fixed location, `<state_dir>/installs.json`. The cli
/// crate owns the receipt's full model (it is the one that writes it); this
/// mirrors only its path, the same way [`receipt_path`] does for the
/// provisioning receipt.
pub fn install_receipt_path() -> anyhow::Result<PathBuf> {
    Ok(config::state_dir()?.join("installs.json"))
}

/// Harnesses recorded in the install receipt at `path`. Reads the file
/// shallowly (`installs[].harness` id strings); a missing, unreadable or
/// unparseable file is an empty list, never an error. The full receipt model
/// stays in the cli crate; this reader tracks only the harness ids and
/// tolerates unknown fields so the two cannot drift apart on shape.
///
/// A user-scope and a project-scope install of the same harness are one
/// harness: provisioning targets a harness's config directory as a whole, not
/// an install scope. The result is ordered by [`HarnessKind`]'s own
/// declaration order rather than the receipt's row order, so it stays stable
/// no matter what order `install`/`uninstall` left the rows in. An id this
/// binary does not recognize (a newer harness written by a future version) is
/// skipped rather than turned into an error.
pub fn installed_harnesses(path: &Path) -> Vec<HarnessKind> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(installs) = value.get("installs").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut present: HashSet<&'static str> = HashSet::new();
    for install in installs {
        if let Some(id) = install.get("harness").and_then(|v| v.as_str())
            && let Some(kind) = HarnessKind::from_id(id)
        {
            present.insert(kind.id());
        }
    }

    [
        HarnessKind::ClaudeCode,
        HarnessKind::Codex,
        HarnessKind::Copilot,
    ]
    .into_iter()
    .filter(|k| present.contains(k.id()))
    .collect()
}

// --- shared manifest and artifact helpers ---------------------------------

/// Parse `entry`'s `MANIFEST.md` into a [`Manifest`], or `None` when the
/// domain is virtual (no filesystem root) or the file is missing, unreadable
/// or fails to parse - the same never-an-error stance [`resolve_source_roots`]
/// and [`crate::manifest::in_root_artifact_dirs`] take.
fn read_manifest(entry: &DomainEntry) -> Option<Manifest> {
    let root = entry.file_path()?;
    let source = std::fs::read_to_string(root.join("MANIFEST.md")).ok()?;
    let engram = parse_engram(&source).ok()?;
    Some(Manifest::from_engram(&engram, &source))
}

/// Per-[`ArtifactType`] counts of a scanned domain's artifacts, keyed by
/// [`ArtifactType::id`], so "how many artifacts of that kind" is true for
/// every kind: a skill is counted once per skill directory (its distinct
/// first `rel` path segment), never once per file underneath it, while
/// commands, agents and mcps stay plain file counts. A kind with zero
/// artifacts is simply absent from the map, rather than present at zero.
fn count_artifacts(artifacts: &DomainArtifacts) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    let mut skill_dirs: HashSet<&str> = HashSet::new();
    for file in &artifacts.files {
        if file.kind == ArtifactType::Skills {
            if let Some(dir) = file.rel.split('/').next() {
                skill_dirs.insert(dir);
            }
        } else {
            *counts.entry(file.kind.id().to_string()).or_insert(0) += 1;
        }
    }
    if !skill_dirs.is_empty() {
        counts.insert(ArtifactType::Skills.id().to_string(), skill_dirs.len());
    }
    if !artifacts.mcps.is_empty() {
        counts.insert(ArtifactType::Mcps.id().to_string(), artifacts.mcps.len());
    }
    counts
}

/// Undecided domains (`provision` absent) that declare a `Provisioning`
/// section and ship at least one artifact - the domains a caller should
/// surface as awaiting a decision. A virtual domain never appears here: it
/// has no filesystem root to scan, and a decided domain (allowed or denied)
/// is no longer pending anything. Neither does a domain named in
/// `env_domains`: an env-defined domain's `provision` field is reset to
/// `None` on every config read (its decision lives nowhere writable - see
/// [`apply`]'s and [`status`]'s doc comments), so without this exclusion it
/// would surface as awaiting a decision forever even though allow/deny both
/// refuse it outright.
fn collect_pending(global: &GlobalConfig, env_domains: &HashSet<&str>) -> Vec<PendingDomain> {
    let mut pending = Vec::new();
    for (name, entry) in &global.domains {
        if entry.provision.is_some() || env_domains.contains(name.as_str()) {
            continue;
        }
        let Some(manifest) = read_manifest(entry) else {
            continue;
        };
        if manifest.provisioning().is_none() {
            continue;
        }
        let roots = resolve_source_roots(name, entry);
        let (artifacts, _scan_notices) = scan_domain(name, &roots);
        if artifacts.files.is_empty() && artifacts.mcps.is_empty() {
            continue;
        }
        pending.push(PendingDomain {
            domain: name.clone(),
            counts: count_artifacts(&artifacts),
        });
    }
    pending
}

/// A cheap fingerprint of `source` as just scanned, or `None` when its
/// metadata cannot be read (already gone, permissions) - the source is left
/// out of the receipt's stamps rather than failing the whole apply over one
/// unreadable file.
fn stamp_source(source: &Path, sha256: &str) -> Option<SourceStamp> {
    let meta = std::fs::metadata(source).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(SourceStamp {
        mtime: mtime as i64,
        size: meta.len(),
        sha256: sha256.to_string(),
    })
}

/// Append `notice` to `notices` unless an identical string was already
/// raised, so a caller's report stays free of repeats while keeping the
/// stable, first-occurrence order every report in this module promises.
fn push_notice(notices: &mut Vec<String>, seen: &mut HashSet<String>, notice: String) {
    if seen.insert(notice.clone()) {
        notices.push(notice);
    }
}

// --- receipt-key resolution (status only; apply's own reconcile keeps its
// key resolution private to `reconcile.rs`) --------------------------------

/// The [`ArtifactType`] a kind id names, or `None` for an unknown id.
fn kind_from_id(id: &str) -> Option<ArtifactType> {
    [
        ArtifactType::Skills,
        ArtifactType::Commands,
        ArtifactType::Agents,
        ArtifactType::Mcps,
    ]
    .into_iter()
    .find(|k| k.id() == id)
}

/// Split a `"<kind>/<rel>"` receipt key into its kind and key-shaped
/// remainder, gated the same way [`plain_rel_key`] documents: a hostile row
/// resolves to `None` and is skipped before any component of it is ever
/// joined onto a real path.
fn resolve_key(key: &str) -> Option<(ArtifactType, &str)> {
    let (prefix, rel) = key.split_once('/')?;
    let kind = kind_from_id(prefix)?;
    if !plain_rel_key(rel) {
        return None;
    }
    Some((kind, rel))
}

/// Join a gated, `/`-separated `rel` onto `base` one plain component at a
/// time, the same shape [`reconcile`]'s own join uses.
fn join_rel(base: &Path, rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for component in rel.split('/') {
        path.push(component);
    }
    path
}

/// How many of `state`'s recorded files differ locally from the receipt's
/// hash (edited) and how many are recorded but no longer on disk (missing).
/// A row this function cannot resolve to a real path - a hostile key, or a
/// kind this harness keeps no base for - is skipped in both counts: there is
/// nothing safe to compare it against.
fn count_edited_and_missing(
    harness: HarnessKind,
    state: &HarnessState,
) -> anyhow::Result<(usize, usize)> {
    let mut edited = 0;
    let mut missing = 0;
    for (key, installed) in &state.files {
        let Some((kind, rel)) = resolve_key(key) else {
            continue;
        };
        let Some(base) = artifact_base(harness, kind)? else {
            continue;
        };
        let target = join_rel(&base, rel);
        match std::fs::read(&target) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing += 1,
            Err(_) => {} // unreadable for another reason: nothing safe to compare
            Ok(bytes) => {
                if sha256_hex(&bytes) != installed.sha256 {
                    edited += 1;
                }
            }
        }
    }
    Ok((edited, missing))
}

/// How many of `state`'s recorded rows (files and mcps together) have
/// drifted from `desired` - a row still part of the desired set but whose
/// recorded hash no longer matches what a domain would now provision,
/// meaning its source changed upstream since the last `apply` - and how many
/// are orphaned: recorded but no longer part of `desired` at all, whether
/// because their domain opted out, was removed from the config entirely or
/// its manifest stopped declaring the artifact. Pure over its inputs and
/// read-only in spirit: it never reads the filesystem or a harness's config
/// directory, only compares two already-loaded maps.
fn count_drift_and_orphaned(state: &HarnessState, desired: &DesiredSet) -> (usize, usize) {
    let mut drift = 0;
    let mut orphaned = 0;
    for (key, installed) in &state.files {
        match desired.files.get(key) {
            Some(file) if file.sha256 == installed.sha256 => {}
            Some(_) => drift += 1,
            None => orphaned += 1,
        }
    }
    for (name, installed) in &state.mcps {
        match desired.mcps.get(name) {
            Some(mcp) if mcp.sha256 == installed.sha256 => {}
            Some(_) => drift += 1,
            None => orphaned += 1,
        }
    }
    (drift, orphaned)
}

// --- any_domain_declares ---------------------------------------------------

/// Whether any registered file domain's MANIFEST declares a `Provisioning`
/// section. Reads and parses each file domain's MANIFEST only, never scans an
/// artifact folder, so a caller can call this on every prompt or tool listing
/// without its cost creeping up as a domain's provisioned artifacts grow. A
/// virtual domain never declares (no filesystem root); a missing or
/// unreadable MANIFEST reads as `false` for that domain, the same
/// never-an-error stance the rest of this module takes.
pub fn any_domain_declares(global: &GlobalConfig) -> bool {
    global
        .domains
        .values()
        .any(|entry| read_manifest(entry).is_some_and(|m| m.provisioning().is_some()))
}

// --- apply ------------------------------------------------------------------

/// One undecided domain awaiting a provisioning decision, with the counts a
/// caller can show alongside the prompt: how many artifacts of each kind it
/// ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDomain {
    /// The domain's name.
    pub domain: String,
    /// [`ArtifactType::id`] to how many artifacts of that kind the domain
    /// ships.
    pub counts: BTreeMap<String, usize>,
}

/// The result of one [`apply`] run: what happened per harness, every
/// user-facing notice raised along the way and every undecided domain still
/// awaiting a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// Each requested harness, in the order given, with the actions taken
    /// reconciling it. A harness whose own reconcile errored out gets an
    /// empty action list; the failure is recorded as a notice instead.
    pub harnesses: Vec<(HarnessKind, Vec<ArtifactAction>)>,
    /// Every scan, collision, skip, mcp and gate notice raised, deduplicated
    /// and in the stable order they were first raised.
    pub notices: Vec<String>,
    /// Undecided domains that declare a `Provisioning` section and ship at
    /// least one artifact, awaiting a decision.
    pub pending: Vec<PendingDomain>,
}

/// Reconcile every opted-in domain's declared artifacts into `harnesses`,
/// updating the receipt at `receipt_path` to match.
///
/// Opted-in domains (`provision == Some(true)`) are scanned in `global`'s own
/// declaration order - the collision-precedence contract [`desired_set`]
/// documents. A virtual domain can never ship artifacts (its MANIFEST lives
/// in the database, unreadable from here), so an opted-in virtual domain
/// produces one notice and contributes nothing to any harness. When
/// `harnesses` is empty and at least one domain is opted in, `apply` stops
/// after that notice (plus a suggestion to install a harness first) and
/// writes nothing at all: there is nowhere yet to provision into. Source
/// stamps from an earlier real apply survive that early return, so a caller
/// building a change prefilter on `sources` must also check the receipt's
/// `harnesses` map for membership - stamps alone never prove a harness is
/// up to date.
///
/// The receipt is loaded once, updated per harness, then saved once at the
/// end alongside a fresh set of source stamps for every opted-in domain (a
/// domain no longer opted in loses its stamps entirely). A receipt that
/// cannot be read back is treated as empty, with a notice, rather than
/// failing the run outright - the receipt is disposable derived state, the
/// same philosophy as the search index. A single harness's own reconcile
/// erroring degrades to a notice and the run continues with the rest; the
/// only error this function returns is a failure to save the receipt itself.
///
/// `env_domains` names every env-defined domain (an `EnvOverlay`'s own
/// domain names, in the `crystalline-service` crate this module cannot
/// depend on) so `ApplyReport::pending` never lists one: its `provision`
/// field always reads back `None` no matter what was decided, since the
/// overlay re-inserts a fresh entry on every effective-config recompute, so
/// without the exclusion it would nag forever about a decision that can
/// never be recorded. Pass an empty set from a caller with no overlay to
/// apply.
pub fn apply(
    global: &GlobalConfig,
    receipt_path: &Path,
    harnesses: &[HarnessKind],
    mcp: &mut dyn McpRunner,
    env_domains: &HashSet<&str>,
) -> anyhow::Result<ApplyReport> {
    let mut notices = Vec::new();
    let mut seen = HashSet::new();

    let mut receipt = match load(receipt_path) {
        Ok(receipt) => receipt,
        Err(_) => {
            push_notice(
                &mut notices,
                &mut seen,
                format!(
                    "`{}` could not be read as a provisioning memory file and has been rebuilt from empty.",
                    receipt_path.display()
                ),
            );
            ProvisionReceipt::default()
        }
    };

    let mut any_opted_in = false;
    let mut domain_artifacts: Vec<DomainArtifacts> = Vec::new();
    for (name, entry) in &global.domains {
        if entry.provision != Some(true) {
            continue;
        }
        any_opted_in = true;
        if entry.is_virtual() {
            push_notice(
                &mut notices,
                &mut seen,
                format!(
                    "the `{name}` domain is virtual, so it has no files to provision - skipping it."
                ),
            );
            continue;
        }
        let roots = resolve_source_roots(name, entry);
        let (artifacts, scan_notices) = scan_domain(name, &roots);
        for notice in scan_notices {
            push_notice(&mut notices, &mut seen, notice);
        }
        domain_artifacts.push(artifacts);
    }

    let pending = collect_pending(global, env_domains);

    if harnesses.is_empty() {
        if any_opted_in {
            push_notice(
                &mut notices,
                &mut seen,
                "no harness is installed yet, so there is nowhere to provision into - run `crystalline install <harness>` first, for example `crystalline install claude-code`.".to_string(),
            );
        }
        return Ok(ApplyReport {
            harnesses: Vec::new(),
            notices,
            pending,
        });
    }

    let mut harness_results = Vec::new();
    for &harness in harnesses {
        let (desired, ds_notices) = desired_set(harness, &domain_artifacts);
        for notice in ds_notices {
            push_notice(&mut notices, &mut seen, notice);
        }

        let state = receipt
            .harnesses
            .entry(harness.id().to_string())
            .or_default();
        match reconcile_harness(harness, &desired, state, mcp) {
            Ok((actions, rec_notices)) => {
                for notice in rec_notices {
                    push_notice(&mut notices, &mut seen, notice);
                }
                harness_results.push((harness, actions));
            }
            Err(e) => {
                push_notice(
                    &mut notices,
                    &mut seen,
                    format!(
                        "could not reconcile {} ({e}) - its artifacts were left as they were.",
                        harness.display_name()
                    ),
                );
                harness_results.push((harness, Vec::new()));
            }
        }
    }

    let mut sources = BTreeMap::new();
    for artifacts in &domain_artifacts {
        let mut files = BTreeMap::new();
        for file in &artifacts.files {
            if let Some(stamp) = stamp_source(&file.source, &file.sha256) {
                files.insert(format!("{}/{}", file.kind.id(), file.rel), stamp);
            }
        }
        sources.insert(artifacts.domain.clone(), DomainSources { files });
    }
    receipt.sources = sources;

    save(receipt_path, &receipt)?;

    Ok(ApplyReport {
        harnesses: harness_results,
        notices,
        pending,
    })
}

// --- status -------------------------------------------------------------

/// Which side of a provisioning decision a domain is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// `provision: true` - this domain's artifacts are provisioned.
    Allowed,
    /// `provision: false` - this domain's artifacts are never provisioned.
    Denied,
    /// `provision` absent - awaiting a decision.
    Undecided,
}

impl Decision {
    /// The decision a [`DomainEntry`]'s own `provision` field carries.
    fn from_entry(entry: &DomainEntry) -> Decision {
        match entry.provision {
            Some(true) => Decision::Allowed,
            Some(false) => Decision::Denied,
            None => Decision::Undecided,
        }
    }
}

/// One domain's provisioning status: its decision, whether it declares a
/// `Provisioning` section, what it would ship and any bullets that failed to
/// parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainStatus {
    /// The domain's name.
    pub domain: String,
    /// Whether this is a virtual domain (database-backed, nothing to scan).
    pub is_virtual: bool,
    /// This domain's own decision.
    pub decision: Decision,
    /// Whether the MANIFEST carries a `Provisioning` section at all.
    pub declares: bool,
    /// [`ArtifactType::id`] to how many artifacts of that kind the domain
    /// ships. Empty for a virtual domain, which has nothing to scan.
    pub counts: BTreeMap<String, usize>,
    /// How many `Provisioning` bullets failed to parse into a declaration.
    pub parse_problems: usize,
}

/// One harness's provisioning status, read straight from the receipt and a
/// live comparison against the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessStatus {
    /// Which harness this status describes.
    pub harness: HarnessKind,
    /// How many files the receipt records as installed for this harness.
    pub installed_files: usize,
    /// How many MCP servers the receipt records as installed for this
    /// harness.
    pub installed_mcps: usize,
    /// How many installed files differ locally from the receipt's hash - a
    /// user's own edit since the last apply.
    pub edited: usize,
    /// How many installed files the receipt records but that are no longer
    /// on disk.
    pub missing: usize,
    /// How many recorded rows (files and mcps together) have drifted: their
    /// domain now ships different bytes than what the receipt last
    /// recorded, so the next `apply` would update them. Compared against
    /// the harness's current desired set - projected fresh from every
    /// opted-in domain's live scan - never against the filesystem, so this
    /// never touches a harness's config directory.
    pub drift: usize,
    /// How many recorded rows (files and mcps together) are orphaned:
    /// recorded but no longer part of the harness's current desired set,
    /// whether their domain opted out, was removed from the config entirely
    /// or its manifest stopped declaring the artifact. The next `apply`
    /// retires them.
    pub orphaned: usize,
}

/// A read-only snapshot of every domain's provisioning decision and every
/// requested harness's installed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    /// Every registered domain, in `global`'s own declaration order.
    pub domains: Vec<DomainStatus>,
    /// Every requested harness, in the order given.
    pub harnesses: Vec<HarnessStatus>,
    /// Undecided domains that declare a `Provisioning` section and ship at
    /// least one artifact, awaiting a decision.
    pub pending: Vec<PendingDomain>,
    /// Virtual domains that carry a decision (`provision` is not absent) - a
    /// decision a virtual domain can never act on, since it has nothing to
    /// provision.
    pub virtual_with_decision: Vec<String>,
}

/// A read-only snapshot: every domain's provisioning decision and declared
/// artifact counts, and every requested harness's installed, edited and
/// missing counts read from the receipt. Scans the filesystem to compare
/// against the receipt but never writes anything - not the receipt, not a
/// harness's config directory.
///
/// `env_domains` carries the same exclusion [`apply`]'s doc comment
/// documents: every env-defined domain name, so none of them ever appears in
/// `StatusReport::pending`. Pass an empty set from a caller with no overlay
/// to apply.
pub fn status(
    global: &GlobalConfig,
    receipt_path: &Path,
    harnesses: &[HarnessKind],
    env_domains: &HashSet<&str>,
) -> anyhow::Result<StatusReport> {
    let receipt = load(receipt_path).unwrap_or_default();

    let mut domains = Vec::new();
    let mut virtual_with_decision = Vec::new();
    // Every opted-in file domain's fresh scan, collected alongside `domains`
    // so the harness loop below can project each harness's current desired
    // set without a second pass over the config - the same `global.domains`
    // declaration order `apply`'s own collision-precedence contract relies
    // on.
    let mut opted_artifacts: Vec<DomainArtifacts> = Vec::new();
    for (name, entry) in &global.domains {
        let decision = Decision::from_entry(entry);
        if entry.is_virtual() {
            if entry.provision.is_some() {
                virtual_with_decision.push(name.clone());
            }
            domains.push(DomainStatus {
                domain: name.clone(),
                is_virtual: true,
                decision,
                declares: false,
                counts: BTreeMap::new(),
                parse_problems: 0,
            });
            continue;
        }

        let manifest = read_manifest(entry);
        let section = manifest.as_ref().and_then(|m| m.provisioning());
        let declares = section.is_some();
        let parse_problems = section.map(|s| s.problems.len()).unwrap_or(0);
        let roots = resolve_source_roots(name, entry);
        let (artifacts, _scan_notices) = scan_domain(name, &roots);
        domains.push(DomainStatus {
            domain: name.clone(),
            is_virtual: false,
            decision,
            declares,
            counts: count_artifacts(&artifacts),
            parse_problems,
        });
        if decision == Decision::Allowed {
            opted_artifacts.push(artifacts);
        }
    }

    let mut harness_statuses = Vec::new();
    for &harness in harnesses {
        let empty = HarnessState::default();
        let state = receipt.harnesses.get(harness.id()).unwrap_or(&empty);
        let (edited, missing) = count_edited_and_missing(harness, state)?;
        let (desired, _notices) = desired_set(harness, &opted_artifacts);
        let (drift, orphaned) = count_drift_and_orphaned(state, &desired);
        harness_statuses.push(HarnessStatus {
            harness,
            installed_files: state.files.len(),
            installed_mcps: state.mcps.len(),
            edited,
            missing,
            drift,
            orphaned,
        });
    }

    let pending = collect_pending(global, env_domains);

    Ok(StatusReport {
        domains,
        harnesses: harness_statuses,
        pending,
        virtual_with_decision,
    })
}

// --- session-start notices --------------------------------------------------

/// The provisioning notices for one session start, appended after the routing
/// prompt body and never altering it.
///
/// This runs inside the `crystalline prompt system` hook path, so it lives
/// under the same contracts [`crate::prompt`] documents: it must stay fast
/// (the prompt's under-50ms budget for 30 domains, see `prompt.rs`'s latency
/// notes) and never let a stumble reach the routing prompt. Every internal
/// failure - an unreadable receipt, an apply that cannot save - degrades to at
/// most one advisory line; nothing here returns an error or panics.
///
/// It does two jobs, in this order in the returned list:
/// 1. Reconcile opted-in domains' FILE artifacts against whatever changed at
///    their sources, and note MCP changes without touching them. Registering
///    an MCP server shells out to a harness CLI, which a hook must never do, so
///    the reconcile runs with a [`DeferringMcpRunner`]: file rows install now
///    while MCP rows defer to an explicit `crystalline provision` run and
///    surface as a single line.
/// 2. Name every undecided domain that ships artifacts, so the agent can raise
///    the decision with the user and apply it with the `provision` tool.
///
/// Determinism: domains are visited in the config's own registered order and
/// every line is built from stable inputs, so an unchanged workspace renders
/// the same lines run to run - the discipline `prompt.rs` documents for the
/// body, extended to the notices that trail it.
///
/// Latency: the fast path (nothing changed) reads the receipt once and stats
/// each opted-in domain's source files without hashing them, comparing mtime
/// and size against the receipt's stamps. Hashing only happens once a change
/// is already suspected, inside `apply`. A domain's MCP configs - always few
/// and small - are the one exception: they carry no source stamp, so they are
/// rescanned each run to tell a real MCP change from a steady one.
///
/// `env_domains` carries the same exclusion [`apply`]'s doc comment
/// documents: every env-defined domain name, so none of them ever nags the
/// pending block for a decision it can never record. Pass an empty set from
/// a caller with no overlay to apply.
pub fn session_notices(
    global: &GlobalConfig,
    receipt_path: &Path,
    harnesses: &[HarnessKind],
    env_domains: &HashSet<&str>,
) -> Vec<String> {
    // The pending block needs neither the receipt nor any hashing (read_dir
    // counts only), so it is built first and always appended last.
    let pending_lines = render_pending(&session_pending(global, env_domains));

    // Opted-in file domains, in config order. A virtual domain can never ship
    // artifacts, so it is never part of the reconcile set - apply skips it too.
    let opted: Vec<(&String, &DomainEntry)> = global
        .domains
        .iter()
        .filter(|(_, e)| e.provision == Some(true) && !e.is_virtual())
        .collect();
    if opted.is_empty() {
        return pending_lines; // nothing opted in: only the pending block, if any
    }

    // A receipt that will not load is never regenerated from a hook - that is
    // an explicit apply's job. One advisory, then only the pending block.
    let receipt = match load(receipt_path) {
        Ok(receipt) => receipt,
        Err(_) => {
            let mut out = vec![corrupt_receipt_notice()];
            out.extend(pending_lines);
            return out;
        }
    };

    // Stat-only walk of each domain's file sources (no hashing) plus a light
    // scan of its MCP configs (few, small) for the drift check below.
    let mut walked: BTreeMap<String, BTreeMap<String, FileStat>> = BTreeMap::new();
    let mut mcp_artifacts: Vec<DomainArtifacts> = Vec::new();
    for (name, entry) in &opted {
        let roots = resolve_source_roots(name, entry);
        let (mcp_roots, file_roots): (Vec<_>, Vec<_>) = roots
            .into_iter()
            .partition(|(kind, _)| *kind == ArtifactType::Mcps);
        walked.insert((*name).clone(), stat_file_sources(&file_roots));
        let (arts, _scan_notices) = scan_domain(name, &mcp_roots);
        mcp_artifacts.push(arts);
    }

    let work = matches!(
        prefilter(Some(&receipt), &walked, harnesses),
        SessionWork::Work
    ) || mcp_drift(&mcp_artifacts, &receipt, harnesses);
    if !work {
        return pending_lines; // everything already current: skip apply entirely
    }

    // Something drifted: reconcile file artifacts now and defer MCP changes.
    let mut out = Vec::new();
    match apply(
        global,
        receipt_path,
        harnesses,
        &mut DeferringMcpRunner,
        env_domains,
    ) {
        Ok(report) => {
            let file_changes = report
                .harnesses
                .iter()
                .flat_map(|(_, actions)| actions)
                .filter(|a| is_file_change(a.status))
                .count();
            if file_changes > 0 {
                out.push(reconcile_summary_notice(file_changes));
            }
            out.extend(report.notices);
            let deferred = report
                .harnesses
                .iter()
                .flat_map(|(_, actions)| actions)
                .any(|a| a.status == ActionStatus::McpDeferred);
            if deferred {
                out.push(mcp_deferred_notice());
            }
        }
        Err(_) => out.push(save_failed_notice()),
    }
    out.extend(pending_lines);
    out
}

/// The mtime (Unix seconds) and size of one source file, the stat-only
/// fingerprint the session prefilter compares against a receipt stamp without
/// ever hashing the file's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStat {
    /// Modification time, Unix seconds, truncated exactly like [`stamp_source`].
    mtime: i64,
    /// File size in bytes.
    size: u64,
}

/// Whether the session prefilter found anything worth reconciling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionWork {
    /// Every opted-in domain's file sources match the receipt, every requested
    /// harness is already recorded and the decision set is unchanged: skip apply.
    NoWork,
    /// Something drifted: run apply to reconcile it.
    Work,
    /// The receipt could not be read: surface one advisory, attempt nothing.
    Advisory,
}

/// Decide, without hashing a single file, whether the session needs to run
/// apply. Pure over its inputs so the whole decision table is unit-testable:
/// `receipt` is `None` for a receipt that would not load (giving
/// [`SessionWork::Advisory`]), `walked` maps each opted-in file domain to the
/// mtime-and-size stat of every source file, and `harnesses` is the set that
/// must each already be recorded.
///
/// [`SessionWork::Work`] on any of: a stamp whose mtime or size moved, a source
/// file added or removed, an opted-in domain the receipt never stamped (a fresh
/// opt-in), a stamped domain no longer opted in (a fresh opt-out) or a harness
/// with no receipt entry yet (freshly installed - stamps alone never prove a
/// harness current, the same reason `apply`'s doc comment gives). MCP drift is
/// judged separately by [`mcp_drift`], since MCP configs carry no source stamp.
fn prefilter(
    receipt: Option<&ProvisionReceipt>,
    walked: &BTreeMap<String, BTreeMap<String, FileStat>>,
    harnesses: &[HarnessKind],
) -> SessionWork {
    let Some(receipt) = receipt else {
        return SessionWork::Advisory;
    };

    // The decision set (opted-in file domains = the keys of `walked`) must be
    // exactly the domains the receipt last stamped, both ways.
    if walked.len() != receipt.sources.len()
        || !walked.keys().all(|d| receipt.sources.contains_key(d))
    {
        return SessionWork::Work;
    }

    // Every stamped file must still be on disk at the same mtime and size, and
    // no new file may have appeared under a root.
    for (domain, files) in walked {
        let Some(stamped) = receipt.sources.get(domain) else {
            return SessionWork::Work; // unreachable: set equality checked above
        };
        if files.len() != stamped.files.len() {
            return SessionWork::Work;
        }
        for (key, stat) in files {
            match stamped.files.get(key) {
                Some(stamp) if stamp.mtime == stat.mtime && stamp.size == stat.size => {}
                _ => return SessionWork::Work,
            }
        }
    }

    // A requested harness recorded nowhere yet (freshly installed) needs a
    // first pass even when every source file is untouched.
    if harnesses
        .iter()
        .any(|h| !receipt.harnesses.contains_key(h.id()))
    {
        return SessionWork::Work;
    }

    SessionWork::NoWork
}

/// Whether any requested harness's recorded MCP servers differ from what the
/// opted-in domains would provision right now. MCP configs carry no source
/// stamp - the receipt's `sources` tracks file artifacts only - so this is the
/// one place the session path rescans, which stays cheap because MCP configs
/// are always few and small. `mcp_artifacts` holds each opted-in domain's
/// freshly scanned MCP set (its files left empty); the comparison mirrors what
/// `apply` would act on for each harness, so a match here means apply would
/// raise no MCP action at all.
fn mcp_drift(
    mcp_artifacts: &[DomainArtifacts],
    receipt: &ProvisionReceipt,
    harnesses: &[HarnessKind],
) -> bool {
    for &harness in harnesses {
        let (desired, _notices) = desired_set(harness, mcp_artifacts);
        let desired_shas: BTreeMap<&str, &str> = desired
            .mcps
            .iter()
            .map(|(name, mcp)| (name.as_str(), mcp.sha256.as_str()))
            .collect();
        let recorded_shas: BTreeMap<&str, &str> = receipt
            .harnesses
            .get(harness.id())
            .map(|state| {
                state
                    .mcps
                    .iter()
                    .map(|(name, mcp)| (name.as_str(), mcp.sha256.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        if desired_shas != recorded_shas {
            return true;
        }
    }
    false
}

// --- session-start stat walk (no hashing) -----------------------------------

/// Stat-only walk of a domain's file source roots (skills, commands, agents;
/// never MCP configs, which carry no stamp). Mirrors `model`'s scan selection
/// exactly - a skill dir needs its `SKILL.md`, commands are `*.md` at any
/// depth, agents are top-level `*.md` - so the keys line up with the stamps
/// `apply` wrote, while reading only metadata, never a file's bytes. A hostile
/// path component is skipped the same way the scan skips it, so a walked key
/// can never diverge from a stamped one over an unsafe name.
fn stat_file_sources(roots: &[(ArtifactType, PathBuf)]) -> BTreeMap<String, FileStat> {
    let mut out = BTreeMap::new();
    for (kind, root) in roots {
        match kind {
            ArtifactType::Skills => stat_skills(root, &mut out),
            ArtifactType::Commands => stat_commands(root, &mut out),
            ArtifactType::Agents => stat_agents(root, &mut out),
            ArtifactType::Mcps => {} // never stamped: judged by mcp_drift instead
        }
    }
    out
}

/// The mtime (Unix seconds) and size of `path`, or `None` when its metadata
/// cannot be read - computed exactly like the stamp [`stamp_source`] writes, so
/// the two are directly comparable.
fn file_stat(path: &Path) -> Option<FileStat> {
    // Metadata-readable is a looser bar than `hash_file`'s content read in
    // `scan_domain`, and that asymmetry is intentional and safe: a file whose
    // metadata reads here but whose bytes later fail to read only makes the
    // stat walk over-report a key relative to a real scan, which triggers a
    // harmless idempotent apply, never a missed change.
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(FileStat {
        mtime: mtime as i64,
        size: meta.len(),
    })
}

/// Recursively collect every visible file under `dir` as `(rel, FileStat)`
/// pairs, `rel` joined with `/` from the walk root - the stat-only twin of
/// `model`'s `walk_visible`. Hidden entries and symlinks are skipped the same
/// way, so this walk sees exactly the files the scan would.
fn walk_visible_stat(dir: &Path, rel: &mut Vec<String>, out: &mut Vec<(String, FileStat)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            rel.push(name);
            walk_visible_stat(&path, rel, out);
            rel.pop();
        } else if file_type.is_file()
            && let Some(stat) = file_stat(&path)
        {
            rel.push(name);
            out.push((rel.join("/"), stat));
            rel.pop();
        }
    }
}

/// Stat every file of each `SKILL.md`-bearing skill directory under `root`,
/// keyed `skills/<skill-dir>/<path-within-skill>` - the stat-only mirror of
/// `model`'s `scan_skills`.
fn stat_skills(root: &Path, out: &mut BTreeMap<String, FileStat>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !is_plain_component(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let skill_dir = entry.path();
        if !skill_dir.join("SKILL.md").is_file() {
            continue;
        }
        let mut visible = Vec::new();
        walk_visible_stat(&skill_dir, &mut Vec::new(), &mut visible);
        for (rel_in_skill, stat) in visible {
            if rel_in_skill.split('/').all(is_plain_component) {
                out.insert(format!("skills/{name}/{rel_in_skill}"), stat);
            }
        }
    }
}

/// Stat every `*.md` file under `root` at any depth, keyed `commands/<rel>` -
/// the stat-only mirror of `model`'s `scan_commands`.
fn stat_commands(root: &Path, out: &mut BTreeMap<String, FileStat>) {
    let mut visible = Vec::new();
    walk_visible_stat(root, &mut Vec::new(), &mut visible);
    for (rel, stat) in visible {
        if Path::new(&rel).extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if rel.split('/').all(is_plain_component) {
            out.insert(format!("commands/{rel}"), stat);
        }
    }
}

/// Stat every top-level `*.md` or `*.toml` file directly inside `root` (no
/// recursion), keyed `agents/<file>` - the stat-only mirror of `model`'s
/// `scan_agents`, matching its `.md`-or-`.toml` selection so a walked key never
/// diverges from a stamped one over a Codex-dialect agent.
fn stat_agents(root: &Path, out: &mut BTreeMap<String, FileStat>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !is_plain_component(&name) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if !file_type.is_file() || !matches!(ext, Some("md") | Some("toml")) {
            continue;
        }
        if let Some(stat) = file_stat(&path) {
            out.insert(format!("agents/{name}"), stat);
        }
    }
}

// --- session-start pending block (read_dir counts, no hashing) --------------

/// Undecided declaring domains that ship at least one artifact, with per-type
/// counts taken at read_dir level - never hashing, unlike [`collect_pending`]
/// which the (non hook) apply and status paths can afford. This is what the
/// session pending block names for the agent to raise with the user.
/// `env_domains` is the same exclusion `collect_pending` documents: an
/// env-defined domain is never included, since its `provision` decision can
/// never be recorded.
fn session_pending(global: &GlobalConfig, env_domains: &HashSet<&str>) -> Vec<PendingDomain> {
    let mut pending = Vec::new();
    for (name, entry) in &global.domains {
        if entry.provision.is_some() || env_domains.contains(name.as_str()) {
            continue;
        }
        let Some(manifest) = read_manifest(entry) else {
            continue;
        };
        if manifest.provisioning().is_none() {
            continue;
        }
        let roots = resolve_source_roots(name, entry);
        let counts = count_source_roots(&roots);
        if counts.values().copied().sum::<usize>() == 0 {
            continue; // declares a section but ships nothing
        }
        pending.push(PendingDomain {
            domain: name.clone(),
            counts,
        });
    }
    pending
}

/// Per-[`ArtifactType`] artifact counts for a domain's resolved source roots,
/// at read_dir level and never hashing. File kinds reuse the scan-mirroring
/// stat walk; a skill is counted once per skill directory (mirroring
/// [`count_artifacts`]'s skill-granularity contract), never once per file
/// underneath it. MCP configs are counted as the `*.json` files their root
/// holds, without parsing them, keeping the pending block's "count, do not
/// hash" contract.
fn count_source_roots(roots: &[(ArtifactType, PathBuf)]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (kind, root) in roots {
        let n = match kind {
            ArtifactType::Skills => {
                let mut m = BTreeMap::new();
                stat_skills(root, &mut m);
                m.keys()
                    .filter_map(|key| key.strip_prefix("skills/"))
                    .filter_map(|rel| rel.split('/').next())
                    .collect::<HashSet<_>>()
                    .len()
            }
            ArtifactType::Commands => {
                let mut m = BTreeMap::new();
                stat_commands(root, &mut m);
                m.len()
            }
            ArtifactType::Agents => {
                let mut m = BTreeMap::new();
                stat_agents(root, &mut m);
                m.len()
            }
            ArtifactType::Mcps => count_json_files(root),
        };
        if n > 0 {
            *counts.entry(kind.id().to_string()).or_insert(0) += n;
        }
    }
    counts
}

/// The number of visible top-level `*.json` files in `root`, the read_dir-level
/// count of MCP configs a domain ships.
fn count_json_files(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            !name.starts_with('.')
                && e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && Path::new(&name).extension().and_then(|x| x.to_str()) == Some("json")
        })
        .count()
}

// --- session-start notice text ----------------------------------------------

/// Whether an [`ActionStatus`] is a file artifact that was actually written or
/// removed, so the session summary counts only real file changes and skips the
/// receipt-only adoption, the left-in-place foreign file and every MCP row.
fn is_file_change(status: ActionStatus) -> bool {
    matches!(
        status,
        ActionStatus::Installed
            | ActionStatus::Updated
            | ActionStatus::UpdatedBackup
            | ActionStatus::Removed
            | ActionStatus::RetiredBackup
    )
}

/// One line per undecided domain that ships artifacts, naming its per-type
/// counts and how to decide - raise it with the user, then apply the decision
/// with the `provision` tool or the CLI. Empty when nothing is pending.
fn render_pending(pending: &[PendingDomain]) -> Vec<String> {
    pending
        .iter()
        .map(|p| {
            format!(
                "[crystalline] The `{}` domain ships artifacts to provision ({}) but has no decision yet - ask the user, then apply it with the `provision` tool or `crystalline provision allow {}`.",
                p.domain,
                render_counts(&p.counts),
                p.domain
            )
        })
        .collect()
}

/// A domain's artifact counts as `kind: n` pairs in a stable order, for a
/// pending line - for example `skills: 2, commands: 1`.
fn render_counts(counts: &BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(kind, n)| format!("{kind}: {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The summary line for a session that reconciled `changes` file artifacts.
fn reconcile_summary_notice(changes: usize) -> String {
    format!(
        "[crystalline] Refreshed {changes} provisioned artifact(s) from your opted-in domains for this session."
    )
}

/// The single line raised when the session deferred one or more MCP changes.
fn mcp_deferred_notice() -> String {
    "[crystalline] MCP server changes are waiting - apply them with the `provision` tool or `crystalline provision`.".to_string()
}

/// The advisory for a receipt that would not load: a hook never rebuilds it, so
/// this points at the explicit run that will.
fn corrupt_receipt_notice() -> String {
    "[crystalline] Your provisioning memory could not be read - run `crystalline provision` to rebuild it and reconcile your domains.".to_string()
}

/// The advisory for an apply that reconciled but could not save its receipt.
fn save_failed_notice() -> String {
    "[crystalline] Could not save your provisioning memory this session - run `crystalline provision` to reconcile your domains.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- installed_harnesses --------------------------------------------------

    #[test]
    fn a_missing_install_receipt_is_no_harnesses() {
        let dir = tempfile::tempdir().unwrap();
        assert!(installed_harnesses(&dir.path().join("missing.json")).is_empty());
    }

    #[test]
    fn a_corrupt_install_receipt_is_no_harnesses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installs.json");
        std::fs::write(&path, "{ nope").unwrap();
        assert!(installed_harnesses(&path).is_empty());
    }

    #[test]
    fn cli_shaped_receipt_dedupes_scopes_skips_unknown_ids_and_orders_by_declaration() {
        // The exact shape `crates/cli/src/receipt.rs` writes, spelled out here
        // rather than imported: core must never depend on the cli crate. A
        // user-scope and a project-scope claude-code install collapse to one
        // entry; an unknown harness id is skipped; codex (declared after
        // claude-code but written first in the file) still comes out after it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installs.json");
        std::fs::write(
            &path,
            r#"{
                "format": 1,
                "installs": [
                    {
                        "harness": "codex",
                        "scope": "user",
                        "version": "0.7.3",
                        "parts": {"mcp": false, "hooks": true, "skills": true},
                        "skills": []
                    },
                    {
                        "harness": "claude-code",
                        "scope": "user",
                        "version": "0.7.3",
                        "parts": {"mcp": true, "hooks": true, "skills": true},
                        "skills": [{"name": "crystalline-routing", "sha256": "abc"}]
                    },
                    {
                        "harness": "claude-code",
                        "scope": "project",
                        "project_path": "/repo",
                        "version": "0.7.3",
                        "parts": {"mcp": true, "hooks": true, "skills": true},
                        "skills": []
                    },
                    {
                        "harness": "some-future-harness",
                        "scope": "user",
                        "version": "0.9.0",
                        "parts": {"mcp": true, "hooks": true, "skills": true},
                        "skills": []
                    }
                ]
            }"#,
        )
        .unwrap();

        let harnesses = installed_harnesses(&path);
        assert_eq!(harnesses, vec![HarnessKind::ClaudeCode, HarnessKind::Codex]);
    }

    // --- any_domain_declares ---------------------------------------------------

    fn write_manifest(dir: &Path, bullets: &str) {
        let source = format!(
            "---\ntype: manifest\ntitle: harbor\npermalink: manifest\n---\n\n\
             # harbor\n\n\
             ## Scope\n\n- Coastal navigation knowledge\n\n\
             ## When to Use\n\n- When docking\n\n{bullets}"
        );
        std::fs::write(dir.join("MANIFEST.md"), source).unwrap();
    }

    #[test]
    fn any_domain_declares_true_when_a_file_domain_has_a_provisioning_section() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "## Provisioning\n\n- skills: skills\n");
        let mut global = GlobalConfig::default();
        global
            .domains
            .insert("harbor".to_string(), DomainEntry::file(dir.path()));
        assert!(any_domain_declares(&global));
    }

    #[test]
    fn any_domain_declares_false_without_a_section_missing_manifest_or_virtual() {
        let no_section = tempfile::tempdir().unwrap();
        write_manifest(no_section.path(), "");
        let missing_manifest = tempfile::tempdir().unwrap();

        let mut global = GlobalConfig::default();
        global.domains.insert(
            "no-section".to_string(),
            DomainEntry::file(no_section.path()),
        );
        global.domains.insert(
            "no-manifest".to_string(),
            DomainEntry::file(missing_manifest.path()),
        );
        global
            .domains
            .insert("notes".to_string(), DomainEntry::virtual_domain());

        assert!(!any_domain_declares(&global));
    }

    // --- session prefilter ----------------------------------------------------

    fn stamp(mtime: i64, size: u64) -> SourceStamp {
        SourceStamp {
            mtime,
            size,
            sha256: String::new(),
        }
    }

    /// A receipt with one file domain `harbor` (two stamped files) provisioned
    /// into claude-code - the steady state each trigger below perturbs.
    fn steady_receipt() -> ProvisionReceipt {
        let mut receipt = ProvisionReceipt::default();
        receipt.sources.insert(
            "harbor".to_string(),
            DomainSources {
                files: BTreeMap::from([
                    ("skills/tide-tables/SKILL.md".to_string(), stamp(100, 10)),
                    ("agents/quartermaster.md".to_string(), stamp(200, 20)),
                ]),
            },
        );
        receipt
            .harnesses
            .insert("claude-code".to_string(), HarnessState::default());
        receipt
    }

    fn steady_walk() -> BTreeMap<String, BTreeMap<String, FileStat>> {
        BTreeMap::from([(
            "harbor".to_string(),
            BTreeMap::from([
                (
                    "skills/tide-tables/SKILL.md".to_string(),
                    FileStat {
                        mtime: 100,
                        size: 10,
                    },
                ),
                (
                    "agents/quartermaster.md".to_string(),
                    FileStat {
                        mtime: 200,
                        size: 20,
                    },
                ),
            ]),
        )])
    }

    #[test]
    fn prefilter_no_work_when_stamps_harness_and_decisions_all_match() {
        assert_eq!(
            prefilter(
                Some(&steady_receipt()),
                &steady_walk(),
                &[HarnessKind::ClaudeCode]
            ),
            SessionWork::NoWork
        );
    }

    #[test]
    fn prefilter_work_on_mtime_drift() {
        let mut walk = steady_walk();
        walk.get_mut("harbor")
            .unwrap()
            .get_mut("agents/quartermaster.md")
            .unwrap()
            .mtime = 201;
        assert_eq!(
            prefilter(Some(&steady_receipt()), &walk, &[HarnessKind::ClaudeCode]),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_work_on_size_drift() {
        let mut walk = steady_walk();
        walk.get_mut("harbor")
            .unwrap()
            .get_mut("skills/tide-tables/SKILL.md")
            .unwrap()
            .size = 11;
        assert_eq!(
            prefilter(Some(&steady_receipt()), &walk, &[HarnessKind::ClaudeCode]),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_work_on_new_opted_in_domain() {
        let mut walk = steady_walk();
        walk.insert("cove".to_string(), BTreeMap::new());
        assert_eq!(
            prefilter(Some(&steady_receipt()), &walk, &[HarnessKind::ClaudeCode]),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_work_on_dropped_opted_in_domain() {
        let mut receipt = steady_receipt();
        // cove was opted in and stamped last time, but is absent from this
        // run's walk (opted out): a decision-set mismatch.
        receipt
            .sources
            .insert("cove".to_string(), DomainSources::default());
        assert_eq!(
            prefilter(Some(&receipt), &steady_walk(), &[HarnessKind::ClaudeCode]),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_work_on_added_source_file() {
        let mut walk = steady_walk();
        walk.get_mut("harbor").unwrap().insert(
            "commands/charts/plot-route.md".to_string(),
            FileStat {
                mtime: 300,
                size: 30,
            },
        );
        assert_eq!(
            prefilter(Some(&steady_receipt()), &walk, &[HarnessKind::ClaudeCode]),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_work_when_a_harness_has_no_receipt_entry() {
        // codex freshly installed: every stamp is clean, but it was never
        // reconciled, so stamps alone cannot prove it current.
        assert_eq!(
            prefilter(
                Some(&steady_receipt()),
                &steady_walk(),
                &[HarnessKind::ClaudeCode, HarnessKind::Codex]
            ),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_advisory_when_the_receipt_would_not_load() {
        assert_eq!(
            prefilter(None, &steady_walk(), &[HarnessKind::ClaudeCode]),
            SessionWork::Advisory
        );
    }

    // --- stat_file_sources vs scan_domain selection parity ---------------------

    /// Write `content` to `path`, creating its parent directories first - the
    /// same create-then-write idiom `reconcile`'s own fixtures use.
    fn fixture_file(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// A fixture tree that hits every edge rule `stat_file_sources` and
    /// `scan_domain` must agree on: a skill with a nested `scripts/` file, a
    /// skill directory missing `SKILL.md`, a hidden entry at the root of a
    /// kind and hidden entry inside a skill, a non-`.md` file among the
    /// commands, a nested command, a top-level markdown agent, a top-level
    /// Codex-dialect `.toml` agent, a nested agents file and an agents file of
    /// another extension (both never selected), and a name unsafe per
    /// `is_plain_component`. Returns the roots ready for both functions.
    fn selection_parity_roots(root: &Path) -> Vec<(ArtifactType, PathBuf)> {
        let skills = root.join("skills");
        // A real skill: SKILL.md plus a nested scripts/ file underneath it.
        fixture_file(&skills.join("good-skill/SKILL.md"), "# good skill");
        fixture_file(&skills.join("good-skill/scripts/run.sh"), "echo hi");
        // Hidden entry inside a skill directory: skipped at that depth.
        fixture_file(&skills.join("good-skill/.hidden-inner"), "nope");
        // A skill directory with no SKILL.md: selected by neither.
        fixture_file(&skills.join("no-skill-md/README.md"), "not a skill");
        // A hidden top-level directory under skills, SKILL.md and all: the
        // hidden check runs before anything looks inside it.
        fixture_file(&skills.join(".hidden-skill/SKILL.md"), "hidden");
        // A skill directory name unsafe per is_plain_component: selected by
        // neither, regardless of what is inside it. A colon is not a legal
        // file name character on Windows, so this edge only exists on unix -
        // which is also the only place such a name can reach the walkers.
        #[cfg(unix)]
        fixture_file(&skills.join("bad:name/SKILL.md"), "unsafe name");

        let commands = root.join("commands");
        fixture_file(&commands.join("top.md"), "# top command");
        // A nested command: commands select *.md at any depth.
        fixture_file(&commands.join("sub/nested.md"), "# nested command");
        // A non-.md file among the commands: selected by neither.
        fixture_file(&commands.join("notes.txt"), "not markdown");

        let agents = root.join("agents");
        // Agents select top-level *.md and *.toml (the Codex agent dialect),
        // nothing nested and no other extension.
        fixture_file(&agents.join("top.md"), "# top agent");
        fixture_file(&agents.join("codex-reviewer.toml"), "name = \"reviewer\"");
        fixture_file(&agents.join("notes.json"), "never selected");
        fixture_file(&agents.join("nested/inner.md"), "never selected");

        vec![
            (ArtifactType::Skills, skills),
            (ArtifactType::Commands, commands),
            (ArtifactType::Agents, agents),
        ]
    }

    #[test]
    fn stat_walk_selection_matches_scan_domain() {
        let dir = tempfile::tempdir().unwrap();
        let roots = selection_parity_roots(dir.path());

        let stat_keys: HashSet<String> = stat_file_sources(&roots).into_keys().collect();
        let (artifacts, _scan_notices) = scan_domain("harbor", &roots);
        let scan_keys: HashSet<String> = artifacts
            .files
            .iter()
            .map(|f| format!("{}/{}", f.kind.id(), f.rel))
            .collect();

        assert_eq!(stat_keys, scan_keys);

        // A key-set diff alone would still pass on two empty sets, so pin down
        // that the edge rules actually fired both ways rather than everything
        // being silently dropped.
        assert!(stat_keys.contains("skills/good-skill/SKILL.md"));
        assert!(stat_keys.contains("skills/good-skill/scripts/run.sh"));
        assert!(stat_keys.contains("commands/top.md"));
        assert!(stat_keys.contains("commands/sub/nested.md"));
        assert!(stat_keys.contains("agents/top.md"));
        assert!(stat_keys.contains("agents/codex-reviewer.toml"));
        assert_eq!(stat_keys.len(), 6);
    }
}
