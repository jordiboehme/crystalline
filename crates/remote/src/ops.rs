//! Pull-side orchestration: turning a domain's origin into local files.
//!
//! This module composes the finished building blocks of this crate into the
//! three read-side operations a domain needs against its GitHub origin:
//!
//! - [`subscribe`] downloads a repository subtree for the first time, laying
//!   down the working tree, the base snapshot and the origin state together.
//! - [`pull`] brings an already-connected domain up to date: it probes the
//!   branch, three-way merges every upstream change into the working tree,
//!   records conflicts it cannot reconcile automatically, advances the base
//!   snapshot and reconciles any share proposals that merged upstream.
//! - [`status`] reports where a domain stands relative to its origin, working
//!   fully offline from local state alone or, with a provider, filling in
//!   whether the branch has moved ahead.
//!
//! Every function is a plain async library function over a [`Provider`] trait
//! object and filesystem paths. There is no service, engine or CLI knowledge
//! here; wiring these into the daemon is a later task. Filesystem work is
//! synchronous `std::fs`; the caller wraps a whole operation in
//! `spawn_blocking` as needed. Provider calls are the only await points.
//!
//! ## Path spaces
//!
//! Two path spaces meet in pull. The provider's compare endpoint speaks
//! repo-relative paths (`<subpath>/notes/a.md`); every local layer - the
//! working tree, the base snapshot in [`crate::state`], conflict records and
//! proposal records - speaks domain-relative paths (`notes/a.md`). Both are
//! normalized to domain-relative in exactly one place, before the merge loop,
//! so nothing downstream has to know where the domain sits in its repository.
//!
//! ## Untrusted upstream content
//!
//! Repository content is untrusted input. Every path that reaches the
//! filesystem is validated through [`crate::state`]'s chokepoint
//! ([`crate::state::base_path`] and the conflict helpers), which rejects
//! traversal-shaped, absolute and Windows-drive-prefixed paths. Working-tree
//! writes funnel through the same validation via [`checked_working_path`]
//! before a byte is written, so a crafted path can never escape the domain
//! root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::archive::extract_tarball;
use crate::changes::{MAX_SHARED_FILE_BYTES, detect_local_changes};
use crate::error::RemoteError;
use crate::merge::{FileMerge, merge_file};
use crate::provider::{
    ChangeKind, CompareResult, HeadProbe, OriginSpec, ProposalState, Provider, UpstreamChange,
};
use crate::state::{self, BaseStamp, Conflict, OriginState, Proposal, ProposalStatus};

/// Above this many changed files (after subpath filtering) a compare is
/// abandoned for a whole-tree tarball diff, matching the provider's own
/// pagination ceiling. A compare that reports truncation takes the same path
/// regardless of count.
const MAX_COMPARE_FILES: usize = 50;

/// What [`subscribe`] wrote when it connected a domain to its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeReport {
    /// The commit the domain was subscribed at, now the base snapshot.
    pub base_commit: String,
    /// How many files were written to the working tree.
    pub files_written: usize,
    /// How many of those files are engrams (`.md`).
    pub engrams: usize,
    /// Upstream files skipped for exceeding [`MAX_SHARED_FILE_BYTES`], each
    /// with its size in bytes.
    pub skipped_large: Vec<(String, u64)>,
}

/// What [`pull`] did to bring a domain up to date with its origin.
#[derive(Debug, Clone, PartialEq)]
pub struct PullReport {
    /// True when the origin had nothing new: no files were written.
    pub up_to_date: bool,
    /// Domain-relative paths written or deleted from upstream this pull,
    /// including clean three-way merges.
    pub applied: Vec<String>,
    /// The subset of `applied` that went through a real three-way text merge
    /// rather than a plain take of upstream content.
    pub merged: Vec<String>,
    /// Conflicts recorded for the first time this pull.
    pub conflicts: Vec<Conflict>,
    /// Proposals whose status changed this pull, as `(number, new status)`.
    pub proposals: Vec<(u64, ProposalStatus)>,
    /// Upstream files skipped for exceeding [`MAX_SHARED_FILE_BYTES`], each
    /// with its size in bytes.
    pub skipped_large: Vec<(String, u64)>,
    /// True when the base commit was unreachable upstream and the domain was
    /// re-baselined onto the current head.
    pub re_baselined: bool,
}

