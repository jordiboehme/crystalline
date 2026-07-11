//! The reconcile engine: making one harness's installed artifacts match a
//! [`DesiredSet`]. Where `model` scans and projects and `receipt` remembers,
//! this is the layer that finally writes into a harness's own config
//! directory - installing new files, updating stale ones, adopting matching
//! ones, retiring what a domain no longer ships and registering MCP servers -
//! all while treating anything Crystalline did not itself write as sacred.
//!
//! The receipt is the ownership record. A file recorded there is Crystalline's
//! to update or remove; a file that is present but unrecorded is the user's or
//! another tool's and is only ever adopted when it already matches byte for
//! byte, never overwritten. The same line runs through the MCP path: a server
//! name already registered outside Crystalline is left alone with a notice,
//! never seized.
//!
//! File writes happen here; process spawning does not. Registering an MCP
//! server means shelling out to a harness's own CLI, which is process work and
//! stays behind the [`McpRunner`] trait so the format crate keeps its promise
//! never to depend on an async runtime or spawn a child. The service crate
//! supplies the real runner; tests supply a scripted one.

use std::path::{Path, PathBuf};

use crate::config;
use crate::harness::{HarnessKind, artifact_base};
use crate::manifest::ArtifactType;
use crate::provision::model::{DesiredFile, DesiredPayload, DesiredSet};
use crate::provision::receipt::{
    HarnessState, InstalledFile, InstalledMcp, plain_rel_key, sha256_hex,
};

/// The result of asking a harness's CLI to register or deregister one MCP
/// server. Registration is process work behind [`McpRunner`], so a reconcile
/// run learns the outcome only through these cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpOutcome {
    /// The change took effect. `notices` carries any user-facing notes raised
    /// while applying it - typically a `server` field the harness CLI has no
    /// flag for, dropped with a notice naming it rather than silently - which
    /// the reconcile surfaces alongside its own notices.
    Applied {
        /// Notices raised while applying the change.
        notices: Vec<String>,
    },
    /// The add was refused because the name is already registered by someone
    /// else - foreign data a reconcile never seizes.
    AlreadyExists,
    /// The change could not be expressed as this harness's CLI invocation at
    /// all - a `server` object naming neither a command nor a url - so
    /// nothing was run.
    Unsupported,
    /// The change was recognized but deliberately not carried out now. The
    /// session-start path installs a runner that answers this for every add
    /// and remove, so file artifacts reconcile in the hook while MCP changes -
    /// which would mean spawning a harness CLI - wait for an explicit
    /// `crystalline provision` run. A reconcile treats it exactly like
    /// [`McpOutcome::Failed`] for state (nothing recorded, nothing forgotten)
    /// but raises no notice: the caller aggregates one line for the whole run.
    Deferred,
    /// The command could not be run or failed. `manual` carries the exact
    /// command a user can run by hand, so a notice can pass it straight
    /// through.
    Failed {
        /// The command to run by hand.
        manual: String,
    },
}

impl McpOutcome {
    /// A clean [`McpOutcome::Applied`] with no notices - the common case, and
    /// the spelling tests and runners reach for when nothing was dropped.
    pub fn applied() -> McpOutcome {
        McpOutcome::Applied {
            notices: Vec::new(),
        }
    }
}

/// An [`McpRunner`] that carries out no MCP change at all: every add and
/// remove answers [`McpOutcome::Deferred`]. The session-start path installs it
/// so a hook can reconcile a domain's file artifacts immediately while every
/// MCP change - the one part that would shell out to a harness CLI - is left
/// for an explicit `crystalline provision` run. Spawning a child from a
/// session-start hook is exactly what this crate refuses to do, and this
/// runner is how the reconcile engine keeps that promise on the hook path.
pub struct DeferringMcpRunner;

impl McpRunner for DeferringMcpRunner {
    fn add(&mut self, _harness: HarnessKind, _name: &str, _server_json: &str) -> McpOutcome {
        McpOutcome::Deferred
    }
    fn remove(&mut self, _harness: HarnessKind, _name: &str) -> McpOutcome {
        McpOutcome::Deferred
    }
}

/// How a reconcile run registers and deregisters MCP servers. The one seam
/// between the format crate's pure, spawn-free reconcile and the service
/// crate's real shell-out to a harness CLI. A test supplies a scripted
/// implementation; the service crate supplies `SystemMcpRunner`.
pub trait McpRunner {
    /// Register `server_json` under `name` for `harness`.
    fn add(&mut self, harness: HarnessKind, name: &str, server_json: &str) -> McpOutcome;
    /// Deregister the server named `name` for `harness`.
    fn remove(&mut self, harness: HarnessKind, name: &str) -> McpOutcome;
}

/// One thing a reconcile run did to one artifact, for a caller to report.
/// `target` is the file's on-disk path for a file action, the server name for
/// an MCP action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactAction {
    /// The file path (files) or server name (MCP) the action concerns.
    pub target: String,
    /// What happened.
    pub status: ActionStatus,
}

/// The outcome of reconciling one artifact. Each variant is a distinct
/// row of the reconcile semantics: a caller renders them, never guesses at
/// what a plain "changed" meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    /// A desired file was written where none (or none owned) existed.
    Installed,
    /// A byte-identical unowned file was taken into the receipt, not rewritten.
    Adopted,
    /// An unowned file differed from the desired copy and was left in place.
    ForeignKept,
    /// An owned, unedited file was overwritten with newer canonical bytes.
    Updated,
    /// An owned but edited file was overwritten, its edit kept as `.bak`.
    UpdatedBackup,
    /// An owned, unedited file the desired set dropped was deleted.
    Removed,
    /// An owned but edited file the desired set dropped was renamed to `.bak`.
    RetiredBackup,
    /// An MCP server was registered.
    McpAdded,
    /// An MCP server's config changed and was re-registered.
    McpUpdated,
    /// An MCP server the desired set dropped was deregistered.
    McpRemoved,
    /// An MCP server was left as it was (already registered elsewhere, or this
    /// harness cannot manage it yet).
    McpSkipped,
    /// An MCP add, update or remove could not be carried out.
    McpFailed,
    /// An MCP add, update or remove was deferred to an explicit provision run
    /// rather than carried out now - the session-start path never spawns a
    /// harness CLI. The receipt entry is left exactly as it was.
    McpDeferred,
}

