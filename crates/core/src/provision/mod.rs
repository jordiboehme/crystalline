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
pub mod select;
pub(crate) mod translate;

pub use model::{
    ArtifactFile, ArtifactListing, ArtifactListingReport, DesiredFile, DesiredMcp, DesiredPayload,
    DesiredSet, DomainArtifacts, McpArtifact, artifact_key, desired_set, harness_supports,
    is_plain_component, list_artifacts, mcp_key, parse_artifact_key, resolve_source_roots,
    scan_domain,
};
pub use receipt::{
    DomainSources, HarnessState, InstalledFile, InstalledMcp, ProvisionReceipt, SourceStamp, load,
    plain_rel_key, receipt_path, save, sha256_hex,
};
pub use reconcile::{
    ActionStatus, ArtifactAction, DeferringMcpRunner, McpOutcome, McpRunner, reconcile_harness,
};
pub use select::{SelectionChange, SelectionOutcome, change_selection};
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

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{self, DomainEntry, GlobalConfig, Provision};
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

    HarnessKind::ALL
        .into_iter()
        .filter(|k| present.contains(k.id()))
        .collect()
}

/// Harnesses recorded in the install receipt at `path` whose install actually
/// wired session hooks (`installs[].parts.hooks == true`), the subset of
/// [`installed_harnesses`] that onboards itself at session start without the
/// MCP server having to. An install run with `--skip-hooks` records `hooks:
/// false` and is therefore absent here, and an uninstalled target has no
/// record at all.
///
/// Reads the same file just as shallowly and just as tolerantly: a missing,
/// unreadable or unparseable receipt is an empty list, never an error, and an
/// unrecognized harness id is skipped. The result is ordered by
/// [`HarnessKind`]'s own declaration order, so it never depends on the row
/// order `install` happened to leave behind.
pub fn harnesses_with_hooks(path: &Path) -> Vec<HarnessKind> {
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
        let hooks = install
            .get("parts")
            .and_then(|p| p.get("hooks"))
            .and_then(|h| h.as_bool())
            .unwrap_or(false);
        if !hooks {
            continue;
        }
        if let Some(id) = install.get("harness").and_then(|v| v.as_str())
            && let Some(kind) = HarnessKind::from_id(id)
        {
            present.insert(kind.id());
        }
    }

    HarnessKind::ALL
        .into_iter()
        .filter(|k| present.contains(k.id()))
        .collect()
}

// --- shared skills folders: one owner ---------------------------------------

/// The user skills folder `harness` writes provisioned skills into.
fn skills_folder(harness: HarnessKind) -> Option<PathBuf> {
    artifact_base(harness, ArtifactType::Skills).ok().flatten()
}

/// The installed harness that covers `harness` because it reads a skills
/// folder, other than its own write folder, that the other one writes (Cursor
/// reads `~/.claude/skills`, which Claude Code writes). The first match in
/// [`HarnessKind::ALL`] order wins.
fn read_cover(harness: HarnessKind, installed: &[HarnessKind]) -> Option<HarnessKind> {
    let own = skills_folder(harness)?;
    let reads: Vec<PathBuf> = harness
        .profile()
        .skills_reads
        .iter()
        .map(|r| r.resolve())
        .filter(|p| *p != own)
        .collect();
    if reads.is_empty() {
        return None;
    }
    HarnessKind::ALL.into_iter().find(|&other| {
        other != harness
            && installed.contains(&other)
            && skills_folder(other).is_some_and(|f| reads.contains(&f))
    })
}

/// The harness that owns `harness`'s provisioned skills, or `None` when
/// `harness` is the owner itself. Among `installed`, the owner of a skills
/// folder is the first harness in [`HarnessKind::ALL`] order that writes it
/// and is not covered by a read; every other installed harness that writes the
/// folder, or reads it and so is covered, provisions nothing of its own and
/// reports under the owner. Commands and agents never take part: the
/// harnesses that can be covered keep none.
fn skills_owner(harness: HarnessKind, installed: &[HarnessKind]) -> Option<HarnessKind> {
    if let Some(by) = read_cover(harness, installed) {
        return Some(by);
    }
    let own = skills_folder(harness)?;
    for other in HarnessKind::ALL {
        if !installed.contains(&other) {
            continue;
        }
        if other == harness {
            return None;
        }
        if skills_folder(other) == Some(own.clone()) && read_cover(other, installed).is_none() {
            return Some(other);
        }
    }
    None
}

/// Whether a receipt key names a skills file.
fn is_skills_key(key: &str) -> bool {
    key.starts_with("skills/")
}

/// Whether a receipt state records any skills file.
fn holds_skills(state: &HarnessState) -> bool {
    state.files.keys().any(|k| is_skills_key(k))
}

/// The ownership moves a receipt still owes, as `(owner, from)` pairs in
/// `ALL` order: for each installed harness that owns its skills folder, every
/// other harness whose row records skills in that same folder (a covered
/// installed one, or one no longer installed) and still holds `skills/` rows.
/// A folder's records belong to the folder, not the row.
fn handover_sources(
    receipt: &ProvisionReceipt,
    installed: &[HarnessKind],
) -> Vec<(HarnessKind, HarnessKind)> {
    let mut moves = Vec::new();
    for owner in HarnessKind::ALL {
        if !installed.contains(&owner) || skills_owner(owner, installed).is_some() {
            continue;
        }
        let Some(folder) = skills_folder(owner) else {
            continue;
        };
        for from in HarnessKind::ALL {
            if from == owner || skills_folder(from).as_ref() != Some(&folder) {
                continue;
            }
            if installed.contains(&from) && skills_owner(from, installed).is_none() {
                continue;
            }
            if receipt.harnesses.get(from.id()).is_some_and(holds_skills) {
                moves.push((owner, from));
            }
        }
    }
    moves
}

/// Carry out [`handover_sources`]: the departed or covered row's skills rows
/// replace the owner's older ones (it was the last writer), and only those
/// rows leave it - commands, agents and MCP records stay where they are.
fn hand_over_skills(receipt: &mut ProvisionReceipt, installed: &[HarnessKind]) {
    let mut cleared: Vec<HarnessKind> = Vec::new();
    for (owner, from) in handover_sources(receipt, installed) {
        let Some(st) = receipt.harnesses.get_mut(from.id()) else {
            continue;
        };
        let keys: Vec<String> = st
            .files
            .keys()
            .filter(|k| is_skills_key(k))
            .cloned()
            .collect();
        let moved: Vec<(String, InstalledFile)> = keys
            .into_iter()
            .filter_map(|k| st.files.remove(&k).map(|v| (k, v)))
            .collect();
        let state = receipt.harnesses.entry(owner.id().to_string()).or_default();
        if !cleared.contains(&owner) {
            cleared.push(owner);
            state.files.retain(|k, _| !is_skills_key(k));
        }
        state.files.extend(moved);
    }
}

/// Whether the receipt still holds skills rows that belong to somebody else
/// now: an installed owner's folder recorded under another row, or an
/// installed covered harness that still records skills.
fn skills_records_misplaced(receipt: &ProvisionReceipt, installed: &[HarnessKind]) -> bool {
    installed.iter().any(|&h| {
        skills_owner(h, installed).is_some()
            && receipt.harnesses.get(h.id()).is_some_and(holds_skills)
    }) || !handover_sources(receipt, installed).is_empty()
        || !stranded_skills(receipt, installed).is_empty()
}