/// A snapshot of where a domain stands relative to its origin, for status
/// displays.
#[derive(Debug, Clone, PartialEq)]
pub struct OriginStatusReport {
    /// The repository this domain tracks, `owner/name`.
    pub repo: String,
    /// The branch this domain tracks.
    pub branch: String,
    /// The base commit the domain is currently synced to.
    pub base_commit: String,
    /// Whether the branch has moved ahead of the base commit, or `None` when
    /// the origin was not probed (offline mode).
    pub behind: Option<bool>,
    /// How many local working-tree changes stand against the base snapshot.
    pub local_changes: usize,
    /// Working-tree files skipped for exceeding [`MAX_SHARED_FILE_BYTES`],
    /// each with its size in bytes.
    pub skipped_large: Vec<(String, u64)>,
    /// Share proposals still open for review.
    pub open_proposals: Vec<Proposal>,
    /// Share proposals closed without merging.
    pub declined_proposals: Vec<Proposal>,
    /// Conflicts still waiting to be resolved.
    pub conflicts: Vec<Conflict>,
    /// When the branch was last checked for new upstream commits.
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
}

/// One upstream change to integrate, already normalized to a domain-relative
/// path. `content` is the new content, or `None` when upstream removed the
/// file.
struct UpstreamEdit {
    path: String,
    content: Option<Vec<u8>>,
}

/// Connects a domain to its origin for the first time: downloads the tracked
/// subtree at the branch head, writes it to `domain_root`, records the
/// identical set as the base snapshot and saves a fresh [`OriginState`].
///
/// The target must look like a domain (a `MANIFEST.md` at the subtree root)
/// and `domain_root` must be absent or an empty directory; both are checked
/// before anything is written, so a rejected subscribe leaves the disk
/// untouched.
pub async fn subscribe(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
) -> Result<SubscribeReport, RemoteError> {
    // An unconditional probe (no etag) must report a concrete head; only a
    // conditional probe can answer Unchanged, so an Unchanged here is a
    // provider contract violation rather than a real state.
    let (head, etag) = match provider.branch_head(spec, None).await? {
        HeadProbe::Changed { head, etag } => (head, etag),
        HeadProbe::Unchanged => {
            return Err(RemoteError::Api {
                status: 0,
                message: "the origin reported no branch head for an unconditional probe"
                    .to_string(),
            });
        }
    };

    let bytes = provider.tarball(spec, &head).await?;
    let (extracted, skipped_large) = extract_tarball(&bytes, spec.subpath.as_deref())?;

    // A domain is defined by a MANIFEST.md at its root; without one the target
    // is not something to subscribe to, and nothing is written.
    if !extracted.contains_key("MANIFEST.md") {
        return Err(RemoteError::NotADomain {
            repo: spec.repo.clone(),
            path: spec.subpath.clone(),
        });
    }

    if domain_root.exists() {
        let mut entries = std::fs::read_dir(domain_root)?;
        if entries.next().is_some() {
            return Err(RemoteError::State(format!(
                "{} already exists and is not empty",
                domain_root.display()
            )));
        }
    }

    // One pass writes the working tree and the base snapshot together and
    // stamps the manifest from the same bytes, so the base is never read back.
    let mut files = BTreeMap::new();
    for (rel, content) in &extracted {
        let wt_path = checked_working_path(state_dir, domain_root, rel)?;
        write_working_file(&wt_path, content)?;
        state::write_base_file(state_dir, rel, content)?;
        files.insert(rel.clone(), stamp(content));
    }

    let engrams = extracted.keys().filter(|p| p.ends_with(".md")).count();

    let mut origin_state = OriginState::new(spec.repo.clone(), spec.branch.clone());
    origin_state.base_commit = head.clone();
    origin_state.ref_etag = etag;
    origin_state.last_checked = Some(Utc::now());
    origin_state.files = files;
    origin_state.save(state_dir)?;

    Ok(SubscribeReport {
        base_commit: head,
        files_written: extracted.len(),
        engrams,
        skipped_large,
    })
}