/// Reconcile one harness's installed artifacts against `desired`, mutating
/// `state` (the harness's receipt) to reflect exactly what is now installed
/// and owned, and driving MCP registration through `mcp`.
///
/// Returns the per-artifact actions and any user-facing notices. A filesystem
/// error on a single artifact, or any MCP stumble, degrades to a notice and
/// the pass continues; nothing short of being unable to resolve where a kind
/// lives at all bubbles up as an error.
pub fn reconcile_harness(
    harness: HarnessKind,
    desired: &DesiredSet,
    state: &mut HarnessState,
    mcp: &mut dyn McpRunner,
) -> anyhow::Result<(Vec<ArtifactAction>, Vec<String>)> {
    // Resolve each kind's base once, up front, so the pure pass below never
    // has to reach for the environment. A base that fails to resolve is the
    // one condition that bubbles rather than degrading to a notice.
    let skills = artifact_base(harness, ArtifactType::Skills)?;
    let commands = artifact_base(harness, ArtifactType::Commands)?;
    let agents = artifact_base(harness, ArtifactType::Agents)?;
    let mcps = artifact_base(harness, ArtifactType::Mcps)?;
    let resolve = move |kind: ArtifactType| match kind {
        ArtifactType::Skills => skills.clone(),
        ArtifactType::Commands => commands.clone(),
        ArtifactType::Agents => agents.clone(),
        ArtifactType::Mcps => mcps.clone(),
    };
    Ok(reconcile_resolved(harness, &resolve, desired, state, mcp))
}

/// The engine behind [`reconcile_harness`], with each kind's base already
/// resolved to a path (or `None` where this harness keeps that kind nowhere on
/// disk). Split out so a test drives the whole file lifecycle against tempdir
/// bases without touching the real environment.
fn reconcile_resolved(
    harness: HarnessKind,
    resolve: &dyn Fn(ArtifactType) -> Option<PathBuf>,
    desired: &DesiredSet,
    state: &mut HarnessState,
    mcp: &mut dyn McpRunner,
) -> (Vec<ArtifactAction>, Vec<String>) {
    let mut actions = Vec::new();
    let mut notices = Vec::new();
    reconcile_files(harness, resolve, desired, state, &mut actions, &mut notices);
    reconcile_mcps(harness, desired, state, mcp, &mut actions, &mut notices);
    (actions, notices)
}

// --- files -------------------------------------------------------------------

/// Reconcile the file artifacts: install, adopt, update or leave every desired
/// file, then retire or forget every receipt row the desired set dropped.
fn reconcile_files(
    harness: HarnessKind,
    resolve: &dyn Fn(ArtifactType) -> Option<PathBuf>,
    desired: &DesiredSet,
    state: &mut HarnessState,
    actions: &mut Vec<ArtifactAction>,
    notices: &mut Vec<String>,
) {
    for (key, want) in &desired.files {
        // Desired keys are trusted by construction, but pass them through the
        // same gate as receipt keys anyway - the check is cheap and the
        // symmetry means one code path can never grow a hole the other closes.
        let Some((kind, rel)) = resolve_key(key) else {
            continue;
        };
        let Some(base) = resolve(kind) else {
            continue; // desired_set filters unsupported kinds; unreachable
        };
        let target = join_rel(&base, rel);
        apply_desired_file(harness, key, &target, want, state, actions, notices);
    }

    // Receipt rows the desired set no longer carries. Collect the keys first
    // so the receipt is not mutated while it is being read.
    let undesired: Vec<String> = state
        .files
        .keys()
        .filter(|k| !desired.files.contains_key(*k))
        .cloned()
        .collect();
    for key in undesired {
        let Some((kind, rel)) = resolve_key(&key) else {
            continue; // hostile receipt row: cannot resolve, so cannot delete
        };
        let Some(base) = resolve(kind) else {
            continue; // no base for this kind here: leave the row untouched
        };
        let target = join_rel(&base, rel);
        retire_file(&key, &base, &target, state, actions, notices);
    }
}

/// Reconcile one desired file against what is on disk and in the receipt.
fn apply_desired_file(
    harness: HarnessKind,
    key: &str,
    target: &Path,
    want: &DesiredFile,
    state: &mut HarnessState,
    actions: &mut Vec<ArtifactAction>,
    notices: &mut Vec<String>,
) {
    let owned = state.files.get(key).cloned();
    match std::fs::read(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Absent locally, recorded or not: an explicit reconcile writes it.
            // A file missing on disk is not read as a deletion here; that
            // session-start nuance arrives in a later milestone.
            match write_payload(target, &want.payload) {
                Ok(sha) => {
                    record_file(state, key, want, sha);
                    actions.push(file_action(target, ActionStatus::Installed));
                }
                Err(msg) => notices.push(msg),
            }
        }
        Err(e) => notices.push(read_error_notice(target, &e)),
        Ok(local) => {
            let local_sha = sha256_hex(&local);
            match owned {
                // Owned and unedited: overwrite only when the canonical bytes
                // actually moved on.
                Some(installed) if installed.sha256 == local_sha => {
                    if local_sha == want.sha256 {
                        record_file(state, key, want, want.sha256.clone());
                    } else {
                        match write_payload(target, &want.payload) {
                            Ok(sha) => {
                                record_file(state, key, want, sha);
                                actions.push(file_action(target, ActionStatus::Updated));
                            }
                            Err(msg) => notices.push(msg),
                        }
                    }
                }
                // Owned but edited: preserve the edit as `.bak`, then overwrite.
                Some(_) => {
                    if let Err(msg) = copy_to_backup(target, &local) {
                        notices.push(msg);
                        return;
                    }
                    match write_payload(target, &want.payload) {
                        Ok(sha) => {
                            record_file(state, key, want, sha);
                            actions.push(file_action(target, ActionStatus::UpdatedBackup));
                        }
                        Err(msg) => notices.push(msg),
                    }
                }
                // Not ours: adopt a byte-identical file, otherwise never touch.
                None => {
                    if local_sha == want.sha256 {
                        record_file(state, key, want, want.sha256.clone());
                        actions.push(file_action(target, ActionStatus::Adopted));
                    } else {
                        notices.push(foreign_kept_notice(harness, target, &want.domain));
                        actions.push(file_action(target, ActionStatus::ForeignKept));
                    }
                }
            }
        }
    }
}