/// Provisioned skills a departed harness left in a folder that no installed
/// owner writes but an installed harness still reads (Claude Code
/// uninstalled, Cursor still reading `~/.claude/skills`; or Codex
/// uninstalled while Cursor, covered by Claude Code, is the only writer of
/// `~/.agents/skills`), so they would keep loading with nobody to retire
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrandedSkills {
    /// The departed harness whose receipt row still records the files.
    pub harness: HarnessKind,
    /// The first installed harness, in [`HarnessKind::ALL`] order, that reads
    /// the folder.
    pub read_by: HarnessKind,
    /// The folder the files sit in.
    pub folder: PathBuf,
    /// How many skills files the row records there.
    pub files: usize,
}

/// Every departed harness's skills records that are stranded, in
/// [`HarnessKind::ALL`] order. Read on a receipt the hand-over already ran
/// on: a folder an installed owner writes has its records moved to that
/// owner instead. A folder nobody installed reads is left as it was.
fn stranded_skills(receipt: &ProvisionReceipt, installed: &[HarnessKind]) -> Vec<StrandedSkills> {
    let mut out = Vec::new();
    for gone in HarnessKind::ALL {
        if installed.contains(&gone) {
            continue;
        }
        let Some(state) = receipt.harnesses.get(gone.id()) else {
            continue;
        };
        let files = state.files.keys().filter(|k| is_skills_key(k)).count();
        if files == 0 {
            continue;
        }
        let Some(folder) = skills_folder(gone) else {
            continue;
        };
        // Only an installed owner takes the records over (the hand-over); a
        // writer that is itself read-covered provisions nothing of its own
        // (Cursor under Claude Code after Codex left ~/.agents/skills).
        if installed.iter().any(|&h| {
            skills_folder(h).as_ref() == Some(&folder) && skills_owner(h, installed).is_none()
        }) {
            continue;
        }
        let reader = HarnessKind::ALL.into_iter().find(|h| {
            installed.contains(h)
                && h.profile()
                    .skills_reads
                    .iter()
                    .any(|r| r.resolve() == folder)
        });
        if let Some(read_by) = reader {
            out.push(StrandedSkills {
                harness: gone,
                read_by,
                folder,
                files,
            });
        }
    }
    out
}