/// Brings an already-connected domain up to date with its origin.
///
/// The algorithm, in order: probe the branch; if it has not moved, only
/// refresh open proposals and return. Otherwise refresh open proposals first
/// (so one that just merged can override the merge below), compute the
/// upstream change set (via compare, or a tarball diff when compare is
/// truncated or huge, or re-baseline when the base commit is gone), three-way
/// merge each change into the working tree, record conflicts, advance the base
/// snapshot over every processed path (conflicted ones included) and consume
/// any merged proposals.
pub async fn pull(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
) -> Result<PullReport, RemoteError> {
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;

    let (head, new_etag) = match provider
        .branch_head(spec, state.ref_etag.as_deref())
        .await?
    {
        HeadProbe::Unchanged => {
            return settle_up_to_date(provider, spec, state_dir, state, None).await;
        }
        HeadProbe::Changed { head, etag } => (head, etag),
    };
    if head == state.base_commit {
        return settle_up_to_date(provider, spec, state_dir, state, Some(new_etag)).await;
    }

    // Refresh open proposals first so a just-merged one can override its own
    // files in the merge loop below.
    let transitions = refresh_proposals(provider, spec, &mut state).await?;
    let merged_this_pull: Vec<Proposal> = state
        .proposals
        .iter()
        .filter(|p| {
            p.status == ProposalStatus::Merged && transitions.iter().any(|(n, _)| *n == p.number)
        })
        .cloned()
        .collect();

    let base_commit_before = state.base_commit.clone();

    // Compute the domain-relative upstream change set, or re-baseline when the
    // base commit is no longer reachable upstream.
    let (edits, skipped_large) = match provider.compare(spec, &state.base_commit, &head).await {
        Ok(cmp) => upstream_edits(provider, spec, &state, &head, cmp).await?,
        Err(RemoteError::RepoNotFound { .. }) | Err(RemoteError::Api { status: 404, .. }) => {
            return rebaseline(
                provider,
                spec,
                domain_root,
                state_dir,
                state,
                head,
                new_etag,
                transitions,
            )
            .await;
        }
        Err(e) => return Err(e),
    };

    let mut applied = Vec::new();
    let mut merged = Vec::new();
    let mut new_conflicts = Vec::new();

    for edit in &edits {
        let rel = &edit.path;
        let base = state::read_base_file(state_dir, rel)?;
        let wt_path = checked_working_path(state_dir, domain_root, rel)?;
        let local = read_optional_file(&wt_path)?;
        let upstream = edit.content.as_deref();

        // A just-merged proposal whose recorded content hash still matches the
        // local file takes upstream unconditionally, so a reviewer's
        // amendments win over the proposed-but-unamended local copy. A user
        // who edited after sharing (hashes differ) falls through to merge.
        if proposal_override_applies(&merged_this_pull, rel, local.as_deref()) {
            match &edit.content {
                Some(bytes) => write_working_file(&wt_path, bytes)?,
                None => remove_working_file(&wt_path)?,
            }
            applied.push(rel.clone());
            continue;
        }

        match merge_file(base.as_deref(), local.as_deref(), upstream) {
            FileMerge::Apply(bytes) => {
                write_working_file(&wt_path, &bytes)?;
                applied.push(rel.clone());
                if is_three_way_merge(base.as_deref(), local.as_deref(), upstream) {
                    merged.push(rel.clone());
                }
            }
            FileMerge::Delete => {
                remove_working_file(&wt_path)?;
                applied.push(rel.clone());
            }
            FileMerge::Converged => {}
            FileMerge::Conflict(kind) => {
                // The local file is left untouched. Skip recording when an
                // identical open conflict already exists (same path, same
                // upstream commit), so a crash-and-retry cannot duplicate it.
                let already = state
                    .conflicts
                    .iter()
                    .any(|c| c.path == *rel && c.upstream_commit == head);
                if !already {
                    let id = state::new_conflict_id();
                    state::record_conflict_files(state_dir, &id, base.as_deref(), upstream)?;
                    let conflict = Conflict {
                        id,
                        path: rel.clone(),
                        kind,
                        base_commit: base_commit_before.clone(),
                        upstream_commit: head.clone(),
                        detected_at: Utc::now(),
                    };
                    state.conflicts.push(conflict.clone());
                    new_conflicts.push(conflict);
                }
            }
        }
    }

    // Advance the base snapshot for every processed path, conflicted paths
    // included: the conflict record preserves the pre-advance base copy, so
    // advancing here means resolving "theirs" later simply converges.
    for edit in &edits {
        match &edit.content {
            Some(bytes) => {
                state::write_base_file(state_dir, &edit.path, bytes)?;
                state.files.insert(edit.path.clone(), stamp(bytes));
            }
            None => {
                state::remove_base_file(state_dir, &edit.path)?;
                state.files.remove(&edit.path);
            }
        }
    }

    state.base_commit = head.clone();
    state.ref_etag = new_etag;
    state.last_checked = Some(Utc::now());

    // Consume merged proposals in memory, then persist base advance,
    // conflicts and history together in one atomic save so a crash cannot
    // leave a merged proposal half-consumed.
    for prop in &merged_this_pull {
        state.proposals.retain(|p| p.number != prop.number);
        let mut consumed = prop.clone();
        consumed.status = ProposalStatus::Merged;
        state.push_history(consumed);
    }
    state.save(state_dir)?;

    // Best-effort branch cleanup, after the state is durable; errors are
    // ignored entirely (the branch lingering upstream harms nothing).
    for prop in &merged_this_pull {
        let _ = provider.delete_branch(spec, &prop.branch).await;
    }

    Ok(PullReport {
        up_to_date: false,
        applied,
        merged,
        conflicts: new_conflicts,
        proposals: transitions,
        skipped_large,
        re_baselined: false,
    })
}