/// Retire one receipt row the desired set dropped: delete a clean copy we own
/// (pruning emptied namespace dirs), rename an edited one to `.bak`, or forget
/// a row whose file is already gone.
fn retire_file(
    key: &str,
    base: &Path,
    target: &Path,
    state: &mut HarnessState,
    actions: &mut Vec<ArtifactAction>,
    notices: &mut Vec<String>,
) {
    let Some(installed) = state.files.get(key).cloned() else {
        return; // the undesired set came from the receipt; defensive
    };
    match std::fs::read(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            state.files.remove(key); // already gone: forget it silently
        }
        Err(e) => notices.push(read_error_notice(target, &e)),
        Ok(local) => {
            if sha256_hex(&local) == installed.sha256 {
                // Provably ours and clean: delete, then prune emptied namespace
                // dirs up to but never including the kind base.
                match std::fs::remove_file(target) {
                    Ok(()) => {
                        if let Some(parent) = target.parent() {
                            prune_empty_dirs(base, parent);
                        }
                        state.files.remove(key);
                        actions.push(file_action(target, ActionStatus::Removed));
                    }
                    Err(e) => notices.push(remove_error_notice(target, &e)),
                }
            } else {
                // Edited since we wrote it: retire to `.bak`, destroy never.
                match rename_to_backup(target) {
                    Ok(()) => {
                        state.files.remove(key);
                        actions.push(file_action(target, ActionStatus::RetiredBackup));
                    }
                    Err(msg) => notices.push(msg),
                }
            }
        }
    }
}

// --- MCP ---------------------------------------------------------------------

/// Reconcile the MCP artifacts: register, re-register or leave every desired
/// server, then deregister every receipt row the desired set dropped. An MCP
/// stumble is never fatal: it degrades to a notice and the next row still runs.
fn reconcile_mcps(
    harness: HarnessKind,
    desired: &DesiredSet,
    state: &mut HarnessState,
    mcp: &mut dyn McpRunner,
    actions: &mut Vec<ArtifactAction>,
    notices: &mut Vec<String>,
) {
    for (name, want) in &desired.mcps {
        match state.mcps.get(name).cloned() {
            // Not ours yet: try to register it.
            None => match mcp.add(harness, name, &want.server_json) {
                McpOutcome::Applied {
                    notices: add_notices,
                } => {
                    notices.extend(add_notices);
                    record_mcp(state, name, &want.domain, &want.sha256);
                    actions.push(mcp_action(name, ActionStatus::McpAdded));
                }
                McpOutcome::AlreadyExists => {
                    notices.push(mcp_already_exists_notice(harness, name, &want.domain));
                    actions.push(mcp_action(name, ActionStatus::McpSkipped));
                }
                McpOutcome::Failed { manual } => {
                    notices.push(mcp_add_failed_notice(harness, name, &manual));
                    actions.push(mcp_action(name, ActionStatus::McpFailed));
                }
                McpOutcome::Unsupported => {
                    notices.push(mcp_unsupported_notice(harness, name));
                    actions.push(mcp_action(name, ActionStatus::McpSkipped));
                }
                // Deferred: like a failed add for state (nothing recorded),
                // but silent - the caller raises one line for the whole run.
                McpOutcome::Deferred => {
                    actions.push(mcp_action(name, ActionStatus::McpDeferred));
                }
            },
            // Ours and unchanged: nothing to do, keep the record.
            Some(installed) if installed.sha256 == want.sha256 => {}
            // Ours but the config changed: remove then add. Any stumble keeps
            // the old record so the server stays exactly as it was.
            Some(_) => {
                let removed = mcp.remove(harness, name);
                // Deferred remove: leave the old entry exactly as it was and
                // raise no notice, the same silent, state-preserving deferral
                // the add path takes.
                if removed == McpOutcome::Deferred {
                    actions.push(mcp_action(name, ActionStatus::McpDeferred));
                    continue;
                }
                let McpOutcome::Applied {
                    notices: remove_notices,
                } = removed
                else {
                    notices.push(mcp_update_failed_notice(
                        harness,
                        name,
                        outcome_manual(&removed),
                    ));
                    actions.push(mcp_action(name, ActionStatus::McpFailed));
                    continue;
                };
                notices.extend(remove_notices);
                match mcp.add(harness, name, &want.server_json) {
                    McpOutcome::Applied {
                        notices: add_notices,
                    } => {
                        notices.extend(add_notices);
                        record_mcp(state, name, &want.domain, &want.sha256);
                        actions.push(mcp_action(name, ActionStatus::McpUpdated));
                    }
                    McpOutcome::Deferred => {
                        actions.push(mcp_action(name, ActionStatus::McpDeferred));
                    }
                    other => {
                        notices.push(mcp_update_failed_notice(
                            harness,
                            name,
                            outcome_manual(&other),
                        ));
                        actions.push(mcp_action(name, ActionStatus::McpFailed));
                    }
                }
            }
        }
    }

    let undesired: Vec<String> = state
        .mcps
        .keys()
        .filter(|n| !desired.mcps.contains_key(*n))
        .cloned()
        .collect();
    for name in undesired {
        match mcp.remove(harness, &name) {
            McpOutcome::Applied {
                notices: remove_notices,
            } => {
                notices.extend(remove_notices);
                state.mcps.remove(&name);
                actions.push(mcp_action(&name, ActionStatus::McpRemoved));
            }
            // Deferred: keep the record until an explicit provision run
            // deregisters it, and stay silent - one aggregated line covers it.
            McpOutcome::Deferred => {
                actions.push(mcp_action(&name, ActionStatus::McpDeferred));
            }
            McpOutcome::Failed { manual } => {
                notices.push(mcp_remove_failed_notice(harness, &name, &manual));
                actions.push(mcp_action(&name, ActionStatus::McpFailed));
            }
            McpOutcome::AlreadyExists | McpOutcome::Unsupported => {
                notices.push(mcp_remove_failed_notice(harness, &name, ""));
                actions.push(mcp_action(&name, ActionStatus::McpFailed));
            }
        }
    }
}

// --- key resolution and path helpers -----------------------------------------

/// The [`ArtifactType`] a kind id names, or `None` for an unknown id. Built on
/// the public [`ArtifactType::id`] so it never drifts from the canonical
/// spellings a scan and a receipt both write.
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