/// Retire every skills file `state` records for `harness` (reconcile against
/// an empty desired set: a clean copy is removed, an edited one renamed to
/// `.bak`), then drop those records. Commands, agents and MCP records stay.
fn retire_skills_rows(
    harness: HarnessKind,
    state: &mut HarnessState,
    notices: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Vec<ArtifactAction> {
    let mut actions = Vec::new();
    if !holds_skills(state) {
        return actions;
    }
    let mut skills_only = HarnessState {
        files: state
            .files
            .iter()
            .filter(|(k, _)| is_skills_key(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        ..HarnessState::default()
    };
    if let Ok((acts, rec_notices)) = reconcile_harness(
        harness,
        &DesiredSet::default(),
        &mut skills_only,
        &mut DeferringMcpRunner,
    ) {
        actions = acts;
        for notice in rec_notices {
            push_notice(notices, seen, notice);
        }
    }
    state.files.retain(|k, _| !is_skills_key(k));
    actions
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

// --- decision filter ----------------------------------------------------------

/// The part of one domain's scan that `decision` installs: every file and MCP
/// whose key [`Provision::selects`], given the artifact's optional mark.
///
/// The filter goes per artifact key, so every file that shares a key goes or
/// stays together: all files of a skill (they all carry the mark of its
/// `SKILL.md`) and an `.md` and `.toml` agent with the same stem (the `.toml`
/// one shares the mark of the `.md` one). `apply`, `status` and the session
/// path all filter through here, so what the session checks for drift is
/// exactly what `apply` installs.
pub fn filter_artifacts(artifacts: &DomainArtifacts, decision: &Provision) -> DomainArtifacts {
    DomainArtifacts {
        domain: artifacts.domain.clone(),
        files: artifacts
            .files
            .iter()
            .filter(|file| decision.selects(&artifact_key(file), file.optional))
            .cloned()
            .collect(),
        mcps: artifacts
            .mcps
            .iter()
            .filter(|mcp| decision.selects(&mcp_key(mcp), mcp.optional))
            .cloned()
            .collect(),
    }
}

/// The receipt's stamp key of a scanned file: `"<kind.id()>/<rel>"`.
fn stamp_key(file: &ArtifactFile) -> String {
    format!("{}/{}", file.kind.id(), file.rel)
}

/// Every artifact key `artifacts` marks optional, files and MCPs together.
fn optional_keys(artifacts: &DomainArtifacts) -> BTreeSet<String> {
    let files = artifacts
        .files
        .iter()
        .filter(|f| f.optional)
        .map(artifact_key);
    let mcps = artifacts.mcps.iter().filter(|m| m.optional).map(mcp_key);
    files.chain(mcps).collect()
}

/// One notice per artifact of `artifacts` that the author made optional
/// since the last apply and that `decision` therefore stops installing.
///
/// A key qualifies only when all of these hold:
/// - it is optional now and was not optional at the last apply (the
///   receipt's `optional` list for the domain);
/// - the decision selects it as a required artifact but not as an optional
///   one, so the mark is what takes it out (under `only` or an `except` name
///   the mark changes nothing);
/// - the last apply installed it from this domain: a harness row records a
///   file of it (or, for an MCP, the server) for the domain. A stamp alone is
///   not enough, since every scanned file is stamped, skipped ones too
///   (an artifact left out by `except` and let in again by `allow` was
///   never installed). A fresh opt-in or a brand-new optional artifact was
///   never installed either, so neither gets a notice.
///
/// The caller writes the new `optional` list afterwards, so the notice fires
/// once.
fn newly_optional_notices(
    artifacts: &DomainArtifacts,
    decision: &Provision,
    receipt: &ProvisionReceipt,
) -> Vec<String> {
    let domain = artifacts.domain.as_str();
    let Some(stamped) = receipt.sources.get(domain) else {
        return Vec::new();
    };
    let mut known: BTreeSet<String> = BTreeSet::new();
    for file in artifacts.files.iter().filter(|f| f.optional) {
        if stamped.files.contains_key(&stamp_key(file)) && installed_from(file, domain, receipt) {
            known.insert(artifact_key(file));
        }
    }
    for mcp in artifacts.mcps.iter().filter(|m| m.optional) {
        let recorded = receipt.harnesses.values().any(|state| {
            state
                .mcps
                .get(&mcp.name)
                .is_some_and(|row| row.domain == domain)
        });
        if recorded {
            known.insert(mcp_key(mcp));
        }
    }
    known
        .into_iter()
        .filter(|key| {
            !stamped.optional.contains(key)
                && decision.selects(key, false)
                && !decision.selects(key, true)
        })
        .map(|key| newly_optional_notice(domain, &key))
        .collect()
}

/// Whether a harness row records `file` as installed from `domain`. Receipt
/// rows are keyed by the harness's own path (Codex flattens commands and
/// renders agents to `.toml`), so the file is projected for each recorded
/// harness and that harness's row looked up. Only called for the few
/// optional files the notice considers.
fn installed_from(file: &ArtifactFile, domain: &str, receipt: &ProvisionReceipt) -> bool {
    receipt.harnesses.iter().any(|(id, state)| {
        let Some(harness) = HarnessKind::from_id(id) else {
            return false;
        };
        let translate::Translated::Emit { rel, .. } = translate::translate_file(harness, file)
        else {
            return false;
        };
        state
            .files
            .get(&format!("{}/{}", file.kind.id(), rel))
            .is_some_and(|row| row.domain == domain)
    })
}

/// The notice for an artifact the author made optional since the last apply,
/// which the decision no longer installs.
fn newly_optional_notice(domain: &str, key: &str) -> String {
    format!(
        "`{key}` from the `{domain}` domain is now optional, so it is no longer installed - run `crystalline provision take {domain} {key}` to keep it."
    )
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
    /// After them comes each departed harness whose stranded skills were
    /// retired (see [`StrandedSkills`]), with those removals.
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
/// Opted-in domains (a decision that [`Provision::includes_any`]) are scanned
/// in `global`'s own declaration order - the collision-precedence contract
/// [`desired_set`] documents. Each scan goes through [`filter_artifacts`]
/// before it reaches the desired sets, so an artifact the decision leaves out
/// is retired like a denied domain's: deleted when clean, kept as a `.bak`
/// when edited, an MCP deregistered. An artifact the author made optional
/// since the last apply raises one notice when that takes it out (see
/// [`DomainSources::optional`]). A virtual domain can never ship artifacts (its MANIFEST lives
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
/// domain no longer opted in loses its stamps entirely). The stamps cover
/// every scanned file, also those the decision leaves out, and sit beside the
/// decision's fingerprint and the domain's optional keys. A receipt that
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

    hand_over_skills(&mut receipt, harnesses);

    let mut any_opted_in = false;
    // Each included domain's whole scan with its decision: the stamps, the
    // fingerprint and the optional list come from these, and the filtered
    // copy below feeds the desired sets.
    let mut domain_artifacts: Vec<(DomainArtifacts, &Provision)> = Vec::new();
    for (name, entry) in &global.domains {
        let Some(decision) = entry.provision.as_ref().filter(|p| p.includes_any()) else {
            continue;
        };
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
        domain_artifacts.push((artifacts, decision));
    }
    let selected: Vec<DomainArtifacts> = domain_artifacts
        .iter()
        .map(|(artifacts, decision)| filter_artifacts(artifacts, decision))
        .collect();

    // Display order only: the reconcile above and below keeps the config's
    // declaration order for its collision precedence.
    let mut pending = collect_pending(global, env_domains);
    pending.sort_by_cached_key(|p| (p.domain.to_lowercase(), p.domain.clone()));

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

    // Read before any harness reconciles: the reconcile below drops the MCP
    // rows of a server that just became optional.
    for (artifacts, decision) in &domain_artifacts {
        for notice in newly_optional_notices(artifacts, decision, &receipt) {
            push_notice(&mut notices, &mut seen, notice);
        }
    }

    let mut harness_results = Vec::new();
    for &harness in harnesses {
        let (desired, ds_notices) = desired_set(harness, &selected);
        for notice in ds_notices {
            push_notice(&mut notices, &mut seen, notice);
        }

        let state = receipt
            .harnesses
            .entry(harness.id().to_string())
            .or_default();
        // A harness whose skills folder another installed harness owns
        // reconciles nothing of its own. Rows in the owner's folder were
        // handed over above; what is left is a copy in a folder nobody else
        // writes (Cursor under Claude Code), retired like a dropped
        // artifact before the records go.
        if skills_owner(harness, harnesses).is_some() {
            let actions = retire_skills_rows(harness, state, &mut notices, &mut seen);
            harness_results.push((harness, actions));
            continue;
        }
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

    // A departed harness's skills in a folder an installed one still reads
    // and nobody writes: retired like a dropped artifact, so a deny reaches
    // them, while the reader gets its own copy in its own folder above.
    for stranded in stranded_skills(&receipt, harnesses) {
        let Some(state) = receipt.harnesses.get_mut(stranded.harness.id()) else {
            continue;
        };
        let actions = retire_skills_rows(stranded.harness, state, &mut notices, &mut seen);
        if state.files.is_empty() && state.mcps.is_empty() {
            receipt.harnesses.remove(stranded.harness.id());
        }
        harness_results.push((stranded.harness, actions));
    }

    // Every scanned file is stamped, also those the decision leaves out: the
    // session walk does not know the decision (see `DomainSources`).
    let mut sources = BTreeMap::new();
    for (artifacts, decision) in &domain_artifacts {
        let mut files = BTreeMap::new();
        for file in &artifacts.files {
            if let Some(stamp) = stamp_source(&file.source, &file.sha256) {
                files.insert(stamp_key(file), stamp);
            }
        }
        sources.insert(
            artifacts.domain.clone(),
            DomainSources {
                files,
                decision: decision.fingerprint(),
                optional: optional_keys(artifacts).into_iter().collect(),
            },
        );
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

/// Where a domain sits in the status listing: allowed (with `only` among
/// them), undecided and denied domains that declare provisioning first, then the domains that declare
/// none, then the virtual ones. Within a group the listing goes by name,
/// case-insensitively, the same comparison `list_domains` uses.
fn status_group(d: &DomainStatus) -> u8 {
    if d.is_virtual {
        return 4;
    }
    if !d.declares {
        return 3;
    }
    match d.decision {
        Decision::Allowed | Decision::Only => 0,
        Decision::Undecided => 1,
        Decision::Denied => 2,
    }
}

/// Which side of a provisioning decision a domain is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// `provision: true`, or an `except`/`also` map - everything the domain
    /// ships is provisioned, apart from the names left out and the optional
    /// artifacts not taken.
    Allowed,
    /// `provision: {only: [...]}` - only the named artifacts are provisioned.
    Only,
    /// `provision: false` - this domain's artifacts are never provisioned.
    Denied,
    /// `provision` absent - awaiting a decision.
    Undecided,
}

impl Decision {
    /// The decision a [`DomainEntry`]'s own `provision` field carries.
    fn from_entry(entry: &DomainEntry) -> Decision {
        match &entry.provision {
            Some(Provision::All { .. }) => Decision::Allowed,
            Some(Provision::Only(_)) => Decision::Only,
            Some(Provision::Deny) => Decision::Denied,
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
    /// The lists of the decision, `None` for plain `true`, `false` and
    /// undecided.
    pub selection: Option<Provision>,
    /// Every artifact the domain ships, one row per key in kind order and
    /// then by key, with whether this machine's decision installs it. Empty
    /// for a virtual domain and a domain that declares no provisioning.
    pub artifacts: Vec<ArtifactState>,
    /// Names in the decision's lists the domain does not ship now. Only for
    /// kinds whose root exists on disk: a missing root makes no name stale.
    pub stale: Vec<String>,
    /// Names in the decision's lists that are not `<kind>/<name>`.
    pub malformed: Vec<String>,
}

/// One artifact a domain ships, as [`DomainStatus::artifacts`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactState {
    /// The provisioning key, `<kind>/<name>`.
    pub key: String,
    /// The artifact's kind.
    pub kind: ArtifactType,
    /// The key without its `<kind>/` prefix.
    pub name: String,
    /// Whether the author marked it `metadata: { provision: optional }`.
    pub optional: bool,
    /// Whether this machine's decision selects it. This is not whether every
    /// harness installs it (a harness may have no surface for the kind); the
    /// harness rows say what is installed. Always `false` for an undecided or
    /// denied domain.
    pub on: bool,
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
    /// The harness that owns the skills folder this one shares or reads,
    /// when another installed harness does. Such a harness provisions
    /// nothing of its own, so every count above is zero and drift, orphans
    /// and edits are reported once, under the owner.
    pub covered_by: Option<HarnessKind>,
}

/// A read-only snapshot of every domain's provisioning decision and every
/// requested harness's installed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    /// Every registered domain: allowed, undecided and denied domains that
    /// declare provisioning, then those that declare none, then virtual
    /// domains, each group by name (case-insensitive, exact name as the
    /// tie-break). `pending` and `virtual_with_decision` go by the same name
    /// key.
    pub domains: Vec<DomainStatus>,
    /// Every requested harness, in the order given.
    pub harnesses: Vec<HarnessStatus>,
    /// Departed harnesses' skills an installed harness still reads and the
    /// next `apply` retires.
    pub stranded: Vec<StrandedSkills>,
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
    let mut receipt = load(receipt_path).unwrap_or_default();
    // Read as `apply` would leave it, without writing: the records of a
    // folder already belong to its owner.
    hand_over_skills(&mut receipt, harnesses);

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
                selection: selection_of(entry),
                artifacts: Vec::new(),
                stale: Vec::new(),
                malformed: Vec::new(),
            });
            continue;
        }

        let manifest = read_manifest(entry);
        let section = manifest.as_ref().and_then(|m| m.provisioning());
        let declares = section.is_some();
        let parse_problems = section.map(|s| s.problems.len()).unwrap_or(0);
        let roots = resolve_source_roots(name, entry);
        let (artifacts, _scan_notices) = scan_domain(name, &roots);
        let (rows, stale, malformed) = if declares {
            let rows = artifact_states(&artifacts, entry.provision.as_ref());
            let (stale, malformed) = stale_and_malformed(
                entry.provision.as_ref(),
                &rows,
                &model::present_kinds(&roots),
            );
            (rows, stale, malformed)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        domains.push(DomainStatus {
            domain: name.clone(),
            is_virtual: false,
            decision,
            declares,
            counts: count_artifacts(&artifacts),
            parse_problems,
            selection: selection_of(entry),
            artifacts: rows,
            stale,
            malformed,
        });
        // Drift and orphans are counted against what `apply` would install:
        // the decision's part of the scan, not the whole of it.
        if let Some(provision) = entry.provision.as_ref().filter(|p| p.includes_any()) {
            opted_artifacts.push(filter_artifacts(&artifacts, provision));
        }
    }

    let mut harness_statuses = Vec::new();
    for &harness in harnesses {
        if let Some(owner) = skills_owner(harness, harnesses) {
            harness_statuses.push(HarnessStatus {
                harness,
                installed_files: 0,
                installed_mcps: 0,
                edited: 0,
                missing: 0,
                drift: 0,
                orphaned: 0,
                covered_by: Some(owner),
            });
            continue;
        }
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
            covered_by: None,
        });
    }

    let mut pending = collect_pending(global, env_domains);

    // The loop above walked the config's declaration order on purpose (it
    // feeds `opted_artifacts`); only the finished lists are put in the
    // reading order, so the report reads the same on every run.
    domains.sort_by_cached_key(|d| (status_group(d), d.domain.to_lowercase(), d.domain.clone()));
    pending.sort_by_cached_key(|p| (p.domain.to_lowercase(), p.domain.clone()));
    virtual_with_decision.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));

    Ok(StatusReport {
        domains,
        harnesses: harness_statuses,
        stranded: stranded_skills(&receipt, harnesses),
        pending,
        virtual_with_decision,
    })
}