/// Reports where a domain stands relative to its origin.
///
/// Works fully offline when `probe` is `None`, reporting from origin state and
/// local change detection alone. With a provider, one conditional branch probe
/// fills `behind` and refreshes the stored etag and last-checked time, saved
/// only when the probe reports the branch has moved.
pub async fn status(
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    probe: Option<&dyn Provider>,
) -> Result<OriginStatusReport, RemoteError> {
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;

    let local = detect_local_changes(domain_root, &state.files)?;

    let mut behind = None;
    if let Some(provider) = probe {
        match provider
            .branch_head(spec, state.ref_etag.as_deref())
            .await?
        {
            HeadProbe::Unchanged => behind = Some(false),
            HeadProbe::Changed { head, etag } => {
                let is_behind = head != state.base_commit;
                behind = Some(is_behind);
                // Refresh the stored etag only while the branch still sits at
                // the base commit: `ref_etag` is the conditional marker for the
                // integrated head, and storing a moved head's etag here would
                // make a later `pull` see Unchanged and wrongly skip
                // integrating it. When behind, the marker is left as it is.
                if !is_behind {
                    state.ref_etag = etag;
                }
                state.last_checked = Some(Utc::now());
                state.save(state_dir)?;
            }
        }
    }

    let open_proposals = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .cloned()
        .collect();
    let declined_proposals = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Declined)
        .cloned()
        .collect();

    Ok(OriginStatusReport {
        repo: state.repo.clone(),
        branch: state.branch.clone(),
        base_commit: state.base_commit.clone(),
        behind,
        local_changes: local.changes.len(),
        skipped_large: local.skipped_large,
        open_proposals,
        declined_proposals,
        conflicts: state.conflicts.clone(),
        last_checked: state.last_checked,
    })
}

