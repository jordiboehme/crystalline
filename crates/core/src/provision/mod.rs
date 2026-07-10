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

pub use model::{
    ArtifactFile, DesiredFile, DesiredMcp, DesiredSet, DomainArtifacts, McpArtifact, desired_set,
    harness_supports, is_plain_component, resolve_source_roots, scan_domain,
};
pub use receipt::{
    DomainSources, HarnessState, InstalledFile, InstalledMcp, ProvisionReceipt, SourceStamp, load,
    plain_rel_key, receipt_path, save, sha256_hex,
};
pub use reconcile::{ActionStatus, ArtifactAction, McpOutcome, McpRunner, reconcile_harness};

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
/// [`ArtifactType::id`]. A kind with zero artifacts is simply absent from the
/// map, rather than present at zero.
fn count_artifacts(artifacts: &DomainArtifacts) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for file in &artifacts.files {
        *counts.entry(file.kind.id().to_string()).or_insert(0) += 1;
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
/// is no longer pending anything.
fn collect_pending(global: &GlobalConfig) -> Vec<PendingDomain> {
    let mut pending = Vec::new();
    for (name, entry) in &global.domains {
        if entry.provision.is_some() {
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
pub fn apply(
    global: &GlobalConfig,
    receipt_path: &Path,
    harnesses: &[HarnessKind],
    mcp: &mut dyn McpRunner,
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

    let pending = collect_pending(global);

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
pub fn status(
    global: &GlobalConfig,
    receipt_path: &Path,
    harnesses: &[HarnessKind],
) -> anyhow::Result<StatusReport> {
    let receipt = load(receipt_path).unwrap_or_default();

    let mut domains = Vec::new();
    let mut virtual_with_decision = Vec::new();
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
    }

    let mut harness_statuses = Vec::new();
    for &harness in harnesses {
        let empty = HarnessState::default();
        let state = receipt.harnesses.get(harness.id()).unwrap_or(&empty);
        let (edited, missing) = count_edited_and_missing(harness, state)?;
        harness_statuses.push(HarnessStatus {
            harness,
            installed_files: state.files.len(),
            installed_mcps: state.mcps.len(),
            edited,
            missing,
        });
    }

    let pending = collect_pending(global);

    Ok(StatusReport {
        domains,
        harnesses: harness_statuses,
        pending,
        virtual_with_decision,
    })
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
}