/// Split a `"<kind>/<rel>"` key into its kind and its key-shaped remainder,
/// gating both halves: the prefix must name a known [`ArtifactType`] and the
/// remainder must pass [`plain_rel_key`], so a hostile receipt row (a bad kind,
/// a `..` segment, an absolute path) resolves to `None` and is skipped before
/// any component of it is ever joined onto a real directory.
fn resolve_key(key: &str) -> Option<(ArtifactType, &str)> {
    let (prefix, rel) = key.split_once('/')?;
    let kind = kind_from_id(prefix)?;
    if !plain_rel_key(rel) {
        return None;
    }
    Some((kind, rel))
}

/// Join a gated, `/`-separated `rel` onto `base` one plain component at a time,
/// so the result is identical on every platform and can only ever land inside
/// `base` (every component has already passed [`plain_rel_key`]).
fn join_rel(base: &Path, rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for component in rel.split('/') {
        path.push(component);
    }
    path
}

/// The `<name>.bak` sibling of `target` - its full file name plus `.bak`, so
/// `SKILL.md` becomes `SKILL.md.bak`. `None` only when `target` has no file
/// name, which a gated key never produces.
fn backup_path(target: &Path) -> Option<PathBuf> {
    let name = target.file_name()?;
    let mut bak = name.to_os_string();
    bak.push(".bak");
    Some(target.with_file_name(bak))
}