/// Handles the "nothing new upstream" outcome of a pull: refresh open
/// proposals (a proposal can still be declined without the branch moving),
/// persist any resulting change, and report up to date.
///
/// `new_etag` is `Some` when this was reached from a moved branch that
/// happened to equal the base commit (carrying a possibly-new etag to store)
/// and `None` when the conditional probe answered Unchanged (nothing to
/// update).
async fn settle_up_to_date(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state_dir: &Path,
    mut state: OriginState,
    new_etag: Option<Option<String>>,
) -> Result<PullReport, RemoteError> {
    let transitions = refresh_proposals(provider, spec, &mut state).await?;
    let mut dirty = !transitions.is_empty();
    if let Some(etag) = new_etag
        && state.ref_etag != etag
    {
        state.ref_etag = etag;
        dirty = true;
    }
    if dirty {
        state.last_checked = Some(Utc::now());
        state.save(state_dir)?;
    }
    Ok(PullReport {
        up_to_date: true,
        applied: Vec::new(),
        merged: Vec::new(),
        conflicts: Vec::new(),
        proposals: transitions,
        skipped_large: Vec::new(),
        re_baselined: false,
    })
}

/// Re-baselines a domain onto `head` when its base commit is gone upstream
/// (history rewritten, base garbage-collected). Downloads the head tree,
/// materializes only upstream files with no local counterpart (never
/// overwriting or deleting a local file that differs, which simply becomes a
/// local change against the new base), replaces the base snapshot wholesale
/// and keeps proposals and conflicts as they are.
#[allow(clippy::too_many_arguments)]
async fn rebaseline(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    mut state: OriginState,
    head: String,
    new_etag: Option<String>,
    transitions: Vec<(u64, ProposalStatus)>,
) -> Result<PullReport, RemoteError> {
    let bytes = provider.tarball(spec, &head).await?;
    let (extracted, skipped_large) = extract_tarball(&bytes, spec.subpath.as_deref())?;

    let mut applied = Vec::new();
    for (rel, content) in &extracted {
        let wt_path = checked_working_path(state_dir, domain_root, rel)?;
        if !wt_path.exists() {
            write_working_file(&wt_path, content)?;
            applied.push(rel.clone());
        }
    }

    state::replace_base_tree(state_dir, &extracted)?;
    state.files = extracted
        .iter()
        .map(|(rel, content)| (rel.clone(), stamp(content)))
        .collect();
    state.base_commit = head;
    state.ref_etag = new_etag;
    state.last_checked = Some(Utc::now());
    state.save(state_dir)?;

    Ok(PullReport {
        up_to_date: false,
        applied,
        merged: Vec::new(),
        conflicts: Vec::new(),
        proposals: transitions,
        skipped_large,
        re_baselined: true,
    })
}

/// Refreshes the status of every open proposal against the provider and
/// records the transitions. Merged proposals are marked but not yet consumed;
/// the caller decides when to move them to history. Returns the changed
/// proposals as `(number, new status)`.
async fn refresh_proposals(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &mut OriginState,
) -> Result<Vec<(u64, ProposalStatus)>, RemoteError> {
    let mut transitions = Vec::new();
    for prop in state.proposals.iter_mut() {
        if prop.status != ProposalStatus::Open {
            continue;
        }
        let new_status = match provider.proposal_state(spec, prop.number).await? {
            ProposalState::Open => continue,
            ProposalState::Merged => ProposalStatus::Merged,
            ProposalState::Declined => ProposalStatus::Declined,
        };
        prop.status = new_status;
        transitions.push((prop.number, new_status));
    }
    Ok(transitions)
}