/// The decision's lists for [`DomainStatus::selection`]: the decision when
/// it is a map, `None` for plain `true`, `false` and undecided.
fn selection_of(entry: &DomainEntry) -> Option<Provision> {
    entry.provision.clone().filter(|p| !p.is_plain_bool())
}

/// One row per artifact key of a full (unfiltered) scan, in kind order and
/// then by key, with whether `decision` selects it.
///
/// Every file of a key shares one row: all files of a skill, and an `.md`
/// and `.toml` agent with the same stem (the row is optional when any of its
/// files is). Two MCP configs naming the same server give one row, the first
/// in scan order, as [`list_artifacts`] does.
fn artifact_states(
    artifacts: &DomainArtifacts,
    decision: Option<&Provision>,
) -> Vec<ArtifactState> {
    let mut rows: BTreeMap<(u8, String), (ArtifactType, bool)> = BTreeMap::new();
    for file in &artifacts.files {
        let slot = rows
            .entry((model::kind_order(file.kind), artifact_key(file)))
            .or_insert((file.kind, false));
        slot.1 |= file.optional;
    }
    for mcp in &artifacts.mcps {
        rows.entry((model::kind_order(ArtifactType::Mcps), mcp_key(mcp)))
            .or_insert((ArtifactType::Mcps, mcp.optional));
    }
    rows.into_iter()
        .map(|((_, key), (kind, optional))| {
            let name = key
                .split_once('/')
                .map_or(key.as_str(), |(_, name)| name)
                .to_string();
            let on = decision.is_some_and(|d| d.selects(&key, optional));
            ArtifactState {
                key,
                kind,
                name,
                optional,
                on,
            }
        })
        .collect()
}