/// Remove `file_parent` and each emptied ancestor above it, stopping at (and
/// never removing) `base`. A non-empty dir, or any error, ends the walk: this
/// only ever tidies away namespace folders a reconcile itself emptied.
fn prune_empty_dirs(base: &Path, file_parent: &Path) {
    let mut dir = file_parent.to_path_buf();
    while dir != base && dir.starts_with(base) {
        let empty = std::fs::read_dir(&dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !empty || std::fs::remove_dir(&dir).is_err() {
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
}

// --- filesystem primitives ---------------------------------------------------

/// Resolve a [`DesiredPayload`] into the exact bytes to write: a passthrough
/// source is read off disk, a rendered target already carries its bytes. This
/// is the one arm the cross-dialect payload enum adds to reconcile; every row
/// of the semantics table below is otherwise unchanged, since it only ever asks
/// "what bytes does this desired file resolve to" and hashes what it wrote.
fn payload_bytes(target: &Path, payload: &DesiredPayload) -> Result<Vec<u8>, String> {
    match payload {
        DesiredPayload::File(source) => std::fs::read(source).map_err(|e| {
            format!(
                "could not read the source for `{}` ({e}) - leaving it as it is.",
                target.display()
            )
        }),
        DesiredPayload::Rendered(bytes) => Ok(bytes.clone()),
    }
}

/// Write a desired file's bytes to `target` atomically (creating parent dirs)
/// and return their hash, or a notice string on any read or write error.
fn write_payload(target: &Path, payload: &DesiredPayload) -> Result<String, String> {
    let bytes = payload_bytes(target, payload)?;
    config::save_bytes(target, &bytes).map_err(|e| {
        format!(
            "could not write `{}` ({e}) - leaving it as it is.",
            target.display()
        )
    })?;
    Ok(sha256_hex(&bytes))
}

/// Copy `local` bytes to `target`'s `.bak` sibling, overwriting any existing
/// backup so the newest divergence always wins.
fn copy_to_backup(target: &Path, local: &[u8]) -> Result<(), String> {
    let Some(bak) = backup_path(target) else {
        return Ok(());
    };
    config::save_bytes(&bak, local).map_err(|e| {
        format!(
            "could not back up `{}` ({e}) - leaving it as it is.",
            target.display()
        )
    })
}

/// Rename `target` onto its `.bak` sibling, overwriting any existing backup.
fn rename_to_backup(target: &Path) -> Result<(), String> {
    let Some(bak) = backup_path(target) else {
        return Ok(());
    };
    // Windows refuses a rename onto an existing file; the old backup loses to
    // the newer divergence either way.
    let _ = std::fs::remove_file(&bak);
    std::fs::rename(target, &bak).map_err(|e| {
        format!(
            "could not move `{}` aside ({e}) - leaving it in place.",
            target.display()
        )
    })
}

// --- receipt writes ----------------------------------------------------------

/// Record `key` as installed and owned at `sha`, from `want`'s domain.
fn record_file(state: &mut HarnessState, key: &str, want: &DesiredFile, sha: String) {
    state.files.insert(
        key.to_string(),
        InstalledFile {
            domain: want.domain.clone(),
            sha256: sha,
        },
    );
}

/// Record `name` as an installed and owned MCP server at `sha`, from `domain`.
fn record_mcp(state: &mut HarnessState, name: &str, domain: &str, sha: &str) {
    state.mcps.insert(
        name.to_string(),
        InstalledMcp {
            domain: domain.to_string(),
            sha256: sha.to_string(),
        },
    );
}

// --- action and notice construction ------------------------------------------

fn file_action(target: &Path, status: ActionStatus) -> ArtifactAction {
    ArtifactAction {
        target: target.display().to_string(),
        status,
    }
}

fn mcp_action(name: &str, status: ActionStatus) -> ArtifactAction {
    ArtifactAction {
        target: name.to_string(),
        status,
    }
}

/// The manual command a [`McpOutcome::Failed`] carries, or an empty string for
/// any other outcome (nothing to hand a user).
fn outcome_manual(outcome: &McpOutcome) -> &str {
    match outcome {
        McpOutcome::Failed { manual } => manual,
        _ => "",
    }
}

fn foreign_kept_notice(harness: HarnessKind, target: &Path, domain: &str) -> String {
    format!(
        "{} already has `{}`, which Crystalline did not write and which differs from the `{domain}` domain's copy - leaving your file in place.",
        harness.display_name(),
        target.display()
    )
}

fn mcp_add_failed_notice(harness: HarnessKind, name: &str, manual: &str) -> String {
    format!(
        "Could not register the MCP server `{name}` with {}. Register it yourself with: {manual}",
        harness.display_name()
    )
}

fn mcp_already_exists_notice(harness: HarnessKind, name: &str, domain: &str) -> String {
    format!(
        "The MCP server `{name}` is already registered with {} outside Crystalline - leaving it as it is. Remove that registration yourself to let the `{domain}` domain manage it, then reconcile again.",
        harness.display_name()
    )
}

fn mcp_unsupported_notice(harness: HarnessKind, name: &str) -> String {
    format!(
        "The MCP server `{name}` could not be turned into a {} registration - check its config's `server` object - so it was left alone.",
        harness.display_name()
    )
}

fn mcp_update_failed_notice(harness: HarnessKind, name: &str, manual: &str) -> String {
    if manual.is_empty() {
        format!(
            "Could not update the MCP server `{name}` for {} - it was left as it was.",
            harness.display_name()
        )
    } else {
        format!(
            "Could not update the MCP server `{name}` for {}. Update it yourself with: {manual}",
            harness.display_name()
        )
    }
}

fn mcp_remove_failed_notice(harness: HarnessKind, name: &str, manual: &str) -> String {
    if manual.is_empty() {
        format!(
            "Could not remove the MCP server `{name}` from {} - it was left registered.",
            harness.display_name()
        )
    } else {
        format!(
            "Could not remove the MCP server `{name}` from {}. Remove it yourself with: {manual}",
            harness.display_name()
        )
    }
}

fn read_error_notice(target: &Path, e: &std::io::Error) -> String {
    format!(
        "could not read `{}` to reconcile it ({e}) - leaving it untouched.",
        target.display()
    )
}

fn remove_error_notice(target: &Path, e: &std::io::Error) -> String {
    format!(
        "could not remove `{}` ({e}) - leaving it in place.",
        target.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // --- scripted MCP runner -------------------------------------------------

    /// An [`McpRunner`] that records every call in order and returns scripted
    /// outcomes, so a test can assert both what was asked of the harness CLI
    /// and in what order.
    struct RecordingRunner {
        calls: Vec<String>,
        add_outcomes: VecDeque<McpOutcome>,
        remove_outcomes: VecDeque<McpOutcome>,
    }

    impl RecordingRunner {
        fn new() -> RecordingRunner {
            RecordingRunner {
                calls: Vec::new(),
                add_outcomes: VecDeque::new(),
                remove_outcomes: VecDeque::new(),
            }
        }

        fn on_add(mut self, outcome: McpOutcome) -> RecordingRunner {
            self.add_outcomes.push_back(outcome);
            self
        }

        fn on_remove(mut self, outcome: McpOutcome) -> RecordingRunner {
            self.remove_outcomes.push_back(outcome);
            self
        }
    }

    impl McpRunner for RecordingRunner {
        fn add(&mut self, _harness: HarnessKind, name: &str, _server_json: &str) -> McpOutcome {
            self.calls.push(format!("add:{name}"));
            self.add_outcomes
                .pop_front()
                .unwrap_or_else(McpOutcome::applied)
        }

        fn remove(&mut self, _harness: HarnessKind, name: &str) -> McpOutcome {
            self.calls.push(format!("remove:{name}"));
            self.remove_outcomes
                .pop_front()
                .unwrap_or_else(McpOutcome::applied)
        }
    }

    /// A runner that must never be called: for pure file tests.
    struct NoMcp;
    impl McpRunner for NoMcp {
        fn add(&mut self, _h: HarnessKind, _n: &str, _s: &str) -> McpOutcome {
            panic!("add called in a file-only test");
        }
        fn remove(&mut self, _h: HarnessKind, _n: &str) -> McpOutcome {
            panic!("remove called in a file-only test");
        }
    }

    // --- test scaffolding ----------------------------------------------------

    /// A resolver mapping every kind to `<root>/<kind id>`, so a desired key
    /// `skills/x` lands under `<root>/skills` and `commands/y` under
    /// `<root>/commands`.
    fn bases(root: &Path) -> impl Fn(ArtifactType) -> Option<PathBuf> + '_ {
        move |kind: ArtifactType| Some(root.join(kind.id()))
    }

    /// Write `content` as the on-disk source for a desired file and return the
    /// [`DesiredFile`] pointing at it, from the `harbor` domain.
    fn desired_file(source_dir: &Path, name: &str, content: &str) -> DesiredFile {
        let src = source_dir.join(name);
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, content).unwrap();
        DesiredFile {
            domain: "harbor".to_string(),
            payload: DesiredPayload::File(src),
            sha256: sha256_hex(content.as_bytes()),
        }
    }

    /// A [`DesiredFile`] whose payload is already-translated bytes (a
    /// cross-dialect render), from the `harbor` domain. No source file exists on
    /// disk: the bytes are carried inline, exactly as a real render produces.
    fn rendered_file(content: &str) -> DesiredFile {
        DesiredFile {
            domain: "harbor".to_string(),
            payload: DesiredPayload::Rendered(content.as_bytes().to_vec()),
            sha256: sha256_hex(content.as_bytes()),
        }
    }

    fn installed(sha: &str) -> InstalledFile {
        InstalledFile {
            domain: "harbor".to_string(),
            sha256: sha.to_string(),
        }
    }

    fn run(
        resolve: &dyn Fn(ArtifactType) -> Option<PathBuf>,
        desired: &DesiredSet,
        state: &mut HarnessState,
        runner: &mut dyn McpRunner,
    ) -> (Vec<ArtifactAction>, Vec<String>) {
        reconcile_resolved(HarnessKind::ClaudeCode, resolve, desired, state, runner)
    }

    fn statuses(actions: &[ArtifactAction]) -> Vec<ActionStatus> {
        actions.iter().map(|a| a.status).collect()
    }

    const KEY: &str = "skills/tide-tables/SKILL.md";

    // --- file rows -----------------------------------------------------------

    #[test]
    fn desired_absent_locally_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let mut desired = DesiredSet::default();
        desired.files.insert(
            KEY.to_string(),
            desired_file(src.path(), "tides", "TIDES\n"),
        );
        let mut state = HarnessState::default();

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::Installed]);
        assert!(notices.is_empty(), "{notices:?}");
        let target = root.path().join("skills/tide-tables/SKILL.md");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "TIDES\n");
        assert_eq!(state.files[KEY].sha256, sha256_hex(b"TIDES\n"));
    }

    #[test]
    fn desired_present_not_in_receipt_matching_bytes_is_adopted() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let target = root.path().join("skills/tide-tables/SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "TIDES\n").unwrap();

        let mut desired = DesiredSet::default();
        desired.files.insert(
            KEY.to_string(),
            desired_file(src.path(), "tides", "TIDES\n"),
        );
        let mut state = HarnessState::default();

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::Adopted]);
        assert!(notices.is_empty(), "{notices:?}");
        // Recorded as owned, no backup written.
        assert_eq!(state.files[KEY].sha256, sha256_hex(b"TIDES\n"));
        assert!(!target.with_file_name("SKILL.md.bak").exists());
    }

    #[test]
    fn desired_present_not_in_receipt_differing_bytes_is_foreign_kept() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let target = root.path().join("skills/tide-tables/SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "MINE\n").unwrap();

        let mut desired = DesiredSet::default();
        desired.files.insert(
            KEY.to_string(),
            desired_file(src.path(), "tides", "TIDES\n"),
        );
        let mut state = HarnessState::default();

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::ForeignKept]);
        assert!(
            notices.iter().any(|n| n.contains("SKILL.md")),
            "notice names the file: {notices:?}"
        );
        // Left in place, not recorded.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "MINE\n");
        assert!(!state.files.contains_key(KEY));
    }

    #[test]
    fn desired_in_receipt_clean_canonical_changed_is_updated() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let target = root.path().join("skills/tide-tables/SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "OLD\n").unwrap();

        let mut desired = DesiredSet::default();
        desired
            .files
            .insert(KEY.to_string(), desired_file(src.path(), "tides", "NEW\n"));
        let mut state = HarnessState::default();
        state
            .files
            .insert(KEY.to_string(), installed(&sha256_hex(b"OLD\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::Updated]);
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "NEW\n");
        assert!(!target.with_file_name("SKILL.md.bak").exists());
        assert_eq!(state.files[KEY].sha256, sha256_hex(b"NEW\n"));
    }

    #[test]
    fn desired_in_receipt_local_edited_is_updated_with_backup() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let target = root.path().join("skills/tide-tables/SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "EDITED\n").unwrap();

        let mut desired = DesiredSet::default();
        desired
            .files
            .insert(KEY.to_string(), desired_file(src.path(), "tides", "NEW\n"));
        let mut state = HarnessState::default();
        // Receipt records some earlier hash, not the edited bytes on disk.
        state
            .files
            .insert(KEY.to_string(), installed(&sha256_hex(b"OLD\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::UpdatedBackup]);
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "NEW\n");
        let bak = target.with_file_name("SKILL.md.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "EDITED\n");
        assert_eq!(state.files[KEY].sha256, sha256_hex(b"NEW\n"));
    }

    #[test]
    fn desired_in_receipt_local_absent_is_reinstalled() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let mut desired = DesiredSet::default();
        desired.files.insert(
            KEY.to_string(),
            desired_file(src.path(), "tides", "TIDES\n"),
        );
        let mut state = HarnessState::default();
        state
            .files
            .insert(KEY.to_string(), installed(&sha256_hex(b"TIDES\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::Installed]);
        assert!(notices.is_empty(), "{notices:?}");
        let target = root.path().join("skills/tide-tables/SKILL.md");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "TIDES\n");
        assert_eq!(state.files[KEY].sha256, sha256_hex(b"TIDES\n"));
    }

    #[test]
    fn desired_in_receipt_unchanged_is_a_noop() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let target = root.path().join("skills/tide-tables/SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "TIDES\n").unwrap();

        let mut desired = DesiredSet::default();
        desired.files.insert(
            KEY.to_string(),
            desired_file(src.path(), "tides", "TIDES\n"),
        );
        let mut state = HarnessState::default();
        state
            .files
            .insert(KEY.to_string(), installed(&sha256_hex(b"TIDES\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert!(
            actions.is_empty(),
            "no action for an unchanged file: {actions:?}"
        );
        assert!(notices.is_empty(), "{notices:?}");
        assert!(state.files.contains_key(KEY));
    }

    #[test]
    fn undesired_in_receipt_local_clean_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("skills/tide-tables/SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "TIDES\n").unwrap();

        let desired = DesiredSet::default(); // no longer desired
        let mut state = HarnessState::default();
        state
            .files
            .insert(KEY.to_string(), installed(&sha256_hex(b"TIDES\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::Removed]);
        assert!(notices.is_empty(), "{notices:?}");
        assert!(!target.exists());
        // The namespace dir is pruned, the kind base is not.
        assert!(!root.path().join("skills/tide-tables").exists());
        assert!(root.path().join("skills").exists());
        assert!(!state.files.contains_key(KEY));
    }

    #[test]
    fn undesired_in_receipt_local_edited_is_retired_to_backup() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("skills/tide-tables/SKILL.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "EDITED\n").unwrap();

        let desired = DesiredSet::default();
        let mut state = HarnessState::default();
        // Recorded hash differs from the edited bytes on disk.
        state
            .files
            .insert(KEY.to_string(), installed(&sha256_hex(b"TIDES\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::RetiredBackup]);
        assert!(notices.is_empty(), "{notices:?}");
        assert!(!target.exists());
        let bak = target.with_file_name("SKILL.md.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "EDITED\n");
        assert!(!state.files.contains_key(KEY));
    }

    #[test]
    fn undesired_in_receipt_local_absent_is_dropped_silently() {
        let root = tempfile::tempdir().unwrap();
        let desired = DesiredSet::default();
        let mut state = HarnessState::default();
        state
            .files
            .insert(KEY.to_string(), installed(&sha256_hex(b"TIDES\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert!(actions.is_empty(), "{actions:?}");
        assert!(notices.is_empty(), "{notices:?}");
        assert!(!state.files.contains_key(KEY));
    }

    // --- rendered (cross-dialect) targets ------------------------------------

    // A Rendered target carries its bytes inline rather than pointing at a
    // source file, but it must travel every row of the semantics table exactly
    // as a File target does - the payload only changes where the bytes come
    // from, never what reconcile decides. These two rows stand in for the rest.

    #[test]
    fn rendered_absent_locally_is_installed_from_inline_bytes() {
        let root = tempfile::tempdir().unwrap();
        let key = "agents/quartermaster.md";
        let mut desired = DesiredSet::default();
        desired
            .files
            .insert(key.to_string(), rendered_file("RENDERED\n"));
        let mut state = HarnessState::default();

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::Installed]);
        assert!(notices.is_empty(), "{notices:?}");
        let target = root.path().join("agents/quartermaster.md");
        // The inline rendered bytes landed verbatim, and the receipt records
        // their hash - not any source file's.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "RENDERED\n");
        assert_eq!(state.files[key].sha256, sha256_hex(b"RENDERED\n"));
    }

    #[test]
    fn rendered_owned_but_edited_is_updated_with_backup() {
        let root = tempfile::tempdir().unwrap();
        let key = "agents/quartermaster.md";
        let target = root.path().join("agents/quartermaster.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "EDITED\n").unwrap();

        let mut desired = DesiredSet::default();
        desired
            .files
            .insert(key.to_string(), rendered_file("RENDERED-NEW\n"));
        let mut state = HarnessState::default();
        // Owned at an earlier hash, differing from the edited bytes on disk.
        state
            .files
            .insert(key.to_string(), installed(&sha256_hex(b"RENDERED-OLD\n")));

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::UpdatedBackup]);
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "RENDERED-NEW\n");
        let bak = target.with_file_name("quartermaster.md.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "EDITED\n");
        assert_eq!(state.files[key].sha256, sha256_hex(b"RENDERED-NEW\n"));
    }

    // --- prune ---------------------------------------------------------------

    #[test]
    fn prune_removes_namespace_dir_but_never_the_base() {
        let root = tempfile::tempdir().unwrap();
        let key = "commands/charts/plot-route.md";
        let target = root.path().join("commands/charts/plot-route.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "ROUTE\n").unwrap();

        let desired = DesiredSet::default();
        let mut state = HarnessState::default();
        state
            .files
            .insert(key.to_string(), installed(&sha256_hex(b"ROUTE\n")));

        let (actions, _) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert_eq!(statuses(&actions), vec![ActionStatus::Removed]);
        assert!(!root.path().join("commands/charts").exists());
        assert!(root.path().join("commands").exists(), "kind base survives");
    }

    // --- cross-kind resolution -----------------------------------------------

    #[test]
    fn cross_kind_keys_resolve_under_their_own_base() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let mut desired = DesiredSet::default();
        desired.files.insert(
            "skills/tide-tables/SKILL.md".to_string(),
            desired_file(src.path(), "skill", "SKILL\n"),
        );
        desired.files.insert(
            "commands/charts/plot-route.md".to_string(),
            desired_file(src.path(), "command", "ROUTE\n"),
        );
        let mut state = HarnessState::default();

        // Distinct bases per kind, so a mis-routed key would land in the wrong
        // tree and the assertions below would fail.
        let skills_base = root.path().join("skills-tree");
        let commands_base = root.path().join("commands-tree");
        let resolve = |kind: ArtifactType| match kind {
            ArtifactType::Skills => Some(skills_base.clone()),
            ArtifactType::Commands => Some(commands_base.clone()),
            _ => None,
        };

        let (actions, notices) = run(&resolve, &desired, &mut state, &mut NoMcp);

        assert_eq!(actions.len(), 2);
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(
            std::fs::read_to_string(skills_base.join("tide-tables/SKILL.md")).unwrap(),
            "SKILL\n"
        );
        assert_eq!(
            std::fs::read_to_string(commands_base.join("charts/plot-route.md")).unwrap(),
            "ROUTE\n"
        );
    }

    // --- hostile receipt keys ------------------------------------------------

    #[test]
    fn hostile_receipt_keys_have_no_effect_and_no_panic() {
        let root = tempfile::tempdir().unwrap();
        let desired = DesiredSet::default();
        let mut state = HarnessState::default();
        for key in ["../x", "a:b", "commands/../x", "weird/x"] {
            state
                .files
                .insert(key.to_string(), installed(&sha256_hex(b"x")));
        }

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut NoMcp);

        assert!(actions.is_empty(), "{actions:?}");
        assert!(notices.is_empty(), "{notices:?}");
        // Rows we cannot safely resolve are left untouched, never deleted.
        assert_eq!(state.files.len(), 4);
    }

    // --- MCP rows ------------------------------------------------------------

    fn desired_mcp(name: &str, sha_seed: &str) -> DesiredSet {
        let mut desired = DesiredSet::default();
        desired.mcps.insert(
            name.to_string(),
            crate::provision::model::DesiredMcp {
                domain: "harbor".to_string(),
                server_json: format!("{{\"seed\":\"{sha_seed}\"}}"),
                sha256: sha256_hex(sha_seed.as_bytes()),
            },
        );
        desired
    }

    fn installed_mcp(sha_seed: &str) -> InstalledMcp {
        InstalledMcp {
            domain: "harbor".to_string(),
            sha256: sha256_hex(sha_seed.as_bytes()),
        }
    }

    #[test]
    fn mcp_add_new_records_and_reports_added() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v1");
        let mut state = HarnessState::default();
        let mut runner = RecordingRunner::new().on_add(McpOutcome::applied());

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpAdded]);
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(runner.calls, vec!["add:lighthouse"]);
        assert_eq!(state.mcps["lighthouse"].sha256, sha256_hex(b"v1"));
    }

    #[test]
    fn mcp_add_applied_with_notices_registers_and_surfaces_them() {
        // A runner can apply a change while dropping a field its CLI has no
        // flag for (Codex http headers): the server still registers and owns
        // its receipt row, and the dropped-field notice reaches the caller.
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v1");
        let mut state = HarnessState::default();
        let dropped = "the `headers` field of the MCP server `lighthouse` has no Codex CLI flag - registering it without those headers.";
        let mut runner = RecordingRunner::new().on_add(McpOutcome::Applied {
            notices: vec![dropped.to_string()],
        });

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpAdded]);
        assert_eq!(notices, vec![dropped.to_string()]);
        assert_eq!(state.mcps["lighthouse"].sha256, sha256_hex(b"v1"));
    }

    #[test]
    fn mcp_sha_change_removes_then_adds() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v2");
        let mut state = HarnessState::default();
        state
            .mcps
            .insert("lighthouse".to_string(), installed_mcp("v1"));
        let mut runner = RecordingRunner::new()
            .on_remove(McpOutcome::applied())
            .on_add(McpOutcome::applied());

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpUpdated]);
        assert!(notices.is_empty(), "{notices:?}");
        // Remove strictly before add.
        assert_eq!(runner.calls, vec!["remove:lighthouse", "add:lighthouse"]);
        assert_eq!(state.mcps["lighthouse"].sha256, sha256_hex(b"v2"));
    }

    #[test]
    fn mcp_remove_undesired_drops_record() {
        let root = tempfile::tempdir().unwrap();
        let desired = DesiredSet::default();
        let mut state = HarnessState::default();
        state
            .mcps
            .insert("lighthouse".to_string(), installed_mcp("v1"));
        let mut runner = RecordingRunner::new().on_remove(McpOutcome::applied());

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpRemoved]);
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(runner.calls, vec!["remove:lighthouse"]);
        assert!(!state.mcps.contains_key("lighthouse"));
    }

    #[test]
    fn mcp_add_already_exists_is_not_recorded() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v1");
        let mut state = HarnessState::default();
        let mut runner = RecordingRunner::new().on_add(McpOutcome::AlreadyExists);

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpSkipped]);
        assert!(
            notices.iter().any(|n| n.contains("already registered")),
            "{notices:?}"
        );
        assert!(
            !state.mcps.contains_key("lighthouse"),
            "foreign data not seized"
        );
    }

    #[test]
    fn mcp_add_failed_records_nothing_and_notices_the_manual_command() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v1");
        let mut state = HarnessState::default();
        let mut runner = RecordingRunner::new().on_add(McpOutcome::Failed {
            manual: "claude mcp add-json lighthouse {...} --scope user".to_string(),
        });

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpFailed]);
        assert!(
            notices
                .iter()
                .any(|n| n.contains("claude mcp add-json lighthouse")),
            "notice carries the manual command: {notices:?}"
        );
        assert!(!state.mcps.contains_key("lighthouse"));
    }

    #[test]
    fn mcp_update_failed_keeps_old_entry() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v2");
        let mut state = HarnessState::default();
        state
            .mcps
            .insert("lighthouse".to_string(), installed_mcp("v1"));
        // Remove succeeds, the re-add fails: the old entry must survive.
        let mut runner = RecordingRunner::new()
            .on_remove(McpOutcome::applied())
            .on_add(McpOutcome::Failed {
                manual: "claude mcp add-json lighthouse {...} --scope user".to_string(),
            });

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpFailed]);
        assert!(
            notices
                .iter()
                .any(|n| n.contains("claude mcp add-json lighthouse")),
            "{notices:?}"
        );
        // Old sha kept, not the new one.
        assert_eq!(state.mcps["lighthouse"].sha256, sha256_hex(b"v1"));
    }

    #[test]
    fn mcp_remove_failed_keeps_entry() {
        let root = tempfile::tempdir().unwrap();
        let desired = DesiredSet::default();
        let mut state = HarnessState::default();
        state
            .mcps
            .insert("lighthouse".to_string(), installed_mcp("v1"));
        let mut runner = RecordingRunner::new().on_remove(McpOutcome::Failed {
            manual: "claude mcp remove lighthouse --scope user".to_string(),
        });

        let (actions, notices) = run(&bases(root.path()), &desired, &mut state, &mut runner);

        assert_eq!(statuses(&actions), vec![ActionStatus::McpFailed]);
        assert!(
            notices
                .iter()
                .any(|n| n.contains("claude mcp remove lighthouse")),
            "{notices:?}"
        );
        assert!(
            state.mcps.contains_key("lighthouse"),
            "record kept on failure"
        );
    }

    // --- MCP deferral (session-start path) -----------------------------------

    #[test]
    fn mcp_add_deferred_records_nothing_and_stays_silent() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v1");
        let mut state = HarnessState::default();

        let (actions, notices) = run(
            &bases(root.path()),
            &desired,
            &mut state,
            &mut DeferringMcpRunner,
        );

        assert_eq!(statuses(&actions), vec![ActionStatus::McpDeferred]);
        assert!(notices.is_empty(), "deferral is silent: {notices:?}");
        assert!(
            !state.mcps.contains_key("lighthouse"),
            "an add not performed is not recorded"
        );
    }

    #[test]
    fn mcp_update_deferred_keeps_the_old_entry_and_stays_silent() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v2");
        let mut state = HarnessState::default();
        state
            .mcps
            .insert("lighthouse".to_string(), installed_mcp("v1"));

        let (actions, notices) = run(
            &bases(root.path()),
            &desired,
            &mut state,
            &mut DeferringMcpRunner,
        );

        assert_eq!(statuses(&actions), vec![ActionStatus::McpDeferred]);
        assert!(notices.is_empty(), "deferral is silent: {notices:?}");
        // The old sha survives untouched, exactly as a failed update leaves it.
        assert_eq!(state.mcps["lighthouse"].sha256, sha256_hex(b"v1"));
    }

    #[test]
    fn mcp_remove_deferred_keeps_the_record_and_stays_silent() {
        let root = tempfile::tempdir().unwrap();
        let desired = DesiredSet::default(); // lighthouse no longer desired
        let mut state = HarnessState::default();
        state
            .mcps
            .insert("lighthouse".to_string(), installed_mcp("v1"));

        let (actions, notices) = run(
            &bases(root.path()),
            &desired,
            &mut state,
            &mut DeferringMcpRunner,
        );

        assert_eq!(statuses(&actions), vec![ActionStatus::McpDeferred]);
        assert!(notices.is_empty(), "deferral is silent: {notices:?}");
        assert!(
            state.mcps.contains_key("lighthouse"),
            "a remove not performed keeps its record"
        );
    }

    #[test]
    fn mcp_unchanged_defers_nothing() {
        let root = tempfile::tempdir().unwrap();
        let desired = desired_mcp("lighthouse", "v1");
        let mut state = HarnessState::default();
        state
            .mcps
            .insert("lighthouse".to_string(), installed_mcp("v1"));

        let (actions, notices) = run(
            &bases(root.path()),
            &desired,
            &mut state,
            &mut DeferringMcpRunner,
        );

        // An unchanged MCP is a no-op even on the deferring path: no runner
        // call, so no deferred action and nothing for the caller to report.
        assert!(
            actions.is_empty(),
            "no action for an unchanged MCP: {actions:?}"
        );
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(state.mcps["lighthouse"].sha256, sha256_hex(b"v1"));
    }
}