/// Builds the domain-relative upstream change set from a compare result,
/// filtering to the domain subtree and enforcing the shared-file size cap on
/// upstream content. Falls back to a whole-tree tarball diff when the compare
/// is truncated or lists more than [`MAX_COMPARE_FILES`] files.
async fn upstream_edits(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &OriginState,
    head: &str,
    cmp: CompareResult,
) -> Result<(Vec<UpstreamEdit>, Vec<(String, u64)>), RemoteError> {
    let sub = spec.subpath.as_deref();

    let filtered: Vec<&UpstreamChange> = cmp
        .files
        .iter()
        .filter(|c| match &c.kind {
            ChangeKind::Renamed { previous } => {
                to_domain_relative(&c.path, sub).is_some()
                    || to_domain_relative(previous, sub).is_some()
            }
            _ => to_domain_relative(&c.path, sub).is_some(),
        })
        .collect();

    if cmp.truncated || filtered.len() > MAX_COMPARE_FILES {
        return upstream_edits_from_tarball(provider, spec, state, head).await;
    }

    let mut edits = Vec::new();
    let mut skipped_large = Vec::new();
    for change in filtered {
        match &change.kind {
            ChangeKind::Added | ChangeKind::Modified => {
                if let Some(rel) = to_domain_relative(&change.path, sub) {
                    push_blob_edit(
                        provider,
                        spec,
                        change.blob_sha.as_deref(),
                        rel,
                        &mut edits,
                        &mut skipped_large,
                    )
                    .await?;
                }
            }
            ChangeKind::Removed => {
                if let Some(rel) = to_domain_relative(&change.path, sub) {
                    edits.push(UpstreamEdit {
                        path: rel,
                        content: None,
                    });
                }
            }
            ChangeKind::Renamed { previous } => {
                // A rename is a removal of the old path and an addition of the
                // new one; either side may fall outside the subtree.
                if let Some(prev) = to_domain_relative(previous, sub) {
                    edits.push(UpstreamEdit {
                        path: prev,
                        content: None,
                    });
                }
                if let Some(rel) = to_domain_relative(&change.path, sub) {
                    push_blob_edit(
                        provider,
                        spec,
                        change.blob_sha.as_deref(),
                        rel,
                        &mut edits,
                        &mut skipped_large,
                    )
                    .await?;
                }
            }
        }
    }
    Ok((edits, skipped_large))
}

/// Fetches a changed file's content by blob sha and records it as an edit,
/// unless it exceeds [`MAX_SHARED_FILE_BYTES`], in which case it is reported as
/// skipped and neither written nor stamped into the base manifest.
async fn push_blob_edit(
    provider: &dyn Provider,
    spec: &OriginSpec,
    blob_sha: Option<&str>,
    rel: String,
    edits: &mut Vec<UpstreamEdit>,
    skipped_large: &mut Vec<(String, u64)>,
) -> Result<(), RemoteError> {
    let sha = blob_sha.ok_or_else(|| RemoteError::Api {
        status: 0,
        message: format!("the origin reported a change to {rel} without a blob to fetch"),
    })?;
    let content = provider.blob(spec, sha).await?;
    if content.len() as u64 > MAX_SHARED_FILE_BYTES {
        skipped_large.push((rel, content.len() as u64));
    } else {
        edits.push(UpstreamEdit {
            path: rel,
            content: Some(content),
        });
    }
    Ok(())
}

/// Builds the upstream change set by downloading the head tree and diffing it
/// against the base manifest, the fallback when a compare is truncated or too
/// large to page. Oversized entries the extractor skipped are excluded from
/// the removal set so they are not mistaken for deletions.
async fn upstream_edits_from_tarball(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &OriginState,
    head: &str,
) -> Result<(Vec<UpstreamEdit>, Vec<(String, u64)>), RemoteError> {
    let bytes = provider.tarball(spec, head).await?;
    let (extracted, skipped_large) = extract_tarball(&bytes, spec.subpath.as_deref())?;
    let skipped: BTreeSet<&str> = skipped_large.iter().map(|(p, _)| p.as_str()).collect();

    let mut edits = Vec::new();
    for (rel, content) in &extracted {
        match state.files.get(rel) {
            None => edits.push(UpstreamEdit {
                path: rel.clone(),
                content: Some(content.clone()),
            }),
            Some(base_stamp) => {
                if state::sha256_hex(content) != base_stamp.sha256 {
                    edits.push(UpstreamEdit {
                        path: rel.clone(),
                        content: Some(content.clone()),
                    });
                }
            }
        }
    }
    for rel in state.files.keys() {
        if !extracted.contains_key(rel) && !skipped.contains(rel.as_str()) {
            edits.push(UpstreamEdit {
                path: rel.clone(),
                content: None,
            });
        }
    }
    Ok((edits, skipped_large))
}