/// The names in `decision`'s lists that are stale (well formed, of a kind
/// whose root exists on disk, but not shipped now) and those that are
/// malformed (not `<kind>/<name>`), each in list order without repeats.
fn stale_and_malformed(
    decision: Option<&Provision>,
    rows: &[ArtifactState],
    present: &[ArtifactType],
) -> (Vec<String>, Vec<String>) {
    let mut stale = Vec::new();
    let mut malformed = Vec::new();
    let Some(decision) = decision else {
        return (stale, malformed);
    };
    let mut seen = HashSet::new();
    for name in decision.names() {
        if !seen.insert(name) {
            continue;
        }
        match parse_artifact_key(name) {
            None => malformed.push(name.to_string()),
            Some((kind, _)) => {
                if present.contains(&kind) && !rows.iter().any(|row| row.key == name) {
                    stale.push(name.to_string());
                }
            }
        }
    }
    (stale, malformed)
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
    let opted: Vec<(&String, &DomainEntry, &Provision)> = global
        .domains
        .iter()
        .filter(|(_, e)| !e.is_virtual())
        .filter_map(|(name, e)| {
            let decision = e.provision.as_ref().filter(|p| p.includes_any())?;
            Some((name, e, decision))
        })
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
    // The file walk is not filtered: `apply` stamps every scanned file, also
    // those the decision leaves out. The MCP scan is filtered by the same
    // decision `apply` uses, or a skipped server would read as drift on every
    // session.
    let mut walked: BTreeMap<String, BTreeMap<String, FileStat>> = BTreeMap::new();
    let mut fingerprints: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut mcp_artifacts: Vec<DomainArtifacts> = Vec::new();
    for (name, entry, decision) in &opted {
        let roots = resolve_source_roots(name, entry);
        let (mcp_roots, file_roots): (Vec<_>, Vec<_>) = roots
            .into_iter()
            .partition(|(kind, _)| *kind == ArtifactType::Mcps);
        walked.insert((*name).clone(), stat_file_sources(&file_roots));
        fingerprints.insert((*name).clone(), decision.fingerprint());
        let (arts, _scan_notices) = scan_domain(name, &mcp_roots);
        mcp_artifacts.push(filter_artifacts(&arts, decision));
    }

    let work = matches!(
        prefilter(Some(&receipt), &walked, &fingerprints, harnesses),
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
/// harness current, the same reason `apply`'s doc comment gives) or skills
/// records that sit under the wrong row: a folder's records under a harness
/// that no longer owns it, or a departed harness's copies an installed one
/// still reads ([`StrandedSkills`], the cover of a read-covered harness left),
/// or a domain whose decision fingerprint in `fingerprints` differs from the
/// one stamped at the last apply (a hand-edited pick; a receipt without a
/// fingerprint matches plain `true`). MCP drift is judged separately by
/// [`mcp_drift`], since MCP configs carry no source stamp.
fn prefilter(
    receipt: Option<&ProvisionReceipt>,
    walked: &BTreeMap<String, BTreeMap<String, FileStat>>,
    fingerprints: &BTreeMap<String, Option<String>>,
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
        let fingerprint = fingerprints.get(domain).cloned().flatten();
        if stamped.decision != fingerprint {
            return SessionWork::Work;
        }
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

    if skills_records_misplaced(receipt, harnesses) {
        return SessionWork::Work;
    }

    SessionWork::NoWork
}

/// Whether any requested harness's recorded MCP servers differ from what the
/// opted-in domains would provision right now. MCP configs carry no source
/// stamp - the receipt's `sources` tracks file artifacts only - so this is the
/// one place the session path rescans, which stays cheap because MCP configs
/// are always few and small. `mcp_artifacts` holds each opted-in domain's
/// freshly scanned MCP set (its files left empty), already passed through
/// [`filter_artifacts`] with the domain's decision; the comparison mirrors what
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
/// counts and how to decide - ask the user for all, none or a pick, then apply
/// it with the `provision` tool or the CLI. Counts only: no frontmatter is
/// read here, so the hook keeps its latency budget; `provision status` lists
/// each artifact and its optional mark. Empty when nothing is pending.
fn render_pending(pending: &[PendingDomain]) -> Vec<String> {
    pending
        .iter()
        .map(|p| {
            format!(
                "[crystalline] The `{}` domain ships artifacts to provision ({}) but has no decision yet - ask the user whether to install all, none or only some of them, then apply the answer with the `provision` tool (allow, deny, take or skip) or `crystalline provision allow {}`.",
                p.domain,
                render_counts(&p.counts),
                p.domain
            )
        })
        .collect()
}

/// A domain's artifact counts as `kind: n` pairs in [`ArtifactType`]
/// declaration order, for a pending line - for example
/// `skills: 2, commands: 1`. An id that names no kind goes last.
fn render_counts(counts: &BTreeMap<String, usize>) -> String {
    const ORDER: [ArtifactType; 4] = [
        ArtifactType::Skills,
        ArtifactType::Commands,
        ArtifactType::Agents,
        ArtifactType::Mcps,
    ];
    let mut pairs: Vec<(&String, &usize)> = counts.iter().collect();
    pairs.sort_by_key(|(kind, _)| {
        ORDER
            .iter()
            .position(|k| k.id() == kind.as_str())
            .unwrap_or(ORDER.len())
    });
    pairs
        .into_iter()
        .map(|(kind, n)| format!("{kind}: {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The summary line for a session that reconciled `changes` file artifacts.
fn reconcile_summary_notice(changes: usize) -> String {
    format!(
        "[crystalline] Refreshed {} from your opted-in domains for this session.",
        crate::text::plural(changes, "provisioned artifact", "provisioned artifacts")
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

    // --- harnesses_with_hooks -------------------------------------------------

    #[test]
    fn a_missing_or_corrupt_receipt_is_no_hooked_harnesses() {
        let dir = tempfile::tempdir().unwrap();
        assert!(harnesses_with_hooks(&dir.path().join("missing.json")).is_empty());
        let path = dir.path().join("installs.json");
        std::fs::write(&path, "{ nope").unwrap();
        assert!(harnesses_with_hooks(&path).is_empty());
    }

    #[test]
    fn only_records_that_actually_wired_hooks_count_as_hooked() {
        // codex was installed with --skip-hooks and copilot's record carries no
        // parts block at all, so neither onboards itself at session start;
        // claude-code did wire hooks, at project scope only, and counts.
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
                        "version": "0.11.0",
                        "parts": {"mcp": true, "hooks": false, "skills": true},
                        "skills": []
                    },
                    {
                        "harness": "copilot",
                        "scope": "user",
                        "version": "0.11.0",
                        "skills": []
                    },
                    {
                        "harness": "claude-code",
                        "scope": "project",
                        "project_path": "/repo",
                        "version": "0.11.0",
                        "parts": {"mcp": true, "hooks": true, "skills": true},
                        "skills": []
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            installed_harnesses(&path),
            vec![
                HarnessKind::ClaudeCode,
                HarnessKind::Codex,
                HarnessKind::Copilot
            ],
            "all three are installed"
        );
        assert_eq!(
            harnesses_with_hooks(&path),
            vec![HarnessKind::ClaudeCode],
            "only the one whose install wired hooks"
        );
    }

    #[test]
    fn an_uninstalled_harness_leaves_no_hooked_record() {
        // `uninstall` removes the row outright, so the receipt that remains
        // simply does not mention the harness.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installs.json");
        std::fs::write(&path, r#"{ "format": 1, "installs": [] }"#).unwrap();
        assert!(harnesses_with_hooks(&path).is_empty());
    }

    /// Both lists follow `HarnessKind::ALL`, so the new harnesses are read
    /// from the receipt and come out in declaration order, whatever row
    /// order install left behind.
    #[test]
    fn the_new_harnesses_are_listed_in_declaration_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installs.json");
        std::fs::write(
            &path,
            r#"{
                "format": 1,
                "installs": [
                    {
                        "harness": "qwen",
                        "scope": "user",
                        "version": "0.22.1",
                        "parts": {"mcp": true, "hooks": true, "skills": true},
                        "skills": []
                    },
                    {
                        "harness": "claude-code",
                        "scope": "user",
                        "version": "0.22.1",
                        "parts": {"mcp": true, "hooks": false, "skills": true},
                        "skills": []
                    },
                    {
                        "harness": "cursor",
                        "scope": "user",
                        "version": "0.22.1",
                        "parts": {"mcp": true, "hooks": true, "skills": true},
                        "skills": []
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            installed_harnesses(&path),
            vec![
                HarnessKind::ClaudeCode,
                HarnessKind::Cursor,
                HarnessKind::Qwen
            ]
        );
        assert_eq!(
            harnesses_with_hooks(&path),
            vec![HarnessKind::Cursor, HarnessKind::Qwen],
            "the same order, only the rows that wired hooks"
        );
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
                ..DomainSources::default()
            },
        );
        receipt
            .harnesses
            .insert("claude-code".to_string(), HarnessState::default());
        receipt
    }

    /// The config's fingerprints when every walked domain says plain `true`:
    /// no entry reads as `None`, the fingerprint of plain `true`.
    fn no_picks() -> BTreeMap<String, Option<String>> {
        BTreeMap::new()
    }

    #[test]
    fn prefilter_work_when_the_decision_fingerprint_changed() {
        let skip = Provision::All {
            except: vec!["skills/tide-tables".to_string()],
            also: Vec::new(),
        };
        let picks = BTreeMap::from([("harbor".to_string(), skip.fingerprint())]);
        assert_eq!(
            prefilter(
                Some(&steady_receipt()),
                &steady_walk(),
                &picks,
                &[HarnessKind::ClaudeCode]
            ),
            SessionWork::Work,
            "a hand-edited pick differs from the stamped plain true"
        );

        let mut receipt = steady_receipt();
        receipt.sources.get_mut("harbor").unwrap().decision = skip.fingerprint();
        assert_eq!(
            prefilter(
                Some(&receipt),
                &steady_walk(),
                &picks,
                &[HarnessKind::ClaudeCode]
            ),
            SessionWork::NoWork,
            "the stamped pick matches"
        );
        assert_eq!(
            prefilter(
                Some(&receipt),
                &steady_walk(),
                &no_picks(),
                &[HarnessKind::ClaudeCode]
            ),
            SessionWork::Work,
            "going back to plain true is a change too"
        );
    }

    // --- filter_artifacts ---------------------------------------------------------

    fn file(kind: ArtifactType, rel: &str, optional: bool) -> ArtifactFile {
        ArtifactFile {
            kind,
            rel: rel.to_string(),
            source: PathBuf::from(rel),
            sha256: String::new(),
            optional,
        }
    }

    fn mcp(name: &str, optional: bool) -> McpArtifact {
        McpArtifact {
            name: name.to_string(),
            server_json: "{}".to_string(),
            sha256: String::new(),
            optional,
        }
    }

    /// A domain with a two-file skill, an `.md` and `.toml` agent pair, a
    /// nested command and two MCP servers, one of them optional.
    fn mixed_domain() -> DomainArtifacts {
        DomainArtifacts {
            domain: "harbor".to_string(),
            files: vec![
                file(ArtifactType::Skills, "tide-tables/SKILL.md", false),
                file(ArtifactType::Skills, "tide-tables/scripts/chart.sh", false),
                file(ArtifactType::Skills, "storm-warnings/SKILL.md", true),
                file(ArtifactType::Commands, "charts/plot-route.md", false),
                file(ArtifactType::Agents, "quartermaster.md", false),
                file(ArtifactType::Agents, "quartermaster.toml", false),
            ],
            mcps: vec![mcp("lighthouse", false), mcp("foghorn", true)],
        }
    }

    fn kept(artifacts: &DomainArtifacts) -> Vec<String> {
        let files = artifacts
            .files
            .iter()
            .map(|f| format!("{}/{}", f.kind.id(), f.rel));
        let mcps = artifacts.mcps.iter().map(mcp_key);
        files.chain(mcps).collect()
    }

    #[test]
    fn filter_artifacts_keeps_or_drops_every_file_of_one_key_together() {
        let domain = mixed_domain();
        let except = Provision::All {
            except: vec![
                "skills/tide-tables".to_string(),
                "agents/quartermaster".to_string(),
            ],
            also: vec!["mcps/foghorn".to_string()],
        };
        assert_eq!(
            kept(&filter_artifacts(&domain, &except)),
            vec![
                "commands/charts/plot-route.md",
                "mcps/lighthouse",
                "mcps/foghorn"
            ]
        );

        let only = Provision::Only(vec![
            "skills/tide-tables".to_string(),
            "agents/quartermaster".to_string(),
            "skills/storm-warnings".to_string(),
        ]);
        assert_eq!(
            kept(&filter_artifacts(&domain, &only)),
            vec![
                "skills/tide-tables/SKILL.md",
                "skills/tide-tables/scripts/chart.sh",
                "skills/storm-warnings/SKILL.md",
                "agents/quartermaster.md",
                "agents/quartermaster.toml"
            ]
        );
    }

    #[test]
    fn filter_artifacts_under_plain_true_leaves_out_only_optional_artifacts() {
        let domain = mixed_domain();
        let filtered = filter_artifacts(&domain, &Provision::all());
        assert_eq!(filtered.domain, "harbor");
        assert_eq!(
            kept(&filtered),
            vec![
                "skills/tide-tables/SKILL.md",
                "skills/tide-tables/scripts/chart.sh",
                "commands/charts/plot-route.md",
                "agents/quartermaster.md",
                "agents/quartermaster.toml",
                "mcps/lighthouse"
            ]
        );
        assert!(kept(&filter_artifacts(&domain, &Provision::Deny)).is_empty());
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
                &no_picks(),
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
            prefilter(
                Some(&steady_receipt()),
                &walk,
                &no_picks(),
                &[HarnessKind::ClaudeCode]
            ),
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
            prefilter(
                Some(&steady_receipt()),
                &walk,
                &no_picks(),
                &[HarnessKind::ClaudeCode]
            ),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_work_on_new_opted_in_domain() {
        let mut walk = steady_walk();
        walk.insert("cove".to_string(), BTreeMap::new());
        assert_eq!(
            prefilter(
                Some(&steady_receipt()),
                &walk,
                &no_picks(),
                &[HarnessKind::ClaudeCode]
            ),
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
            prefilter(
                Some(&receipt),
                &steady_walk(),
                &no_picks(),
                &[HarnessKind::ClaudeCode]
            ),
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
            prefilter(
                Some(&steady_receipt()),
                &walk,
                &no_picks(),
                &[HarnessKind::ClaudeCode]
            ),
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
                &no_picks(),
                &[HarnessKind::ClaudeCode, HarnessKind::Codex]
            ),
            SessionWork::Work
        );
    }

    #[test]
    fn prefilter_work_when_a_departed_cover_left_skills_an_installed_harness_reads() {
        // Claude Code covered Cursor and left: its copy in ~/.claude/skills is
        // still read by Cursor and nobody writes that folder any more.
        let mut receipt = steady_receipt();
        receipt
            .harnesses
            .get_mut("claude-code")
            .unwrap()
            .files
            .insert(
                "skills/tide-tables/SKILL.md".to_string(),
                InstalledFile {
                    domain: "harbor".to_string(),
                    sha256: String::new(),
                },
            );
        receipt
            .harnesses
            .insert("cursor".to_string(), HarnessState::default());
        receipt
            .harnesses
            .insert("codex".to_string(), HarnessState::default());
        assert_eq!(
            prefilter(
                Some(&receipt),
                &steady_walk(),
                &no_picks(),
                &[HarnessKind::Cursor]
            ),
            SessionWork::Work
        );
        // Codex never reads ~/.claude/skills: nothing is stranded for it.
        assert_eq!(
            prefilter(
                Some(&receipt),
                &steady_walk(),
                &no_picks(),
                &[HarnessKind::Codex]
            ),
            SessionWork::NoWork
        );
    }

    #[test]
    fn prefilter_advisory_when_the_receipt_would_not_load() {
        assert_eq!(
            prefilter(
                None,
                &steady_walk(),
                &no_picks(),
                &[HarnessKind::ClaudeCode]
            ),
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

    // --- status order ------------------------------------------------------------

    /// A file domain whose MANIFEST declares `agents: agents` and ships one
    /// agent file `<agent>.md` holding `content`.
    fn shipping_domain(agent: &str, content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path(), "## Provisioning\n\n- agents: agents\n");
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(
            dir.path().join("agents").join(format!("{agent}.md")),
            content,
        )
        .unwrap();
        dir
    }

    fn with_decision(mut entry: DomainEntry, decision: Option<bool>) -> DomainEntry {
        entry.provision = decision.map(Provision::from);
        entry
    }

    #[test]
    fn status_lists_domains_grouped_and_sorted_whatever_the_registration_order() {
        let shipping = |agent: &str| shipping_domain(agent, "# Agent\n");
        let (b_up, b_low, beta, alpha_up) = (
            shipping("a1"),
            shipping("a2"),
            shipping("a3"),
            shipping("a4"),
        );
        let (delta, charlie) = (shipping("a5"), shipping("a6"));
        let (bravo, alpha_low) = (shipping("a7"), shipping("a8"));
        let plain = tempfile::tempdir().unwrap();
        write_manifest(plain.path(), "");

        // Registered in an order that matches no group and no sort.
        let mut global = GlobalConfig::default();
        let d = &mut global.domains;
        d.insert(
            "zeta".into(),
            with_decision(DomainEntry::virtual_domain(), Some(false)),
        );
        d.insert(
            "Bravo".into(),
            with_decision(DomainEntry::file(bravo.path()), Some(false)),
        );
        d.insert("alpha-plain".into(), DomainEntry::file(plain.path()));
        d.insert("delta".into(), DomainEntry::file(delta.path()));
        d.insert(
            "b".into(),
            with_decision(DomainEntry::file(b_low.path()), Some(true)),
        );
        d.insert("charlie".into(), DomainEntry::file(charlie.path()));
        d.insert(
            "beta".into(),
            with_decision(DomainEntry::file(beta.path()), Some(true)),
        );
        d.insert(
            "Echo".into(),
            with_decision(DomainEntry::virtual_domain(), Some(true)),
        );
        d.insert(
            "alpha".into(),
            with_decision(DomainEntry::file(alpha_low.path()), Some(false)),
        );
        d.insert(
            "B".into(),
            with_decision(DomainEntry::file(b_up.path()), Some(true)),
        );
        d.insert(
            "Alpha".into(),
            with_decision(DomainEntry::file(alpha_up.path()), Some(true)),
        );

        let receipts = tempfile::tempdir().unwrap();
        let report = status(
            &global,
            &receipts.path().join("receipt.json"),
            &[],
            &HashSet::new(),
        )
        .unwrap();

        let names: Vec<&str> = report.domains.iter().map(|d| d.domain.as_str()).collect();
        assert_eq!(
            names,
            [
                // allowed
                "Alpha",
                "B",
                "b",
                "beta",
                // undecided
                "charlie",
                "delta",
                // denied
                "alpha",
                "Bravo",
                // declares no provisioning
                "alpha-plain",
                // virtual
                "Echo",
                "zeta",
            ]
        );
        let pending: Vec<&str> = report.pending.iter().map(|p| p.domain.as_str()).collect();
        assert_eq!(pending, ["charlie", "delta"]);
        assert_eq!(report.virtual_with_decision, ["Echo", "zeta"]);
    }

    #[test]
    fn apply_lists_the_domains_awaiting_a_decision_in_name_order() {
        let (zulu, mike, echo) = (
            shipping_domain("a1", "# A\n"),
            shipping_domain("a2", "# A\n"),
            shipping_domain("a3", "# A\n"),
        );
        let mut global = GlobalConfig::default();
        global
            .domains
            .insert("zulu".into(), DomainEntry::file(zulu.path()));
        global
            .domains
            .insert("mike".into(), DomainEntry::file(mike.path()));
        global
            .domains
            .insert("Echo".into(), DomainEntry::file(echo.path()));

        let dir = tempfile::tempdir().unwrap();
        let report = apply(
            &global,
            &dir.path().join("receipt.json"),
            &[],
            &mut DeferringMcpRunner,
            &HashSet::new(),
        )
        .unwrap();
        let pending: Vec<&str> = report.pending.iter().map(|p| p.domain.as_str()).collect();
        assert_eq!(pending, ["Echo", "mike", "zulu"]);
    }

    #[test]
    fn status_keeps_the_declaration_order_for_the_harness_projection() {
        // Two allowed domains ship the same agent name with different bytes.
        // The first-declared one ("zulu") must still win the collision, even
        // though the report lists "alpha" before it: a receipt recording the
        // first-declared bytes shows no drift.
        let zulu = shipping_domain("shared", "# From zulu\n");
        let alpha = shipping_domain("shared", "# From alpha\n");
        let mut global = GlobalConfig::default();
        global.domains.insert(
            "zulu".into(),
            with_decision(DomainEntry::file(zulu.path()), Some(true)),
        );
        global.domains.insert(
            "alpha".into(),
            with_decision(DomainEntry::file(alpha.path()), Some(true)),
        );

        let dir = tempfile::tempdir().unwrap();
        let receipt_path = dir.path().join("receipt.json");
        let mut receipt = ProvisionReceipt::default();
        let mut state = HarnessState::default();
        state.files.insert(
            "agents/shared.md".to_string(),
            InstalledFile {
                domain: "zulu".to_string(),
                sha256: sha256_hex(b"# From zulu\n"),
            },
        );
        receipt.harnesses.insert("claude-code".to_string(), state);
        save(&receipt_path, &receipt).unwrap();

        let report = status(
            &global,
            &receipt_path,
            &[HarnessKind::ClaudeCode],
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(report.domains[0].domain, "alpha");
        assert_eq!(
            report.harnesses[0].drift, 0,
            "the first-declared domain wins"
        );
    }

    // --- status artifact rows ------------------------------------------------------

    const OPTIONAL_MARK: &str = "---\nmetadata:\n  provision: optional\n---\n\nBody.\n";

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// A harbor that declares skills, agents and mcps: `skills/tide`
    /// (required, two files), `skills/storm` (optional), `agents/pilot`
    /// (required), `mcps/buoy` (required) and `mcps/lighthouse` (optional).
    fn marked_harbor() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_manifest(
            root,
            "## Provisioning\n\n- skills: skills\n- agents: agents\n- mcps: mcps\n",
        );
        write_file(
            root,
            "skills/tide/SKILL.md",
            "---\nname: tide\n---\n\nTides.\n",
        );
        write_file(root, "skills/tide/notes.md", "Notes.\n");
        write_file(root, "skills/storm/SKILL.md", OPTIONAL_MARK);
        write_file(root, "agents/pilot.md", "Steer.\n");
        write_file(
            root,
            "mcps/buoy.json",
            r#"{"name": "buoy", "server": {"type": "stdio"}}"#,
        );
        write_file(
            root,
            "mcps/lighthouse.json",
            r#"{"name": "lighthouse", "metadata": {"provision": "optional"}, "server": {"type": "http"}}"#,
        );
        dir
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn status_of(global: &GlobalConfig) -> StatusReport {
        let receipts = tempfile::tempdir().unwrap();
        status(
            global,
            &receipts.path().join("receipt.json"),
            &[],
            &HashSet::new(),
        )
        .unwrap()
    }

    fn rows(d: &DomainStatus) -> Vec<(&str, bool, bool)> {
        d.artifacts
            .iter()
            .map(|a| (a.key.as_str(), a.optional, a.on))
            .collect()
    }

    fn domain<'a>(report: &'a StatusReport, name: &str) -> &'a DomainStatus {
        report.domains.iter().find(|d| d.domain == name).unwrap()
    }

    fn picked_entry(dir: &Path, decision: Provision) -> DomainEntry {
        let mut entry = DomainEntry::file(dir);
        entry.provision = Some(decision);
        entry
    }

    #[test]
    fn status_lists_every_artifact_on_or_off_with_its_optional_mark() {
        let (picked, plain, undecided, denied) = (
            marked_harbor(),
            marked_harbor(),
            marked_harbor(),
            marked_harbor(),
        );
        let pick = Provision::All {
            except: names(&["skills/tide"]),
            also: names(&["mcps/lighthouse"]),
        };
        let mut global = GlobalConfig::default();
        let d = &mut global.domains;
        d.insert("picked".into(), picked_entry(picked.path(), pick.clone()));
        d.insert(
            "plain".into(),
            with_decision(DomainEntry::file(plain.path()), Some(true)),
        );
        d.insert("undecided".into(), DomainEntry::file(undecided.path()));
        d.insert(
            "denied".into(),
            with_decision(DomainEntry::file(denied.path()), Some(false)),
        );
        let report = status_of(&global);

        let picked = domain(&report, "picked");
        assert_eq!(picked.selection, Some(pick));
        assert_eq!(
            rows(picked),
            [
                ("skills/storm", true, false),
                ("skills/tide", false, false),
                ("agents/pilot", false, true),
                ("mcps/buoy", false, true),
                ("mcps/lighthouse", true, true),
            ]
        );
        let tide = &picked.artifacts[1];
        assert_eq!(tide.kind, ArtifactType::Skills);
        assert_eq!(tide.name, "tide");
        assert!(picked.stale.is_empty() && picked.malformed.is_empty());

        let plain = domain(&report, "plain");
        assert_eq!(plain.selection, None);
        assert_eq!(
            rows(plain),
            [
                ("skills/storm", true, false),
                ("skills/tide", false, true),
                ("agents/pilot", false, true),
                ("mcps/buoy", false, true),
                ("mcps/lighthouse", true, false),
            ]
        );

        for name in ["undecided", "denied"] {
            let d = domain(&report, name);
            assert_eq!(d.selection, None, "{name}");
            assert_eq!(d.artifacts.len(), 5, "{name}");
            assert!(d.artifacts.iter().all(|a| !a.on), "{name}");
            let optional: Vec<&str> = d
                .artifacts
                .iter()
                .filter(|a| a.optional)
                .map(|a| a.key.as_str())
                .collect();
            assert_eq!(optional, ["skills/storm", "mcps/lighthouse"], "{name}");
        }
    }

    #[test]
    fn status_keeps_empty_artifact_lists_for_virtual_and_undeclaring_domains() {
        let plain = tempfile::tempdir().unwrap();
        write_manifest(plain.path(), "");
        write_file(plain.path(), "skills/tide/SKILL.md", "Tides.\n");
        let pick = Provision::Only(names(&["nonsense", "skills/gone"]));
        let mut global = GlobalConfig::default();
        global
            .domains
            .insert("plain".into(), picked_entry(plain.path(), pick.clone()));
        let mut virtual_entry = DomainEntry::virtual_domain();
        virtual_entry.provision = Some(pick);
        global.domains.insert("cloud".into(), virtual_entry);
        let report = status_of(&global);

        assert_eq!(report.domains.len(), 2);
        for d in &report.domains {
            assert!(d.artifacts.is_empty(), "{}", d.domain);
            assert!(d.stale.is_empty(), "{}", d.domain);
            assert!(d.malformed.is_empty(), "{}", d.domain);
        }
    }

    #[test]
    fn status_names_stale_and_malformed_names() {
        let (picked, only) = (marked_harbor(), marked_harbor());
        let mut global = GlobalConfig::default();
        let pick = Provision::All {
            except: names(&["skills/old-charts", "skills", "boats/dinghy", "skills/tide"]),
            also: names(&["mcps/gone", "mcps/lighthouse", "skills/old-charts"]),
        }
        .normalized();
        global
            .domains
            .insert("picked".into(), picked_entry(picked.path(), pick));
        let pick = Provision::Only(names(&["agents/pilot", "agents/ghost"])).normalized();
        global
            .domains
            .insert("only".into(), picked_entry(only.path(), pick));
        let report = status_of(&global);

        let picked = domain(&report, "picked");
        // `except` before `also`, each sorted; a name in both lists once.
        assert_eq!(picked.stale, names(&["skills/old-charts", "mcps/gone"]));
        assert_eq!(picked.malformed, names(&["boats/dinghy", "skills"]));
        // A stale or malformed name adds no artifact row.
        assert_eq!(picked.artifacts.len(), 5);

        let only = domain(&report, "only");
        assert_eq!(only.decision, Decision::Only);
        assert_eq!(only.stale, names(&["agents/ghost"]));
        assert!(only.malformed.is_empty());
        assert_eq!(
            rows(only),
            [
                ("skills/storm", true, false),
                ("skills/tide", false, false),
                ("agents/pilot", false, true),
                ("mcps/buoy", false, false),
                ("mcps/lighthouse", true, false),
            ]
        );
    }

    #[test]
    fn a_missing_root_reports_no_stale_names() {
        let dir = tempfile::tempdir().unwrap();
        // Commands and mcps are declared, but only the skills root is on disk
        // (a mirror not pulled, a tools repository not checked out).
        write_manifest(
            dir.path(),
            "## Provisioning\n\n- skills: skills\n- commands: commands\n- mcps: mcps\n",
        );
        write_file(dir.path(), "skills/tide/SKILL.md", "Tides.\n");
        let pick = Provision::Only(names(&[
            "commands/charts/plot-route",
            "mcps/lighthouse",
            "skills/old-charts",
            "skills/tide",
        ]));
        let mut global = GlobalConfig::default();
        global
            .domains
            .insert("harbor".into(), picked_entry(dir.path(), pick));
        let report = status_of(&global);

        let harbor = domain(&report, "harbor");
        assert_eq!(harbor.stale, names(&["skills/old-charts"]));
        assert_eq!(rows(harbor), [("skills/tide", false, true)]);
    }

    #[test]
    fn only_sorts_with_allowed() {
        let (alpha, bravo, aaa, abc) = (
            shipping_domain("a1", "# A\n"),
            shipping_domain("a2", "# A\n"),
            shipping_domain("a3", "# A\n"),
            shipping_domain("a4", "# A\n"),
        );
        let mut global = GlobalConfig::default();
        let d = &mut global.domains;
        d.insert(
            "abc".into(),
            with_decision(DomainEntry::file(abc.path()), Some(false)),
        );
        d.insert("aaa".into(), DomainEntry::file(aaa.path()));
        d.insert(
            "bravo".into(),
            picked_entry(bravo.path(), Provision::Only(names(&["agents/a2"]))),
        );
        d.insert(
            "alpha".into(),
            with_decision(DomainEntry::file(alpha.path()), Some(true)),
        );
        let report = status_of(&global);

        let order: Vec<(&str, Decision)> = report
            .domains
            .iter()
            .map(|d| (d.domain.as_str(), d.decision))
            .collect();
        assert_eq!(
            order,
            [
                ("alpha", Decision::Allowed),
                ("bravo", Decision::Only),
                ("aaa", Decision::Undecided),
                ("abc", Decision::Denied),
            ]
        );
    }

    #[test]
    fn render_pending_offers_all_none_or_pick() {
        let pending = [PendingDomain {
            domain: "harbor".to_string(),
            counts: BTreeMap::from([("skills".to_string(), 2), ("mcps".to_string(), 1)]),
        }];
        assert_eq!(
            render_pending(&pending),
            vec![
                "[crystalline] The `harbor` domain ships artifacts to provision (skills: 2, mcps: 1) but has no decision yet - ask the user whether to install all, none or only some of them, then apply the answer with the `provision` tool (allow, deny, take or skip) or `crystalline provision allow harbor`."
                    .to_string()
            ]
        );
    }
}