/// True when `rel` (a domain-relative path) belongs to a proposal that just
/// merged this pull and the local file's content still matches the hash the
/// proposal recorded. Such a file takes upstream unconditionally rather than
/// merging.
fn proposal_override_applies(merged: &[Proposal], rel: &str, local: Option<&[u8]>) -> bool {
    let Some(local_bytes) = local else {
        return false;
    };
    let local_sha = state::sha256_hex(local_bytes);
    merged.iter().any(|prop| {
        prop.files
            .iter()
            .any(|pf| pf.path.as_str() == rel && pf.sha256.as_deref() == Some(local_sha.as_str()))
    })
}

/// True when a [`FileMerge::Apply`] came from a real three-way text merge
/// rather than a plain take of upstream content, detected by the call shape:
/// both local and upstream present and both differing from the base (the
/// add/add and edit/edit cases the merge engine runs through `diffy`). A plain
/// take (local unchanged from base) fails this and is only "applied".
fn is_three_way_merge(base: Option<&[u8]>, local: Option<&[u8]>, upstream: Option<&[u8]>) -> bool {
    local.is_some() && upstream.is_some() && local != base && upstream != base
}

/// The base stamp for `content`: its sha-256 digest and byte length.
fn stamp(content: &[u8]) -> BaseStamp {
    BaseStamp {
        sha256: state::sha256_hex(content),
        size: content.len() as u64,
    }
}

/// Maps a repo-relative path to its domain-relative form under `subpath`, or
/// `None` when it lies outside the subtree. Mirrors the prefix stripping
/// [`crate::archive::extract_tarball`] applies, so the compare and tarball
/// paths agree on which files belong to the domain.
fn to_domain_relative(repo_rel: &str, subpath: Option<&str>) -> Option<String> {
    match subpath {
        None => Some(repo_rel.to_string()),
        Some(sub) => {
            let prefix = format!("{}/", sub.trim_matches('/'));
            repo_rel.strip_prefix(&prefix).map(str::to_string)
        }
    }
}

/// Reads a working-tree file, returning `None` when it does not exist.
fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>, RemoteError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Writes `content` to a working-tree file, creating parent directories.
fn write_working_file(path: &Path, content: &[u8]) -> Result<(), RemoteError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Removes a working-tree file; removing one already gone is not an error.
fn remove_working_file(path: &Path) -> Result<(), RemoteError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Validates `rel` through [`crate::state`]'s path chokepoint and returns the
/// working-tree path it names under `domain_root`.
///
/// The validation is the security-critical step: [`crate::state::base_path`]
/// rejects any `rel` shaped like a traversal, an absolute path or a Windows
/// drive-prefix attempt with [`RemoteError::State`], so upstream content can
/// never steer a working-tree write outside `domain_root`. Once validated,
/// `rel` is a plain forward-slash relative path whose components are joined
/// onto `domain_root`.
fn checked_working_path(
    state_dir: &Path,
    domain_root: &Path,
    rel: &str,
) -> Result<PathBuf, RemoteError> {
    // Funnel the path through state.rs's validation; the returned base path is
    // discarded, only its acceptance of `rel` matters here.
    state::base_path(state_dir, rel)?;
    let mut path = domain_root.to_path_buf();
    for part in rel.split('/') {
        if !part.is_empty() {
            path.push(part);
        }
    }
    Ok(path)
}
