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
use crystalline_core::manifest::Manifest;
use crystalline_core::parse_engram;

use crate::archive::{extract_repo_subtree, extract_tarball};
use crate::changes::{LocalChange, MAX_SHARED_FILE_BYTES, detect_local_changes};
use crate::error::RemoteError;
use crate::merge::{FileMerge, merge_file};
use crate::provider::{
    ChangeKind, CompareResult, HeadProbe, OriginSpec, ProposalRequest, ProposalState, Provider,
    TreeWrite, UpstreamChange,
};
use crate::state::{
    self, BaseStamp, Conflict, OriginState, Proposal, ProposalStatus, ProposedChange, ProposedFile,
};

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
    /// How many files were written to the working tree (for an adopted
    /// target, only the upstream files that had no local counterpart).
    pub files_written: usize,
    /// How many extracted upstream files are engrams (`.md`).
    pub engrams: usize,
    /// Upstream files skipped for exceeding [`MAX_SHARED_FILE_BYTES`], each
    /// with its size in bytes.
    pub skipped_large: Vec<(String, u64)>,
    /// Whether the target already held files and was connected in place.
    pub adopted: bool,
    /// How many local files differ from the new base snapshot right after
    /// subscribing: kept local edits plus local-only files, all shareable or
    /// updatable through the ordinary flows. Always 0 for a fresh download.
    pub local_changes: usize,
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
    /// Numbers of open proposals whose live branch head no longer matches the
    /// recorded head_commit: a reviewer amended the branch. Empty when the
    /// live list was not consulted (offline, no probe, or the list call
    /// failed and status degraded to local state).
    pub amended_upstream: Vec<u64>,
    /// The stack number linking this domain's open chain on the forge, `None`
    /// when nothing is stacked (zero or one open layer, the fallback path, or
    /// a chain whose link is still owed).
    pub stack_number: Option<u64>,
    /// Numbers of Declined layers that still carry open layers above them: the
    /// chain is wedged on them, and the next share or withdrawal repairs it
    /// (see [`crate::state::OriginState::proposals`] for the bottom-to-top
    /// order this walks).
    pub stack_wedged: Vec<u64>,
    /// Whether a chain repair was interrupted and is still owed. The next
    /// stacked share or withdrawal finishes it before its own work.
    pub repair_pending: bool,
    /// Whether a stack link this machine owes the forge is still unpaid: every
    /// pull request exists, they are simply not grouped. A status with a probe
    /// tries to settle it first, so this reads false once the retry lands.
    pub stack_link_pending: bool,
}

/// What [`propose`] did with a domain's local changes.
#[derive(Debug, Clone, PartialEq)]
pub enum ProposeOutcome {
    /// A new pull request was opened.
    Proposed(ProposeReport),
    /// The one open proposal was updated in place: same number, same URL,
    /// a new commit on the same branch. The report's number/url/branch name
    /// the existing proposal; added/updated/deleted are the fresh change list.
    Updated(ProposeReport),
    /// Success-shaped, not an error: the team already has everything this
    /// domain knows, so there was nothing to open a pull request for.
    NothingToShare {
        /// Working-tree files skipped for exceeding
        /// [`MAX_SHARED_FILE_BYTES`], each with its size in bytes.
        skipped_large: Vec<(String, u64)>,
    },
    /// A reviewer pushed commits onto the proposal branch; nothing was
    /// written. The caller relays the guidance: let the reviewer finish
    /// (merge on GitHub) or withdraw and re-share.
    ProposalDiverged {
        /// The open proposal's number.
        number: u64,
        /// The web URL a human reviews the proposal at.
        url: String,
        /// The branch a reviewer moved out from under us.
        branch: String,
    },
}

/// What [`propose`] did when it opened a pull request from local changes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposeReport {
    /// The web URL a human reviews the proposal at.
    pub url: String,
    /// The proposal number.
    pub number: u64,
    /// The branch carrying the proposed commits.
    pub branch: String,
    /// Domain-relative paths of files added by the proposal.
    pub added: Vec<String>,
    /// Domain-relative paths of files modified by the proposal.
    pub updated: Vec<String>,
    /// Domain-relative paths of files deleted by the proposal.
    pub deleted: Vec<String>,
    /// Working-tree files skipped for exceeding [`MAX_SHARED_FILE_BYTES`],
    /// each with its size in bytes.
    pub skipped_large: Vec<(String, u64)>,
    /// A one-line, human-readable summary of the change mix (also the first
    /// line of the generated proposal body, when the caller supplies no
    /// description of their own).
    pub summary: String,
    /// The stack number linking this proposal's chain on the forge. `None`
    /// off the stacked path, and `None` on a stacked layer whose linking call
    /// failed (the chain is degraded, which
    /// [`crate::state::OriginState::stack_link_pending`] carries).
    pub stack_number: Option<u64>,
    /// Where this proposal sits in the open chain, as `(position, open
    /// layers)` with a 1-based position. `None` off the stacked path.
    pub stack_position: Option<(usize, usize)>,
}

/// What [`withdraw`] did with a declined or still-open proposal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WithdrawReport {
    /// The proposal that was withdrawn.
    pub number: u64,
    /// True when a live pull request was closed on the forge (an Open
    /// proposal); false for a Declined one, already closed by a reviewer.
    pub closed: bool,
    /// Domain-relative paths restored to base-tree content (revert only): a
    /// proposed `Modified` or `Deleted` file whose local copy still matched
    /// what was proposed.
    pub restored: Vec<String>,
    /// Domain-relative paths deleted (a proposed `Added` file whose local
    /// copy still matched what was proposed; revert only).
    pub deleted: Vec<String>,
    /// Paths left untouched because the local file diverged since sharing:
    /// newer work is never destroyed.
    pub skipped_diverged: Vec<String>,
    /// Paths a revert could not restore because their pre-share content is
    /// nowhere to be had: the trunk snapshot never carried the file (a layer
    /// below the withdrawn one added it) and that layer's record names no blob
    /// to fetch it back from, or the fetch itself failed. The path is left
    /// exactly as it stands rather than failing the withdrawal.
    pub skipped_reverts: Vec<String>,
    /// True when the chain around the withdrawn layer was repaired: the stack
    /// was dissolved and, where two layers survived, recreated, with every
    /// layer above the hole replayed onto the one below it.
    pub repaired: bool,
    /// The NEW stack number a repair's recreate allocated, `None` when no
    /// stack was recreated (fewer than two survivors, or no repair at all).
    /// A dissolve plus recreate never keeps the old number: stack numbers come
    /// off the same sequence as issue and pull request numbers.
    pub restacked: Option<u64>,
}

/// How to settle one recorded conflict, passed to [`resolve`].
#[derive(Debug, Clone, Copy)]
pub enum Resolution<'a> {
    /// Keep the local copy: the working tree is left untouched, an ordinary
    /// local change against the advanced base, shareable on the next
    /// `propose`.
    Mine,
    /// Take upstream's copy: writes the recorded upstream content, or
    /// deletes the local file when upstream had none (an `EditDelete`
    /// conflict).
    Theirs,
    /// Write this caller-supplied content as the resolved merge.
    Merged(&'a [u8]),
}

/// What [`resolve`] did with one conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveReport {
    /// The domain-relative path that was resolved.
    pub resolved: String,
    /// How many conflicts remain open after this one.
    pub remaining: usize,
}

/// One upstream change to integrate, already normalized to a domain-relative
/// path. `content` is the new content, or `None` when upstream removed the
/// file.
struct UpstreamEdit {
    path: String,
    content: Option<Vec<u8>>,
}

/// Connects a domain to its origin for the first time: downloads the tracked
/// subtree at the branch head, records it as the base snapshot and saves a
/// fresh [`OriginState`].
///
/// The origin must look like a domain (a `MANIFEST.md` at the subtree root),
/// checked before anything is written, so a rejected subscribe leaves the
/// disk untouched. An absent or empty `domain_root` receives the full tree; a
/// non-empty one is adopted in place: only upstream files with no local
/// counterpart are materialized and no local file is ever overwritten or
/// deleted, so an existing file that differs from the origin simply becomes
/// an ordinary local change against the new base, ready to share or update.
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

    // Materialize the out-of-subtree artifact mirror from the same tarball,
    // driven by the MANIFEST's own provisioning decls at the fetched commit.
    // Done before a working-tree byte is written so a decl escaping the
    // repository root fails the whole subscribe with the disk untouched, the
    // same fail-before-write stance the MANIFEST check above takes.
    let manifest_source = extracted
        .get("MANIFEST.md")
        .and_then(|bytes| std::str::from_utf8(bytes).ok());
    write_artifact_mirror(state_dir, spec.subpath.as_deref(), manifest_source, &bytes)?;

    let adopted = domain_root.exists() && std::fs::read_dir(domain_root)?.next().is_some();

    // One pass writes the base snapshot and stamps the manifest from the same
    // bytes, so the base is never read back. The working tree only receives
    // upstream files that do not exist locally: on a fresh target that is the
    // whole tree, on an adopted one every local file stays exactly as it was.
    let mut files = BTreeMap::new();
    let mut files_written = 0usize;
    for (rel, content) in &extracted {
        let wt_path = checked_working_path(state_dir, domain_root, rel)?;
        if !wt_path.exists() {
            write_working_file(&wt_path, content)?;
            files_written += 1;
        }
        state::write_base_file(state_dir, rel, content)?;
        files.insert(rel.clone(), stamp(content));
    }

    let engrams = extracted.keys().filter(|p| p.ends_with(".md")).count();
    let local_changes = if adopted {
        detect_local_changes(domain_root, &files)?.changes.len()
    } else {
        0
    };

    let mut origin_state = OriginState::new(spec.repo.clone(), spec.branch.clone());
    origin_state.base_commit = head.clone();
    origin_state.ref_etag = etag;
    origin_state.last_checked = Some(Utc::now());
    origin_state.files = files;
    origin_state.save(state_dir)?;

    Ok(SubscribeReport {
        base_commit: head,
        files_written,
        engrams,
        skipped_large,
        adopted,
        local_changes,
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
///
/// Consuming a merged layer is also what puts a stacked chain back together
/// after the forge moved it: merging one layer rebases every layer above it,
/// so once the merge is in history [`adopt_rebased_layers`] takes each
/// survivor's new head where the move carried none of that layer's own work.
/// Without that a plain merge would leave the whole chain looking amended.
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
    // files in the merge loop below. Every Merged record is collected,
    // however it came to be flagged, not only the ones that flipped in this
    // refresh: `status` can flip a record to Merged between pulls, and such a
    // record is un-consumed work this pull has to finish.
    let (transitions, _) = refresh_proposals(provider, spec, &mut state).await?;
    let merged_to_consume: Vec<Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Merged)
        .cloned()
        .collect();

    let base_commit_before = state.base_commit.clone();

    // Compute the domain-relative upstream change set, or re-baseline when the
    // base commit is no longer reachable upstream.
    let (edits, skipped_large) = match provider.compare(spec, &state.base_commit, &head).await {
        Ok(cmp) => {
            // Decide from the compare (before subpath filtering) whether the
            // artifact mirror could have moved: a change under a declared
            // out-of-subtree root, or a change to the subtree MANIFEST whose
            // decls may themselves have shifted.
            let refresh_needed = mirror_refresh_needed(&cmp, state_dir, spec.subpath.as_deref())?;
            let (edits, skipped_large, fallback_tarball) =
                upstream_edits(provider, spec, &state, &head, cmp).await?;
            // The mirror rebuild runs before the merge loop and the base
            // advance below, so a later commit that turned a decl hostile
            // fails the whole pull with the previous mirror and base left
            // intact. The tarball fallback already fetched the bytes, so it
            // refreshes from those unconditionally; a clean compare fetches a
            // tarball of its own only when the decision above asked for it.
            match fallback_tarball {
                Some(bytes) => {
                    refresh_artifact_mirror(state_dir, spec.subpath.as_deref(), &bytes)?;
                }
                None if refresh_needed => {
                    let bytes = provider.tarball(spec, &head).await?;
                    refresh_artifact_mirror(state_dir, spec.subpath.as_deref(), &bytes)?;
                }
                None => {}
            }
            (edits, skipped_large)
        }
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
        if proposal_override_applies(&merged_to_consume, rel, local.as_deref()) {
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

    // Consume merged proposals in memory, adopt the rebase the forge performed
    // on whatever still stands above them, then persist base advance,
    // conflicts and history together in one atomic save so a crash cannot
    // leave a merged proposal half-consumed.
    let consumed = consume_merged(&mut state);
    if !consumed.is_empty()
        && state
            .proposals
            .iter()
            .any(|p| p.status == ProposalStatus::Open)
    {
        adopt_rebased_layers(provider, spec, &mut state).await;
    }
    state.save(state_dir)?;

    // Best-effort branch cleanup, after the state is durable; errors are
    // ignored entirely (the branch lingering upstream harms nothing).
    for prop in &consumed {
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

/// Moves every Merged record out of the chain and into history, returning
/// them so the caller can delete their branches once its state is durable.
///
/// The one rule beyond the move: when the last open layer leaves this way, the
/// stack number goes with it. A number naming a chain with nothing open in it
/// names nothing, and leaving it behind would have the next share dissolve a
/// stack that is already finished.
fn consume_merged(state: &mut OriginState) -> Vec<Proposal> {
    let merged: Vec<Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Merged)
        .cloned()
        .collect();
    if merged.is_empty() {
        return merged;
    }
    state
        .proposals
        .retain(|p| p.status != ProposalStatus::Merged);
    for prop in &merged {
        state.push_history(prop.clone());
    }
    if !state
        .proposals
        .iter()
        .any(|p| p.status == ProposalStatus::Open)
    {
        state.stack_number = None;
    }
    merged
}

/// Adopts the rebase the forge performs on a chain when a layer below it
/// merges, so a surviving layer is not reported as amended for work nobody
/// did.
///
/// Merging one layer of a stack moves every layer above it: GitHub rebases
/// each survivor onto the new trunk, which gives it a new head sha carrying
/// the same content. Left alone, that reads exactly like a reviewer's
/// amendment ([`status`]'s `amended_upstream`), and a share would refuse with
/// [`ProposeOutcome::ProposalDiverged`] over a move this domain asked for.
///
/// Telling the two apart is one comparison per moved layer: a rebase changes
/// what the layer's commit stands on and nothing the layer itself proposes, so
/// a compare from the recorded head to the live one that touches NONE of the
/// layer's own files is a rebase, and the record simply takes the new head.
/// The spec's blob-by-blob check is the same statement over the verbs this
/// provider actually has. Anything else leaves the record exactly as it
/// stands: a compare that touches an own path is a real amendment and must
/// stay visible, a truncated compare is no answer at all (the next poll asks
/// again), and a gone branch ref is the settling paths' business rather than
/// this one's.
///
/// Only ever called after a merge was consumed, and best effort throughout: a
/// failure here leaves the layer looking amended, which is the safe reading
/// and one the user can act on, so it never fails the pull that healed
/// everything else.
async fn adopt_rebased_layers(provider: &dyn Provider, spec: &OriginSpec, state: &mut OriginState) {
    let sub = spec.subpath.as_deref();
    // What each layer stands on, walked bottom to top: the freshly advanced
    // trunk for the bottom layer, the layer below for every other. Recording
    // it keeps the chain's recorded shape intact, which is what
    // `stack_shape_broken` reads to decide a repair is owed.
    let mut parent_head = state.base_commit.clone();
    for index in 0..state.proposals.len() {
        if state.proposals[index].status != ProposalStatus::Open {
            continue;
        }
        let layer = state.proposals[index].clone();
        match rebased_head(provider, spec, &layer, sub).await {
            Some(live) => {
                let record = &mut state.proposals[index];
                record.head_commit = Some(live.clone());
                record.base_commit = Some(parent_head);
                parent_head = live;
            }
            None => {
                // A layer left alone keeps its recorded head, so the layer
                // above it records a base that is stale upstream. That is the
                // consistent reading rather than a guess: the chain stays
                // self-consistent (`stack_shape_broken` sees each base still
                // matching the head below it, so no repair is ordered) and the
                // divergence surfaces where it belongs, as `amended_upstream`
                // on the layer that really moved.
                if let Some(head) = layer.head_commit {
                    parent_head = head;
                }
            }
        }
    }
}

/// The live head of `layer`'s branch when a rebase is the only thing that
/// moved it, `None` in every other case (see [`adopt_rebased_layers`] for what
/// those cases are and why each one is left alone).
async fn rebased_head(
    provider: &dyn Provider,
    spec: &OriginSpec,
    layer: &Proposal,
    sub: Option<&str>,
) -> Option<String> {
    let recorded = layer.head_commit.as_deref()?;
    let live = match provider.branch_ref(spec, &layer.branch).await {
        Ok(Some(live)) => live,
        Ok(None) => return None,
        Err(e) => {
            tracing::debug!(
                "reading proposal #{}'s branch failed; leaving its recorded head alone: {e}",
                layer.number
            );
            return None;
        }
    };
    // A head this machine put there - settled or half-pushed - is nothing to
    // adopt: the share paths own both.
    if head_is_ours(layer, &live) {
        return None;
    }
    let cmp = match provider.compare(spec, recorded, &live).await {
        Ok(cmp) => cmp,
        Err(e) => {
            tracing::debug!(
                "comparing proposal #{}'s heads failed; leaving its recorded head alone: {e}",
                layer.number
            );
            return None;
        }
    };
    if cmp.truncated {
        return None;
    }
    let own: BTreeSet<String> = layer
        .files
        .iter()
        .map(|file| to_repo_relative(&file.path, sub))
        .collect();
    let touches_own = cmp.files.iter().any(|change| {
        own.contains(&change.path)
            || matches!(&change.kind, ChangeKind::Renamed { previous } if own.contains(previous))
    });
    if touches_own { None } else { Some(live) }
}

/// Reports where a domain stands relative to its origin.
///
/// Works fully offline when `probe` is `None`, reporting from origin state and
/// local change detection alone. With a provider, one conditional branch probe
/// fills `behind` and refreshes the stored etag and last-checked time, saved
/// only when the probe reports the branch has moved.
///
/// With a provider and at least one open proposal, exactly one
/// [`Provider::list_open_proposals`] call brings the proposal picture up to
/// date: a record still on the list whose live head differs from the recorded
/// `head_commit` is reported in `amended_upstream`, and a record that left the
/// list is classified by a single state GET and flipped to Merged or Declined
/// in saved state. A flipped record is not consumed - it stays in
/// `state.proposals` and simply leaves this report's open list; moving it to
/// history stays [`pull`]'s job. The list call is best-effort: a failure logs
/// and degrades to the offline picture (local statuses unchanged, no amended
/// flags), never failing the status.
///
/// The four stack fields are read off local state, with one exception: an
/// owed stack link is settled before they are read ([`retry_stack_link`]), so
/// a status that could pay the debt reports the healed truth rather than the
/// debt. That needs a provider and `github.stacks` on, and it costs nothing
/// when nothing is owed - the flag is tested before the capability is.
///
/// `stacks_allowed` carries `github.stacks` through to that retry; nothing
/// else in the report depends on it.
pub async fn status(
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    probe: Option<&dyn Provider>,
    stacks_allowed: bool,
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

    // A link this machine owes the forge is settled before anything is
    // reported, so a status names the chain as it really stands rather than as
    // the last failed call left it. The debt is tested first: an origin that
    // owes nothing never asks whether the forge serves stacks at all.
    //
    // Nothing on this path may fail the status, the capability check included:
    // its one error is the save that caches a fresh verdict, and a status that
    // could not write that cache still has every answer it came for.
    if let Some(provider) = probe
        && state.stack_link_pending
    {
        match stacks_available(provider, spec, &mut state, state_dir, stacks_allowed).await {
            Ok(true) => retry_stack_link(provider, spec, &mut state, state_dir).await,
            Ok(false) => {}
            Err(e) => {
                tracing::debug!("the stacks probe failed; reporting the owed link as it is: {e}");
            }
        }
    }

    // One live list call, made only when there is an open record for it to
    // say something about, tells two things at once: which open proposals a
    // reviewer amended out from under us, and which ones left the open list
    // upstream (merged or declined elsewhere). A failure degrades to today's
    // offline behavior rather than failing the status.
    let mut amended_upstream = Vec::new();
    if let Some(provider) = probe
        && state
            .proposals
            .iter()
            .any(|p| p.status == ProposalStatus::Open)
    {
        match provider.list_open_proposals(spec).await {
            Ok(open_list) => {
                let mut dirty = false;
                for prop in state.proposals.iter_mut() {
                    if prop.status != ProposalStatus::Open {
                        continue;
                    }
                    match open_list.iter().find(|o| o.number == prop.number) {
                        Some(live) => {
                            // `head_is_ours` rather than a bare comparison, so
                            // this machine's own interrupted update is not
                            // reported to the user as a reviewer's amendment.
                            if !head_is_ours(prop, &live.head_sha) {
                                amended_upstream.push(prop.number);
                            }
                        }
                        None => {
                            // Gone from the open list: one GET classifies it.
                            // The flip is applied and saved, but NOT consumed -
                            // consumption stays a pull concern; status only
                            // makes the user see the truth.
                            match provider.proposal_state(spec, prop.number).await {
                                Ok(refreshed) => {
                                    let new_status = match refreshed {
                                        ProposalState::Merged => ProposalStatus::Merged,
                                        ProposalState::Declined => ProposalStatus::Declined,
                                        ProposalState::Open => continue,
                                    };
                                    prop.status = new_status;
                                    dirty = true;
                                }
                                Err(e) => {
                                    // Same degradation as a failed list: the
                                    // record keeps its local status rather
                                    // than failing the status.
                                    tracing::debug!(
                                        "classifying proposal #{} failed; keeping its local status: {e}",
                                        prop.number
                                    );
                                }
                            }
                        }
                    }
                }
                if dirty {
                    state.save(state_dir)?;
                }
            }
            Err(e) => {
                // Degrade to today's offline behavior: local statuses
                // unchanged, no amended flags.
                tracing::debug!("open-proposal list failed; reporting local state: {e}");
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
        amended_upstream,
        stack_number: state.stack_number,
        stack_wedged: under_an_open_layer(&state)
            .iter()
            .filter(|p| p.status == ProposalStatus::Declined)
            .map(|p| p.number)
            .collect(),
        repair_pending: state.repair_pending,
        stack_link_pending: state.stack_link_pending,
    })
}

/// The recorded layers that still carry an OPEN layer above them, in chain
/// order.
///
/// One walk answers both questions this crate asks about a closed layer left
/// inside a chain, because they are the same question: is there still open
/// work standing on it? A Declined one there is a wedge a repair has to close
/// ([`OriginStatusReport::stack_wedged`]); a Merged one there is content a
/// replay would rebuild without ([`merged_layer_blocking_repair`]). Above the
/// topmost open layer neither is true: nothing is stacked on it, so nothing is
/// replayed over it.
fn under_an_open_layer(state: &OriginState) -> &[Proposal] {
    match state
        .proposals
        .iter()
        .rposition(|p| p.status == ProposalStatus::Open)
    {
        Some(top) => &state.proposals[..top],
        None => &[],
    }
}

/// Everything a share call carries besides the domain itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShareOptions<'a> {
    /// The proposal's title (a new layer) or commit message (an amend).
    pub title: Option<&'a str>,
    /// The proposal's description body.
    pub description: Option<&'a str>,
    /// Amend this open layer instead of stacking a new proposal.
    pub proposal: Option<u64>,
    /// Whether stacked proposals may be used at all (github.stacks config).
    pub stacks_allowed: bool,
}

/// The cached stacks verdict for this origin, probing once when unknown.
///
/// A forge either serves stacks or it does not, and that answer does not
/// change between two shares, so it is asked once per origin and kept in
/// origin state. `stacks_allowed = false` (the `github.stacks` config) is the
/// user saying no before the question is worth asking: it short-circuits
/// without probing and without consulting or touching the cache, so turning
/// the setting back on finds the cache exactly as it was.
///
/// Only two answers are verdicts worth remembering: the forge served the
/// listing, or it said it does not serve stacks at all. Any other failure -
/// offline, a 500, a timeout - is this probe failing rather than the forge
/// answering, so the share falls back for this run alone and nothing is
/// cached: the next share asks again. An optional capability is never worth
/// failing a share over.
async fn stacks_available(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &mut OriginState,
    state_dir: &Path,
    stacks_allowed: bool,
) -> Result<bool, RemoteError> {
    if !stacks_allowed {
        return Ok(false);
    }
    if let Some(cached) = state.stacks_available {
        return Ok(cached);
    }
    let verdict = match provider.list_stacks(spec, None).await {
        Ok(_) => true,
        Err(RemoteError::StacksUnsupported) => false,
        Err(e) => {
            tracing::debug!("the stacks probe failed; sharing without stacking this time: {e}");
            return Ok(false);
        }
    };
    state.stacks_available = Some(verdict);
    state.save(state_dir)?;
    Ok(verdict)
}

/// Whether this origin's open chain is one more layers may be stacked onto.
///
/// Zero or one open proposal is trivially compatible: there is no chain to be
/// inconsistent with. Beyond that the chain must be one this machine knows as
/// a stack - a recorded stack number, or a link it still owes the forge. A
/// multi-open state carrying neither predates stacking, and its shares take
/// the fallback flows.
fn chain_is_stacked(state: &OriginState) -> bool {
    let open = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .count();
    open <= 1 || state.stack_number.is_some() || state.stack_link_pending
}

/// The chain tip: what the working tree would look like if every open layer
/// were already merged, as a base snapshot manifest
/// [`detect_local_changes`] can compare against.
///
/// This is the base a stacked share detects against, and it is a base rather
/// than a filter on purpose. A layer must record ITS delta and nothing else:
/// with the trunk as the base, every layer re-proposes what the layers below
/// it already carry, a share with nothing new opens an empty layer instead of
/// reporting nothing to share, and a delete of a file only a lower layer
/// holds is invisible, because the trunk never carried that file at all.
///
/// The overlay is the trunk snapshot with each OPEN layer's own recorded
/// files laid over it in `state.proposals` order (bottom to top, which is the
/// order the layers stack in): an added or modified path takes that layer's
/// recorded digest and size, a deleted path leaves the map. Tasks 7, 8 and 9
/// reuse this - an amend recomputes one layer's delta against the tip below
/// it, a withdrawal excises a layer from the chain, and a pull adopts what a
/// merged layer brought in.
fn effective_tip_files(state: &OriginState) -> BTreeMap<String, BaseStamp> {
    let open: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .collect();
    tip_files_over(&state.files, &open)
}

/// [`effective_tip_files`] over an explicit layer list, for a caller that
/// knows some recorded layer is about to leave the chain (a preview mirroring
/// the settle a share would perform on a gone branch ref).
///
/// A layer entry with no recorded digest is skipped rather than guessed at:
/// the path keeps its trunk stamp, so the file reads as this share's own work
/// again. That costs a redundant tree write and never loses a change. Only a
/// record written before those fields existed can be in that shape, and such
/// a record cannot sit on a stacked chain, since stacking and the fields
/// shipped together. A recorded size is used when present and falls back to
/// the trunk's, which can at worst produce that same redundant write.
fn tip_files_over(
    base: &BTreeMap<String, BaseStamp>,
    layers: &[&Proposal],
) -> BTreeMap<String, BaseStamp> {
    let mut tip = base.clone();
    for layer in layers {
        for file in &layer.files {
            match file.change {
                ProposedChange::Added | ProposedChange::Modified => {
                    let Some(sha256) = file.sha256.clone() else {
                        continue;
                    };
                    let size = file
                        .size
                        .or_else(|| base.get(&file.path).map(|stamp| stamp.size))
                        .unwrap_or_default();
                    tip.insert(file.path.clone(), BaseStamp { sha256, size });
                }
                ProposedChange::Deleted => {
                    tip.remove(&file.path);
                }
            }
        }
    }
    tip
}

/// Settles a stack link this machine still owes the forge, best effort.
///
/// A layer whose `create_stack` or `extend_stack` call failed leaves the
/// chain degraded rather than wrong: every pull request exists, they are
/// simply not grouped. [`crate::state::OriginState::stack_link_pending`]
/// records that debt, and this pays it off at the start of the next share,
/// withdrawal or status, before that operation does any work of its own.
///
/// Which call settles it follows from what is already known: with no stack
/// number the whole open chain is grouped in one `create_stack`; with one,
/// the forge is asked which members it holds and only the missing layers are
/// added. Fewer than two open layers is not a chain at all, so the flag is
/// simply cleared. Every failure here is swallowed and logged - the debt
/// stays recorded and the next operation tries again.
async fn retry_stack_link(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &mut OriginState,
    state_dir: &Path,
) {
    if !state.stack_link_pending {
        return;
    }
    let open: Vec<u64> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .map(|p| p.number)
        .collect();
    if open.len() < 2 {
        state.stack_link_pending = false;
        if let Err(e) = state.save(state_dir) {
            tracing::debug!("clearing the stale stack-link debt failed: {e}");
        }
        return;
    }

    let linked = match state.stack_number {
        None => provider
            .create_stack(spec, &open)
            .await
            .map(|info| Some(info.number)),
        Some(number) => match provider.list_stacks(spec, Some(open[0])).await {
            Ok(stacks) => {
                let known: Vec<u64> = stacks
                    .iter()
                    .find(|s| s.number == number)
                    .map(|s| s.members.iter().map(|m| m.number).collect())
                    .unwrap_or_default();
                let missing: Vec<u64> = open
                    .iter()
                    .copied()
                    .filter(|n| !known.contains(n))
                    .collect();
                if missing.is_empty() {
                    Ok(Some(number))
                } else {
                    provider
                        .extend_stack(spec, number, &missing)
                        .await
                        .map(|_| Some(number))
                }
            }
            Err(e) => Err(e),
        },
    };

    match linked {
        Ok(number) => {
            state.stack_number = number;
            state.stack_link_pending = false;
            if let Err(e) = state.save(state_dir) {
                tracing::debug!("recording the settled stack link failed: {e}");
            }
        }
        Err(e) => {
            tracing::debug!("the owed stack link still fails; the chain stays degraded: {e}");
        }
    }
}

/// Proposes a domain's local changes as a pull request against its origin.
///
/// `domain_name` is the domain's registered name, the contract's sole
/// authority over what a share calls the domain: it seeds the branch slug
/// (see [`share_branch_name`]) and the generated title and body, regardless
/// of what `domain_root`'s own directory happens to be named.
///
/// The algorithm, in order: pull first, so every proposal is opened
/// mergeable (any upstream movement is integrated onto the working tree
/// before anything is proposed); refuse with [`RemoteError::ConflictsPending`]
/// when any conflict is open afterward, new or pre-existing, before a single
/// provider write call is made; detect local changes against the
/// now-current base, reporting [`ProposeOutcome::NothingToShare`] when there
/// are none; supersede any declined proposal (moved to history keeping
/// `Declined`, its branch best-effort deleted); then either update the one
/// open proposal in place or create a new one.
///
/// Two shapes of chain live behind that last step. On a forge that serves
/// stacks, with `stacks_allowed` on, a share never rewrites what is already
/// open: it opens a NEW proposal layered on the top open one (its tree built
/// on that layer's head, its commit carrying exactly one parent, its pull
/// request targeting that layer's branch) and links it into the forge's
/// stack, so every share stays reviewable on its own. A layer is detected
/// against the chain TIP rather than the trunk (see [`effective_tip_files`]),
/// so it records its own delta and nothing the layers below it already carry:
/// a share with nothing new is [`ProposeOutcome::NothingToShare`] even with a
/// chain open, and retiring a file only a lower layer holds is a change like
/// any other. Everything below is the fallback that path falls back to, and
/// the flow a chain this machine does not know as a stack keeps.
///
/// `options.proposal` names a layer to amend instead, the verb a reviewer's
/// feedback is answered with: the share lands on that layer's own branch and
/// [`amend_layer`] replays every layer above it onto the amended head, so the
/// chain heals itself rather than needing a manual re-base. A number that is
/// not an open layer is refused with the open layers listed. Off the stacked
/// path the number can only name the one living proposal, and amending it is
/// exactly the in-place update this function has always done.
///
/// On the fallback path a domain has at most one open proposal at a time.
/// When one is open, this
/// pushes a fresh commit onto its branch and rewrites its body rather than
/// opening a second pull request ([`ProposeOutcome::Updated`]), and refuses
/// with [`ProposeOutcome::ProposalDiverged`] - before any provider write -
/// when a reviewer has moved the branch away from every head this machine
/// recorded (see [`head_is_ours`]: the settled head, a half-finished push this
/// machine announced before making it, or no recorded head at all). An open
/// record whose branch ref is gone is settled from the proposal's own state
/// (an open pull request with no branch counts as declined) and the share
/// falls through to a fresh creation.
///
/// Creating uploads each added or modified file's content as a blob, builds a
/// tree from the base commit with every domain-relative path re-prefixed to
/// its repo-relative form (see [`to_repo_relative`]), commits it, opens a
/// share branch named per [`share_branch_name`] and opens the pull request;
/// finally the proposal is recorded in state (status `Open`) and saved. Local
/// files are never touched: a share only ever reads them.
pub async fn propose(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    domain_name: &str,
    state_dir: &Path,
    options: ShareOptions<'_>,
) -> Result<ProposeOutcome, RemoteError> {
    let ShareOptions {
        title, description, ..
    } = options;
    // Freshness first: every proposal must be mergeable at creation.
    pull(provider, spec, domain_root, state_dir).await?;
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;
    if !state.conflicts.is_empty() {
        return Err(RemoteError::ConflictsPending {
            count: state.conflicts.len(),
        });
    }

    // Ask the forge once, before any share work: whether this share can stack
    // is a property of the origin, and the answer is cached from here on.
    let stacked_path = stacks_available(
        provider,
        spec,
        &mut state,
        state_dir,
        options.stacks_allowed,
    )
    .await?;

    // 1b. A share naming a proposal is the amend verb: it lands on that
    //     layer's own branch instead of opening a new one. On a stackable
    //     chain that is [`amend_layer`], cascade and all; off it, the number
    //     can only name the one living proposal, and the amend IS the
    //     ordinary update below - merge commit and all - so it falls through.
    if let Some(number) = options.proposal {
        let index = amend_target_index(&state, number, stacked_path)?;
        if stacked_path && chain_is_stacked(&state) {
            return amend_layer(
                provider,
                spec,
                domain_root,
                domain_name,
                state_dir,
                state,
                index,
                title,
                description,
            )
            .await;
        }
    }

    // 2. What this share is measured against, and the layer it would sit on,
    //    are one question on a stackable chain: the base is the chain tip
    //    (see [`effective_tip_files`]) rather than the trunk, and which
    //    layers make up that tip depends on which of them are still really
    //    open - a gone branch ref settles its layer and the tip shrinks with
    //    it, so the walk down and the detection run together.
    if stacked_path && chain_is_stacked(&state) {
        // A repair a previous operation left half-done - and a chain that came
        // apart without one, which is what a settled gone-top ref leaves - is
        // finished before anything is stacked onto it: growing a chain whose
        // stack still holds a closed member wedges every later link call.
        finish_pending_repair(provider, spec, &mut state, state_dir).await?;
        // Any link still owed to the forge is settled next: growing the
        // chain over an unsettled gap would only widen it.
        retry_stack_link(provider, spec, &mut state, state_dir).await;
    }
    let (local, stack_top) = loop {
        let top = if stacked_path && chain_is_stacked(&state) {
            state
                .proposals
                .iter()
                .rev()
                .find(|p| p.status == ProposalStatus::Open)
                .cloned()
        } else {
            None
        };
        // Off the stacked path, and for a chain that emptied out, a share is
        // measured against the trunk exactly as it always was.
        let base = match top {
            Some(_) => effective_tip_files(&state),
            None => state.files.clone(),
        };
        let local = detect_local_changes(domain_root, &base)?;
        if local.changes.is_empty() {
            return Ok(ProposeOutcome::NothingToShare {
                skipped_large: local.skipped_large,
            });
        }
        let Some(top) = top else {
            break (local, None);
        };
        match provider.branch_ref(spec, &top.branch).await? {
            Some(live_head) => {
                if !head_is_ours(&top, &live_head) {
                    // A reviewer moved the layer this one would sit on;
                    // refuse before any write, naming the top.
                    return Ok(ProposeOutcome::ProposalDiverged {
                        number: top.number,
                        url: top.url.clone(),
                        branch: top.branch.clone(),
                    });
                }
                break (local, Some((top, live_head)));
            }
            None => {
                // The ref is gone: settle this layer exactly as the fallback
                // path settles a gone ref, then go round again - the layer
                // below becomes the new top and leaves the tip as it goes.
                let refreshed = provider.proposal_state(spec, top.number).await?;
                state.proposals.retain(|p| p.number != top.number);
                let mut settled = top.clone();
                settled.status = match refreshed {
                    ProposalState::Merged => ProposalStatus::Merged,
                    ProposalState::Declined | ProposalState::Open => ProposalStatus::Declined,
                };
                settled.pending_head_commit = None;
                state.push_history(settled);
                state.save(state_dir)?;
            }
        }
    };
    if stacked_path && stack_top.is_none() {
        // Nothing is left to stack onto, so the create path below opens a
        // fresh bottom layer and the old chain's stack number goes with the
        // chain it named.
        state.stack_number = None;
    }

    // 3. A declined proposal is superseded by this share: record to history
    //    (keeping Declined), branch best-effort deleted, exactly like the
    //    merged path's cleanup.
    let declined: Vec<Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Declined)
        .cloned()
        .collect();
    if !declined.is_empty() {
        for prop in &declined {
            state.proposals.retain(|p| p.number != prop.number);
            state.push_history(prop.clone());
        }
        state.save(state_dir)?;
        for prop in &declined {
            let _ = provider.delete_branch(spec, &prop.branch).await;
        }
    }

    // 4a. On a stackable chain a share never rewrites what is already open:
    //     it opens a new layer on top of the one resolved above, so each
    //     share stays reviewable on its own. A chain this machine does not
    //     know as a stack (multi-open residue from before stacking) never
    //     reaches here and falls through to 4b untouched.
    if let Some((top, live_head)) = stack_top {
        return stack_new_layer(
            provider,
            spec,
            domain_root,
            domain_name,
            state_dir,
            state,
            top,
            live_head,
            local,
            title,
            description,
        )
        .await;
    }

    // 4b. An open proposal is updated in place, never paralleled.
    let open = state
        .proposals
        .iter()
        .find(|p| p.status == ProposalStatus::Open)
        .cloned();
    if let Some(prop) = open {
        match provider.branch_ref(spec, &prop.branch).await? {
            Some(live_head) => {
                if !head_is_ours(&prop, &live_head) {
                    // A reviewer pushed commits; refuse before any write.
                    return Ok(ProposeOutcome::ProposalDiverged {
                        number: prop.number,
                        url: prop.url.clone(),
                        branch: prop.branch.clone(),
                    });
                }
                // The live head is one of ours (see `head_is_ours`), so this
                // is an ordinary update. Adopting an interrupted push needs no
                // separate write: `update_open_proposal` rewrites both head
                // fields on the record before it returns.
                return update_open_proposal(
                    provider,
                    spec,
                    domain_root,
                    domain_name,
                    state_dir,
                    state,
                    prop,
                    live_head,
                    local,
                    title,
                    description,
                )
                .await;
            }
            None => {
                // The ref is gone. Refresh this one proposal's state once:
                // merged or declined take their normal paths; an open PR with
                // no branch is treated as declined. Either way fall through
                // to create.
                let refreshed = provider.proposal_state(spec, prop.number).await?;
                state.proposals.retain(|p| p.number != prop.number);
                let mut settled = prop.clone();
                settled.status = match refreshed {
                    ProposalState::Merged => ProposalStatus::Merged,
                    ProposalState::Declined | ProposalState::Open => ProposalStatus::Declined,
                };
                // A settled record can never be updated again, so any push it
                // was still announcing is over: keep no pending sha in history.
                settled.pending_head_commit = None;
                state.push_history(settled);
                state.save(state_dir)?;
            }
        }
    }

    // 5. Create, as today, plus head_commit/base_commit on the fresh record.
    let collected =
        collect_changes(provider, spec, domain_root, state_dir, &local, description).await?;

    let generated_title = generate_title(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
        domain_name,
    );
    let effective_title = title.map(str::to_string).unwrap_or(generated_title);
    let summary = generate_summary_line(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
    );
    let body = description
        .map(str::to_string)
        .unwrap_or_else(|| generate_body(&summary, &collected.entries, domain_name));

    let tree_sha = provider
        .create_tree(spec, &state.base_commit, &collected.writes)
        .await?;
    let commit_sha = provider
        .create_commit(
            spec,
            &effective_title,
            &tree_sha,
            std::slice::from_ref(&state.base_commit),
        )
        .await?;
    let branch = share_branch_name(domain_name);
    provider.create_branch(spec, &branch, &commit_sha).await?;
    let handle = provider
        .create_proposal(
            spec,
            &ProposalRequest {
                title: effective_title.clone(),
                body,
                branch: branch.clone(),
                base_branch: spec.branch.clone(),
            },
        )
        .await?;

    state.proposals.push(Proposal {
        number: handle.number,
        url: handle.url.clone(),
        branch: branch.clone(),
        title: effective_title,
        created_at: Utc::now(),
        status: ProposalStatus::Open,
        files: collected.files,
        head_commit: Some(commit_sha),
        // A fresh record carries no half-finished push: the create path either
        // opened the proposal on this commit or failed before recording it.
        pending_head_commit: None,
        base_commit: Some(state.base_commit.clone()),
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
    });
    state.save(state_dir)?;

    Ok(ProposeOutcome::Proposed(ProposeReport {
        url: handle.url,
        number: handle.number,
        branch,
        added: collected.added,
        updated: collected.updated,
        deleted: collected.deleted,
        skipped_large: local.skipped_large,
        summary,
        // A bottom layer is not stacked onto anything: the chain starts here.
        stack_number: None,
        stack_position: None,
    }))
}

/// What a share would do, computed without a single provider write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharePlan {
    /// What a share run right now would do with the detected changes.
    pub action: PlannedAction,
    /// The local changes a share would carry, exactly as [`propose`] would
    /// detect them against the freshly pulled base.
    pub changes: crate::changes::LocalChanges,
    /// The caller's title, or the generated one for this change mix. Empty
    /// when there is nothing to title (nothing to share, conflicts pending).
    pub effective_title: String,
}

/// The single thing a share would do, as [`propose_preview`] classifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// A share would open a new proposal.
    Create,
    /// A share would stack a new layer on top of this open proposal, leaving
    /// it and every layer below it exactly as they stand.
    StackOnTop {
        /// The top open layer's number, the one the new layer would sit on.
        top_number: u64,
        /// The top open layer's title, so a caller can name what is being
        /// built on without a second lookup.
        top_title: String,
    },
    /// A share would amend this open layer in place, leaving the layers below
    /// it alone and re-basing every layer above it onto the amended head.
    Amend {
        /// The amended layer's number.
        number: u64,
        /// The web URL a human reviews the layer at.
        url: String,
        /// How many open layers sit above it, each one replayed by the
        /// cascade the amend runs.
        layers_above: usize,
    },
    /// A share would update this open proposal in place.
    Update {
        /// The open proposal's number.
        number: u64,
        /// The web URL a human reviews the proposal at.
        url: String,
    },
    /// Nothing to share.
    NothingToShare,
    /// Conflicts block a share until resolved.
    ConflictsPending {
        /// How many conflicts are still open.
        count: usize,
    },
    /// The open proposal's branch was amended by a reviewer.
    ProposalDiverged {
        /// The open proposal's number.
        number: u64,
        /// The web URL a human reviews the proposal at.
        url: String,
        /// The branch a reviewer moved out from under us.
        branch: String,
    },
}

/// The read-only twin of [`propose`]: runs the same pull, conflicts guard,
/// change detection and open/declined/divergence classification, but performs
/// no provider write and moves no record. It DOES perform the pull's writes
/// to the working tree - freshness is part of previewing honestly, and a plan
/// computed against a stale base would name changes a real share would never
/// make.
///
/// On a stackable chain it walks the open layers down exactly as [`propose`]
/// does, skipping each layer whose branch ref is gone (a share settles those
/// and stacks onto the layer below) and dropping it out of the chain tip it
/// then detects against, so both the named action and the change list match
/// the share the plan describes.
///
/// `domain_name` plays the same role it plays in [`propose`]: it is what the
/// generated title calls the domain when the caller supplies none. Of
/// `options` a preview reads the title alone: a description is only ever
/// written by a real share, and previewing writes nothing.
pub async fn propose_preview(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    domain_name: &str,
    state_dir: &Path,
    options: ShareOptions<'_>,
) -> Result<SharePlan, RemoteError> {
    let title = options.title;
    pull(provider, spec, domain_root, state_dir).await?;
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;
    // The same capability question a real share asks, and the same cached
    // answer: a preview that guessed would name an action the share then
    // would not take.
    let stacked_path = stacks_available(
        provider,
        spec,
        &mut state,
        state_dir,
        options.stacks_allowed,
    )
    .await?;

    // A share naming a layer is the amend verb, previewed the same way it is
    // validated: the number must be an open layer (and, off the stacked path,
    // the one living proposal), and what it would carry is the work standing
    // against the chain tip - the layer's own recorded files are already
    // proposed and are not this share's doing.
    if let Some(number) = options.proposal {
        let index = amend_target_index(&state, number, stacked_path)?;
        let stacked = stacked_path && chain_is_stacked(&state);
        let base = if stacked {
            effective_tip_files(&state)
        } else {
            state.files.clone()
        };
        let local = detect_local_changes(domain_root, &base)?;
        if !state.conflicts.is_empty() {
            return Ok(SharePlan {
                action: PlannedAction::ConflictsPending {
                    count: state.conflicts.len(),
                },
                changes: local,
                effective_title: String::new(),
            });
        }
        // The same probe the amend makes, in the same order, so a preview
        // never promises an amend the share then refuses.
        let layer = &state.proposals[index];
        let diverged = match provider.branch_ref(spec, &layer.branch).await? {
            Some(live_head) if head_is_ours(layer, &live_head) => None,
            Some(_) => Some(PlannedAction::ProposalDiverged {
                number: layer.number,
                url: layer.url.clone(),
                branch: layer.branch.clone(),
            }),
            None => return Err(branch_gone(layer)),
        };
        if diverged.is_none() && local.changes.is_empty() {
            return Ok(SharePlan {
                action: PlannedAction::NothingToShare,
                changes: local,
                effective_title: String::new(),
            });
        }
        let (added, updated, deleted) = count_changes(&local);
        let effective_title = title
            .map(str::to_string)
            .unwrap_or_else(|| generate_title(added, updated, deleted, domain_name));
        if let Some(action) = diverged {
            return Ok(SharePlan {
                action,
                changes: local,
                effective_title,
            });
        }
        let layers_above = state.proposals[index + 1..]
            .iter()
            .filter(|p| p.status == ProposalStatus::Open)
            .count();
        return Ok(SharePlan {
            action: PlannedAction::Amend {
                number: layer.number,
                url: layer.url.clone(),
                layers_above,
            },
            changes: local,
            effective_title,
        });
    }

    // The stacked walk, read-only: `propose` settles a layer whose branch ref
    // is gone and stacks onto the one below, so a preview walks down the same
    // way and drops each vanished layer out of the tip it detects against. A
    // share naming a layer to amend is a different action, and not this one.
    let stacked = stacked_path && chain_is_stacked(&state) && options.proposal.is_none();
    let mut surviving: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .collect();
    let mut stacked_action = None;
    if stacked {
        while let Some(top) = surviving.last().copied() {
            match provider.branch_ref(spec, &top.branch).await? {
                Some(live_head) if head_is_ours(top, &live_head) => {
                    stacked_action = Some(PlannedAction::StackOnTop {
                        top_number: top.number,
                        top_title: top.title.clone(),
                    });
                    break;
                }
                Some(_) => {
                    stacked_action = Some(PlannedAction::ProposalDiverged {
                        number: top.number,
                        url: top.url.clone(),
                        branch: top.branch.clone(),
                    });
                    break;
                }
                // Gone: a real share would settle this record, so its files
                // leave the tip and the layer below becomes the top.
                None => {
                    surviving.pop();
                }
            }
        }
    }

    // Detection runs against the chain tip whenever the walk above found a
    // chain, and against the trunk otherwise - the same base the share the
    // plan describes would use. A diverged top counts as a chain like any
    // other: that share writes nothing, but the changes it reports are still
    // the work standing on the chain, never the whole chain re-read against
    // the trunk.
    let base = if stacked_action.is_some() {
        tip_files_over(&state.files, &surviving)
    } else {
        state.files.clone()
    };
    let local = detect_local_changes(domain_root, &base)?;

    if !state.conflicts.is_empty() {
        return Ok(SharePlan {
            action: PlannedAction::ConflictsPending {
                count: state.conflicts.len(),
            },
            changes: local,
            effective_title: String::new(),
        });
    }
    if local.changes.is_empty() {
        return Ok(SharePlan {
            action: PlannedAction::NothingToShare,
            changes: local,
            effective_title: String::new(),
        });
    }

    let (added, updated, deleted) = count_changes(&local);
    let effective_title = title
        .map(str::to_string)
        .unwrap_or_else(|| generate_title(added, updated, deleted, domain_name));

    if let Some(action) = stacked_action {
        return Ok(SharePlan {
            action,
            changes: local,
            effective_title,
        });
    }

    // The fallback classification: a declined record would be superseded (so
    // it reads as Create), an open one is an Update unless a reviewer amended
    // its branch, and a gone ref would be re-created.
    let open = state
        .proposals
        .iter()
        .find(|p| p.status == ProposalStatus::Open);
    let action = match open {
        None => PlannedAction::Create,
        Some(prop) => match provider.branch_ref(spec, &prop.branch).await? {
            Some(live_head) => {
                // The same acceptance `propose` applies, including the
                // interrupted-update case: a preview that called our own
                // half-finished push a divergence would send the caller to
                // withdraw a proposal the next share heals by itself.
                if head_is_ours(prop, &live_head) {
                    PlannedAction::Update {
                        number: prop.number,
                        url: prop.url.clone(),
                    }
                } else {
                    PlannedAction::ProposalDiverged {
                        number: prop.number,
                        url: prop.url.clone(),
                        branch: prop.branch.clone(),
                    }
                }
            }
            None => PlannedAction::Create,
        },
    };
    Ok(SharePlan {
        action,
        changes: local,
        effective_title,
    })
}

/// The (added, updated, deleted) counts of a detected change set.
fn count_changes(local: &crate::changes::LocalChanges) -> (usize, usize, usize) {
    let mut counts = (0usize, 0usize, 0usize);
    for change in &local.changes {
        match change {
            LocalChange::Added { .. } => counts.0 += 1,
            LocalChange::Modified { .. } => counts.1 += 1,
            LocalChange::Deleted { .. } => counts.2 += 1,
        }
    }
    counts
}

/// Everything a share commit is built from, collected in one pass over the
/// detected local changes: the tree writes (blobs already uploaded), the
/// proposal file records, the per-kind path lists and the body entries.
struct CollectedChanges {
    writes: Vec<TreeWrite>,
    files: Vec<ProposedFile>,
    added: Vec<String>,
    updated: Vec<String>,
    deleted: Vec<String>,
    entries: ChangeEntries,
}

/// Uploads a blob for every added or modified local file and collects
/// everything both share paths - opening a new proposal and updating the open
/// one - build their commit and their proposal body from.
async fn collect_changes(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    local: &crate::changes::LocalChanges,
    description: Option<&str>,
) -> Result<CollectedChanges, RemoteError> {
    let mut out = CollectedChanges {
        writes: Vec::new(),
        files: Vec::new(),
        added: Vec::new(),
        updated: Vec::new(),
        deleted: Vec::new(),
        entries: ChangeEntries::default(),
    };

    for change in &local.changes {
        match change {
            LocalChange::Added { path, sha256 } => {
                let wt_path = checked_working_path(state_dir, domain_root, path)?;
                let bytes = std::fs::read(&wt_path)?;
                let size = bytes.len() as u64;
                let blob_sha = provider.create_blob(spec, &bytes).await?;
                out.writes.push(TreeWrite {
                    path: to_repo_relative(path, spec.subpath.as_deref()),
                    blob_sha: Some(blob_sha.clone()),
                });
                out.entries.added.push((path.clone(), bytes));
                out.added.push(path.clone());
                out.files.push(ProposedFile {
                    path: path.clone(),
                    change: ProposedChange::Added,
                    sha256: Some(sha256.clone()),
                    blob_sha: Some(blob_sha),
                    size: Some(size),
                });
            }
            LocalChange::Modified { path, sha256 } => {
                let wt_path = checked_working_path(state_dir, domain_root, path)?;
                let bytes = std::fs::read(&wt_path)?;
                let size = bytes.len() as u64;
                let blob_sha = provider.create_blob(spec, &bytes).await?;
                out.writes.push(TreeWrite {
                    path: to_repo_relative(path, spec.subpath.as_deref()),
                    blob_sha: Some(blob_sha.clone()),
                });
                out.entries.updated.push((path.clone(), bytes));
                out.updated.push(path.clone());
                out.files.push(ProposedFile {
                    path: path.clone(),
                    change: ProposedChange::Modified,
                    sha256: Some(sha256.clone()),
                    blob_sha: Some(blob_sha),
                    size: Some(size),
                });
            }
            LocalChange::Deleted { path } => {
                out.writes.push(TreeWrite {
                    path: to_repo_relative(path, spec.subpath.as_deref()),
                    blob_sha: None,
                });
                // Only worth reading back the retired file's last known
                // content (for its engram title in the generated body) when
                // there is a generated body to put it in at all.
                if description.is_none() {
                    let base_content = state::read_base_file(state_dir, path)?;
                    out.entries.deleted.push((path.clone(), base_content));
                }
                out.deleted.push(path.clone());
                out.files.push(ProposedFile {
                    path: path.clone(),
                    change: ProposedChange::Deleted,
                    sha256: None,
                    blob_sha: None,
                    size: None,
                });
            }
        }
    }

    Ok(out)
}

/// Whether `live_head` is a commit this machine put on the proposal branch,
/// which is what separates an ordinary update from a reviewer's amendment.
///
/// Three ways it is ours, and every other head is a divergence:
///
/// 1. it equals the recorded [`Proposal::head_commit`] - the settled case, the
///    branch is exactly where the last completed share left it;
/// 2. it equals [`Proposal::pending_head_commit`] - our own interrupted
///    update: the branch move landed and the step after it did not, so the
///    record never caught up. Adopting it is the only outcome that can heal,
///    since retrying is what a caller does and a retry that still refused
///    would refuse forever;
/// 3. no head was ever recorded (`head_commit` is `None`) - a record written
///    before the field existed, adopted silently because there is nothing to
///    compare against.
fn head_is_ours(prop: &Proposal, live_head: &str) -> bool {
    match &prop.head_commit {
        None => true,
        Some(recorded) => {
            recorded == live_head || prop.pending_head_commit.as_deref() == Some(live_head)
        }
    }
}

/// Updates the one open proposal in place: a new commit on its branch (a
/// merge commit when the base advanced), the ref fast-forwarded, the PR body
/// regenerated (title PATCHed only when the caller supplied one) and the
/// record rewritten. The tree is built on the CURRENT base commit's tree -
/// the fresh pull inside [`propose`] guarantees local content already carries
/// everything merged upstream - so the new tree is complete on its own.
#[allow(clippy::too_many_arguments)]
async fn update_open_proposal(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    domain_name: &str,
    state_dir: &Path,
    mut state: OriginState,
    prop: Proposal,
    live_head: String,
    local: crate::changes::LocalChanges,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<ProposeOutcome, RemoteError> {
    let collected =
        collect_changes(provider, spec, domain_root, state_dir, &local, description).await?;

    let generated_title = generate_title(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
        domain_name,
    );
    let effective_title = title.map(str::to_string).unwrap_or(generated_title);
    let summary = generate_summary_line(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
    );
    let body = description
        .map(str::to_string)
        .unwrap_or_else(|| generate_body(&summary, &collected.entries, domain_name));

    // Parents: plain child of the branch head while the proposal's recorded
    // base still equals the current one; a two-parent merge commit (head
    // first, then the advanced base) once the base moved, so the PR's merge
    // base moves forward and the diff never shows upstream changes as ours.
    let parents: Vec<String> = if prop.base_commit.as_deref() == Some(state.base_commit.as_str()) {
        vec![live_head.clone()]
    } else {
        vec![live_head.clone(), state.base_commit.clone()]
    };

    let tree_sha = provider
        .create_tree(spec, &state.base_commit, &collected.writes)
        .await?;
    let commit_sha = provider
        .create_commit(spec, &effective_title, &tree_sha, &parents)
        .await?;
    // Announce the push before making it. Everything from here to the final
    // save is one logical step that the network, or a dying process, can cut
    // in half; the only half that leaves a mark upstream is a landed
    // `update_branch`, and a landed branch move whose `head_commit` never
    // caught up reads exactly like a reviewer's amendment. Recording the sha
    // first turns that into a recognizable "our own interrupted update"
    // (see `Proposal::pending_head_commit` and `head_is_ours`), which the next
    // share adopts. Saving twice costs one small file write per update.
    {
        let record = state
            .proposals
            .iter_mut()
            .find(|p| p.number == prop.number)
            .expect("the open proposal was just read out of this state");
        record.pending_head_commit = Some(commit_sha.clone());
    }
    state.save(state_dir)?;

    provider
        .update_branch(spec, &prop.branch, &commit_sha, false)
        .await?;
    provider
        .update_proposal(spec, prop.number, title, Some(&body), None)
        .await?;

    let record = state
        .proposals
        .iter_mut()
        .find(|p| p.number == prop.number)
        .expect("the open proposal was just read out of this state");
    record.files = collected.files;
    record.head_commit = Some(commit_sha);
    // The push is finished and recorded: nothing is pending any more.
    record.pending_head_commit = None;
    record.base_commit = Some(state.base_commit.clone());
    record.updated_at = Some(Utc::now());
    if let Some(t) = title {
        record.title = t.to_string();
    }
    state.save(state_dir)?;

    Ok(ProposeOutcome::Updated(ProposeReport {
        url: prop.url,
        number: prop.number,
        branch: prop.branch,
        added: collected.added,
        updated: collected.updated,
        deleted: collected.deleted,
        skipped_large: local.skipped_large,
        summary,
        // The living-proposal path is the fallback: no chain, no position.
        stack_number: None,
        stack_position: None,
    }))
}

/// Stacks a new layer on top of the open chain: a commit whose single parent
/// is the top layer's head, a branch of its own and a pull request targeting
/// the top layer's branch, then linked into the forge's stack.
///
/// The tree is built on `top_head` rather than on the trunk, so the layer
/// carries everything the layers below it carry and the forge's diff shows
/// only what this share adds. The commit has exactly one parent for the same
/// reason: a merge commit would drag the trunk into a layer that is meant to
/// read as one reviewable step.
///
/// Linkage comes last and is allowed to fail. The pull request already
/// exists by then, so a failed `create_stack`/`extend_stack` leaves the chain
/// degraded (every layer open, none of them grouped) rather than losing the
/// share: [`crate::state::OriginState::stack_link_pending`] records the debt
/// and [`retry_stack_link`] pays it off at the start of the next operation.
#[allow(clippy::too_many_arguments)]
async fn stack_new_layer(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    domain_name: &str,
    state_dir: &Path,
    mut state: OriginState,
    top: Proposal,
    top_head: String,
    local: crate::changes::LocalChanges,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<ProposeOutcome, RemoteError> {
    let collected =
        collect_changes(provider, spec, domain_root, state_dir, &local, description).await?;

    let generated_title = generate_title(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
        domain_name,
    );
    let effective_title = title.map(str::to_string).unwrap_or(generated_title);
    let summary = generate_summary_line(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
    );
    let body = description
        .map(str::to_string)
        .unwrap_or_else(|| generate_body(&summary, &collected.entries, domain_name));

    let tree_sha = provider
        .create_tree(spec, &top_head, &collected.writes)
        .await?;
    let commit_sha = provider
        .create_commit(
            spec,
            &effective_title,
            &tree_sha,
            std::slice::from_ref(&top_head),
        )
        .await?;
    let branch = share_branch_name(domain_name);
    provider.create_branch(spec, &branch, &commit_sha).await?;
    let handle = provider
        .create_proposal(
            spec,
            &ProposalRequest {
                title: effective_title.clone(),
                body,
                branch: branch.clone(),
                base_branch: top.branch.clone(),
            },
        )
        .await?;

    // With no stack number yet the chain is exactly the layer below plus this
    // one: `chain_is_stacked` only lets an unlinked chain here while a single
    // layer is open, so those two are the whole stack.
    let linked = match state.stack_number {
        None => provider
            .create_stack(spec, &[top.number, handle.number])
            .await
            .map(|info| Some(info.number)),
        Some(number) => provider
            .extend_stack(spec, number, &[handle.number])
            .await
            .map(|_| Some(number)),
    };
    match linked {
        Ok(number) => {
            state.stack_number = number;
            state.stack_link_pending = false;
        }
        Err(e) => {
            tracing::debug!(
                "linking proposal #{} into the stack failed; the chain is degraded until the next share: {e}",
                handle.number
            );
            state.stack_link_pending = true;
        }
    }

    state.proposals.push(Proposal {
        number: handle.number,
        url: handle.url.clone(),
        branch: branch.clone(),
        title: effective_title,
        created_at: Utc::now(),
        status: ProposalStatus::Open,
        files: collected.files,
        head_commit: Some(commit_sha),
        pending_head_commit: None,
        // A layer's base is the layer below it, never the trunk.
        base_commit: Some(top_head),
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
    });
    state.save(state_dir)?;

    Ok(ProposeOutcome::Proposed(ProposeReport {
        url: handle.url,
        number: handle.number,
        branch,
        added: collected.added,
        updated: collected.updated,
        deleted: collected.deleted,
        skipped_large: local.skipped_large,
        summary,
        stack_number: state.stack_number,
        stack_position: Some(open_position(&state, handle.number)),
    }))
}

/// Where the proposal numbered `number` sits among the open layers, as
/// `(1-based position, open layers)`. A number that is not open reads as the
/// top, which is what a freshly pushed layer is anyway.
fn open_position(state: &OriginState, number: u64) -> (usize, usize) {
    let open: Vec<u64> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .map(|p| p.number)
        .collect();
    let position = open
        .iter()
        .position(|n| *n == number)
        .map(|i| i + 1)
        .unwrap_or(open.len());
    (position, open.len())
}

/// The index into `state.proposals` of the layer a share named to amend, or
/// the teaching refusal listing what is actually open.
///
/// Two rules, one message. The number must name an OPEN record, and off the
/// stacked path it must name the only one: the fallback flow has a single
/// living proposal by construction, and a chain of records left over from
/// before stacking is exactly the state where naming one of them would mean
/// something this machine cannot deliver.
fn amend_target_index(
    state: &OriginState,
    number: u64,
    stacked_path: bool,
) -> Result<usize, RemoteError> {
    let open: Vec<(usize, &Proposal)> = state
        .proposals
        .iter()
        .enumerate()
        .filter(|(_, p)| p.status == ProposalStatus::Open)
        .collect();
    let stacked = stacked_path && chain_is_stacked(state);
    match open.iter().find(|(_, p)| p.number == number) {
        Some((index, _)) if stacked || open.len() == 1 => Ok(*index),
        _ => Err(not_an_open_layer(state, number)),
    }
}

/// The refusal a share naming an unamendable proposal gets: what it named and
/// what is open instead, each layer with its position in the chain, so the
/// caller can retry against a real number without a second call.
fn not_an_open_layer(state: &OriginState, number: u64) -> RemoteError {
    let open: Vec<String> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .enumerate()
        .map(|(position, p)| format!("#{} (layer {})", p.number, position + 1))
        .collect();
    if open.is_empty() {
        return RemoteError::State(format!(
            "proposal #{number} is not an open layer of this domain; this domain has no open layers"
        ));
    }
    RemoteError::State(format!(
        "proposal #{number} is not an open layer of this domain; open layers: {}",
        open.join(", ")
    ))
}

/// The refusal a share naming a layer whose branch is gone upstream earns:
/// there is no branch left to amend, and re-creating one behind the same
/// pull request would be a different operation than the caller asked for.
fn branch_gone(layer: &Proposal) -> RemoteError {
    RemoteError::State(format!(
        "proposal #{}'s branch {} is gone upstream; withdraw the proposal and share again",
        layer.number, layer.branch
    ))
}

/// The refusal a layer that cannot be replayed earns, named rather than
/// worked around: rebuilding a layer's tree needs the blob shas its record
/// carries, and a record written before those existed (pre-0.17.0) has none.
fn unreplayable_layer(number: u64) -> RemoteError {
    RemoteError::State(format!(
        "layer #{number} predates stacked shares and cannot be replayed; withdraw it or merge it first"
    ))
}

/// Checks every open layer from `from_index` up for the blob shas a replay
/// needs, so an amend or a repair refuses BEFORE its first write rather than
/// halfway up the chain. A deletion needs no blob and is never the reason.
fn ensure_replayable(state: &OriginState, from_index: usize) -> Result<(), RemoteError> {
    for layer in state
        .proposals
        .iter()
        .skip(from_index)
        .filter(|p| p.status == ProposalStatus::Open)
    {
        let missing = layer.files.iter().any(|file| {
            matches!(
                file.change,
                ProposedChange::Added | ProposedChange::Modified
            ) && file.blob_sha.is_none()
        });
        if missing {
            return Err(unreplayable_layer(layer.number));
        }
    }
    Ok(())
}

/// The chain tip BELOW the layer at `index`: the trunk snapshot with every
/// open layer beneath it laid over ([`tip_files_over`]), which is both what
/// that layer's own delta is measured against and the presence walk a replay
/// asks before writing a deletion - retiring a path nothing below carries is
/// not a change, it is noise in someone's review.
fn tip_below_layer(state: &OriginState, index: usize) -> BTreeMap<String, BaseStamp> {
    let below: Vec<&Proposal> = state.proposals[..index]
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .collect();
    tip_files_over(&state.files, &below)
}

/// Amends the open layer at `index`: a fresh commit on ITS branch carrying
/// its own delta again, then [`cascade_replays`] over every layer above it.
///
/// What the layer proposes afterwards is its recorded files with this share's
/// work merged in, recomputed against the tip below it: a path this share
/// touched takes the working tree's content, and a path it did not keeps
/// exactly what the record carries, blob sha and digest included. The working
/// tree cannot speak for the latter - it holds the chain TIP, so a file a
/// layer above changed reads there as that layer's content, not this one's -
/// and re-reading it would quietly move one layer's work into another's. A
/// fresh change is dropped from the layer when its content already matches
/// the tip below (there is nothing left to propose there) and a retirement is
/// dropped when nothing below carries the path; a KEPT entry is not re-tested
/// that way, since its own recorded content is what defines the layer.
///
/// One consequence to know before reaching for this verb: a fresh edit to a
/// path an open layer ABOVE owns lands in the amended layer's commit, and
/// then that layer's replay writes its own recorded content over it, so at
/// the chain tip the edit is gone and the file reads as an unshared local
/// change again on the next share. Nothing is lost on disk and no layer is
/// corrupted, but the edit is not where the caller meant it to be. Amending
/// the layer that OWNS the path is the way to change it.
///
/// Three properties the shape of this function exists for:
///
/// - the commit's parent is ALWAYS the layer's live head alone. A stacked
///   layer is one reviewable step, and a merge commit would drag the trunk
///   into it; an advanced trunk arrives through the tree instead, since the
///   tree is built on the tip below rather than on the layer's own history.
/// - divergence is checked on this layer's branch only. The layers above are
///   rewritten from their records either way, so their heads are this amend's
///   business rather than its precondition.
/// - not one stack call is made. No base ref moves, so the forge's membership
///   holds exactly as it stands.
#[allow(clippy::too_many_arguments)]
async fn amend_layer(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    domain_name: &str,
    state_dir: &Path,
    mut state: OriginState,
    index: usize,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<ProposeOutcome, RemoteError> {
    // Nothing is written until every layer this cascade replays is known to be
    // replayable: a chain re-based halfway is worse than one never touched.
    // The amended layer itself is not in that set - it is rebuilt from the
    // working tree rather than replayed from its record, so
    // [`collect_amend_changes`] decides per file what it can stand on.
    ensure_replayable(&state, index + 1)?;

    let layer = state.proposals[index].clone();
    let live_head = match provider.branch_ref(spec, &layer.branch).await? {
        Some(live_head) => {
            if !head_is_ours(&layer, &live_head) {
                // A reviewer moved the very layer this share would amend;
                // refuse before any write, exactly as a stacking share does.
                return Ok(ProposeOutcome::ProposalDiverged {
                    number: layer.number,
                    url: layer.url,
                    branch: layer.branch,
                });
            }
            live_head
        }
        // The branch is gone. A stacking share settles such a layer and
        // carries on, but this share named it: there is nothing to amend.
        None => return Err(branch_gone(&layer)),
    };

    // This share's own work is what stands against the chain tip; the layer's
    // recorded files are already proposed and are not it.
    let fresh = detect_local_changes(domain_root, &effective_tip_files(&state))?;
    if fresh.changes.is_empty() {
        return Ok(ProposeOutcome::NothingToShare {
            skipped_large: fresh.skipped_large,
        });
    }

    // Which paths the layers above claim decides whether the working tree may
    // be read for a file this layer's record can no longer name a blob for:
    // where a layer above owns the path, the tree holds that layer's content.
    let owned_above: BTreeSet<String> = state.proposals[index + 1..]
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .flat_map(|p| p.files.iter().map(|file| file.path.clone()))
        .collect();

    let below = tip_below_layer(&state, index);
    let collected = collect_amend_changes(
        provider,
        spec,
        domain_root,
        state_dir,
        &layer,
        &below,
        &owned_above,
        &fresh,
        description,
    )
    .await?;

    let generated_title = generate_title(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
        domain_name,
    );
    let effective_title = title.map(str::to_string).unwrap_or(generated_title);
    let summary = generate_summary_line(
        collected.added.len(),
        collected.updated.len(),
        collected.deleted.len(),
    );
    let body = description
        .map(str::to_string)
        .unwrap_or_else(|| generate_body(&summary, &collected.entries, domain_name));

    // The tree is built on the tip below this layer - the layer beneath it,
    // or the trunk for the bottom one - so the layer's diff stays its own
    // delta and an advanced trunk costs no merge commit.
    let below_head = state.proposals[..index]
        .iter()
        .rev()
        .find(|p| p.status == ProposalStatus::Open)
        .and_then(|p| p.head_commit.clone())
        .unwrap_or_else(|| state.base_commit.clone());

    let tree_sha = provider
        .create_tree(spec, &below_head, &collected.writes)
        .await?;
    let commit_sha = provider
        .create_commit(
            spec,
            &effective_title,
            &tree_sha,
            std::slice::from_ref(&live_head),
        )
        .await?;

    // The push is announced before it is made, the same interrupted-update
    // protocol an ordinary share update follows (see `pending_head_commit`
    // and `head_is_ours`), and the branch moves fast-forward: this layer's
    // own history is being extended, not rewritten.
    state.proposals[index].pending_head_commit = Some(commit_sha.clone());
    state.save(state_dir)?;

    provider
        .update_branch(spec, &layer.branch, &commit_sha, false)
        .await?;
    provider
        .update_proposal(spec, layer.number, title, Some(&body), None)
        .await?;

    {
        let record = &mut state.proposals[index];
        record.files = collected.files;
        record.head_commit = Some(commit_sha.clone());
        record.pending_head_commit = None;
        record.base_commit = Some(below_head);
        record.updated_at = Some(Utc::now());
        if let Some(t) = title {
            record.title = t.to_string();
        }
    }
    state.save(state_dir)?;

    // Every layer above now sits on a commit that is no longer this layer's
    // head, so each is replayed onto the amended one, bottom-up.
    cascade_replays(provider, spec, &mut state, state_dir, index + 1, commit_sha).await?;

    Ok(ProposeOutcome::Updated(ProposeReport {
        url: layer.url,
        number: layer.number,
        branch: layer.branch,
        added: collected.added,
        updated: collected.updated,
        deleted: collected.deleted,
        skipped_large: fresh.skipped_large,
        summary,
        stack_number: state.stack_number,
        stack_position: Some(open_position(&state, layer.number)),
    }))
}

/// Collects what an amended layer proposes: its recorded files with `fresh`
/// merged in, every entry recomputed against `below` (the tip beneath the
/// layer), and a blob uploaded for each path this share touched.
///
/// A recorded entry is kept verbatim - its blob sha is what rebuilds the
/// tree - unless this share speaks for the same path or the entry stopped
/// being a change against `below`. A fresh change whose content already
/// matches `below` takes the path OUT of the layer entirely: the layer has
/// nothing left to propose there.
///
/// A kept entry from before blob shas were recorded (pre-0.17.0) has nothing
/// to rebuild the tree from, and there is exactly one place to get it back:
/// the working tree. That is sound only where no OPEN layer above owns the
/// path, since above it the tree holds the upper layer's content rather than
/// this one's - `owned_above` is that set, and a path in it refuses the whole
/// amend with the teaching error instead. Where the re-read is allowed it is
/// still checked rather than trusted: a kept entry is by construction one
/// that did not change against the chain tip, so the bytes on disk must hash
/// to the digest the record carries, and content that does not is a refusal
/// rather than a blob upload of the wrong bytes.
#[allow(clippy::too_many_arguments)]
async fn collect_amend_changes(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    layer: &Proposal,
    below: &BTreeMap<String, BaseStamp>,
    owned_above: &BTreeSet<String>,
    fresh: &crate::changes::LocalChanges,
    description: Option<&str>,
) -> Result<CollectedChanges, RemoteError> {
    let sub = spec.subpath.as_deref();
    let fresh_paths: BTreeSet<&str> = fresh.changes.iter().map(LocalChange::path).collect();

    let mut merged: BTreeMap<String, ProposedFile> = BTreeMap::new();
    for file in &layer.files {
        if fresh_paths.contains(file.path.as_str()) {
            continue;
        }
        let still_a_change = match file.change {
            ProposedChange::Added | ProposedChange::Modified => true,
            ProposedChange::Deleted => below.contains_key(&file.path),
        };
        if !still_a_change {
            continue;
        }
        let mut kept = file.clone();
        let needs_blob = matches!(
            kept.change,
            ProposedChange::Added | ProposedChange::Modified
        ) && kept.blob_sha.is_none();
        if needs_blob {
            if owned_above.contains(&kept.path) {
                return Err(unreplayable_layer(layer.number));
            }
            let Some(recorded) = kept.sha256.clone() else {
                // No blob and no digest: nothing to rebuild the file from and
                // nothing to check a re-read against.
                return Err(unreplayable_layer(layer.number));
            };
            let wt_path = checked_working_path(state_dir, domain_root, &kept.path)?;
            let Some(bytes) = read_optional_file(&wt_path)? else {
                return Err(unreplayable_layer(layer.number));
            };
            if state::sha256_hex(&bytes) != recorded {
                return Err(RemoteError::State(format!(
                    "layer #{} records {} at content the working tree no longer holds; share that change or withdraw the layer",
                    layer.number, kept.path
                )));
            }
            kept.blob_sha = Some(provider.create_blob(spec, &bytes).await?);
            kept.size = Some(bytes.len() as u64);
        }
        merged.insert(kept.path.clone(), kept);
    }

    for change in &fresh.changes {
        match change {
            LocalChange::Added { path, sha256 } | LocalChange::Modified { path, sha256 } => {
                let beneath = below.get(path);
                if beneath.map(|stamp| stamp.sha256.as_str()) == Some(sha256.as_str()) {
                    // The tip below already carries exactly this content.
                    merged.remove(path);
                    continue;
                }
                let wt_path = checked_working_path(state_dir, domain_root, path)?;
                let bytes = std::fs::read(&wt_path)?;
                let blob_sha = provider.create_blob(spec, &bytes).await?;
                merged.insert(
                    path.clone(),
                    ProposedFile {
                        path: path.clone(),
                        change: match beneath {
                            Some(_) => ProposedChange::Modified,
                            None => ProposedChange::Added,
                        },
                        sha256: Some(sha256.clone()),
                        blob_sha: Some(blob_sha),
                        size: Some(bytes.len() as u64),
                    },
                );
            }
            LocalChange::Deleted { path } => {
                if !below.contains_key(path) {
                    // Nothing below carries it, so there is nothing here to
                    // retire: the layer simply stops proposing the path.
                    merged.remove(path);
                    continue;
                }
                merged.insert(
                    path.clone(),
                    ProposedFile {
                        path: path.clone(),
                        change: ProposedChange::Deleted,
                        sha256: None,
                        blob_sha: None,
                        size: None,
                    },
                );
            }
        }
    }

    let mut out = CollectedChanges {
        writes: Vec::new(),
        files: Vec::new(),
        added: Vec::new(),
        updated: Vec::new(),
        deleted: Vec::new(),
        entries: ChangeEntries::default(),
    };
    for (path, file) in merged {
        match file.change {
            ProposedChange::Added | ProposedChange::Modified => {
                let blob_sha = file
                    .blob_sha
                    .clone()
                    .ok_or_else(|| unreplayable_layer(layer.number))?;
                out.writes.push(TreeWrite {
                    path: to_repo_relative(&path, sub),
                    blob_sha: Some(blob_sha),
                });
                // Content is only worth reading back for the generated body's
                // engram titles, and a kept entry's file may not be on disk at
                // all (a layer above may have retired it since); an unreadable
                // one simply falls back to its bare path.
                if description.is_none() {
                    let wt_path = checked_working_path(state_dir, domain_root, &path)?;
                    let content = read_optional_file(&wt_path)?.unwrap_or_default();
                    match file.change {
                        ProposedChange::Added => out.entries.added.push((path.clone(), content)),
                        _ => out.entries.updated.push((path.clone(), content)),
                    }
                }
                match file.change {
                    ProposedChange::Added => out.added.push(path.clone()),
                    _ => out.updated.push(path.clone()),
                }
            }
            ProposedChange::Deleted => {
                out.writes.push(TreeWrite {
                    path: to_repo_relative(&path, sub),
                    blob_sha: None,
                });
                if description.is_none() {
                    let base_content = state::read_base_file(state_dir, &path)?;
                    out.entries.deleted.push((path.clone(), base_content));
                }
                out.deleted.push(path.clone());
            }
        }
        out.files.push(file);
    }

    Ok(out)
}

/// Replays every open layer from `from_index` up onto `parent_head`,
/// bottom-up, rebuilding each one's commit from its OWN recorded files and
/// forcing its branch onto the result.
///
/// This is what makes a mid-chain change self-healing: an amend below (and, in
/// the withdrawal repair, a hole cut out of the chain) leaves every layer
/// above sitting on a commit that is no longer the chain, and each one is put
/// back where it belongs without a human re-basing anything. A layer's content
/// comes from its record rather than from the working tree, which holds the
/// chain tip and could not tell one layer's work from another's.
///
/// Two rules the replay follows. A deletion is written only when the path is
/// actually present in the tip below that layer ([`tip_below_layer`]), so a
/// retirement of something no longer there is dropped rather than replayed as
/// a phantom change. And each layer's branch move follows the same
/// interrupted-update protocol a share update does - the sha recorded and
/// saved BEFORE the ref moves, promoted and cleared after - except the move is
/// forced: a replayed layer's history is rewritten, not extended.
async fn cascade_replays(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &mut OriginState,
    state_dir: &Path,
    from_index: usize,
    mut parent_head: String,
) -> Result<(), RemoteError> {
    let sub = spec.subpath.as_deref();
    for index in from_index..state.proposals.len() {
        if state.proposals[index].status != ProposalStatus::Open {
            continue;
        }
        let layer = state.proposals[index].clone();
        let below = tip_below_layer(state, index);

        let mut writes = Vec::new();
        for file in &layer.files {
            match file.change {
                ProposedChange::Added | ProposedChange::Modified => {
                    // Guarded before the first write by `ensure_replayable`;
                    // refusing here too costs nothing and beats a panic on a
                    // caller that skipped the check.
                    let blob_sha = file
                        .blob_sha
                        .clone()
                        .ok_or_else(|| unreplayable_layer(layer.number))?;
                    writes.push(TreeWrite {
                        path: to_repo_relative(&file.path, sub),
                        blob_sha: Some(blob_sha),
                    });
                }
                ProposedChange::Deleted => {
                    if below.contains_key(&file.path) {
                        writes.push(TreeWrite {
                            path: to_repo_relative(&file.path, sub),
                            blob_sha: None,
                        });
                    }
                }
            }
        }

        let tree_sha = provider.create_tree(spec, &parent_head, &writes).await?;
        let commit_sha = provider
            .create_commit(
                spec,
                &layer.title,
                &tree_sha,
                std::slice::from_ref(&parent_head),
            )
            .await?;

        state.proposals[index].pending_head_commit = Some(commit_sha.clone());
        state.save(state_dir)?;

        provider
            .update_branch(spec, &layer.branch, &commit_sha, true)
            .await?;

        {
            let record = &mut state.proposals[index];
            record.head_commit = Some(commit_sha.clone());
            record.pending_head_commit = None;
            record.base_commit = Some(parent_head);
            record.updated_at = Some(Utc::now());
        }
        state.save(state_dir)?;

        parent_head = commit_sha;
    }
    Ok(())
}

/// What a chain repair did, carried back to the caller's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct RepairOutcome {
    /// Whether the repair touched the forge at all (a dissolve, a replay or a
    /// recreate).
    repaired: bool,
    /// The NEW stack number a recreate allocated, when one ran.
    restacked: Option<u64>,
}

/// Whether the recorded chain and the stack this machine believes in have
/// come apart, without asking the forge anything.
///
/// Three shapes say so, and all of them are the residue of a layer leaving
/// the chain outside a withdrawal - a settled gone branch ref, or a reviewer
/// closing one layer of several. A stack whose members are no longer a chain
/// wedges every later `extend_stack` with the base-ref 422 and cannot be
/// unwedged without dissolving first, so finding these cheaply matters:
///
/// - a stack number with fewer than two open layers under it, which is a
///   number naming something that is not a chain any more;
/// - an open layer whose recorded base is not the head of the open layer below
///   it, which is a hole in between the two;
/// - a hole below the bottom open layer, which shows as that layer not yet
///   standing on the trunk it now has to target.
///
/// Every test is a comparison of recorded fields, so this costs no call, and
/// each one reads false again once [`repair_chain`] has done its work - a
/// repaired chain must not re-repair itself on every later operation.
///
/// A chain carrying a merged layer with open work above it answers false
/// whatever its shape: such a chain is not repairable at all until the merge
/// is pulled in ([`merged_layer_blocking_repair`]), so the honest answer is
/// "not for this function to fix" rather than "fine".
fn stack_shape_broken(state: &OriginState) -> bool {
    if state.stack_number.is_none() {
        return false;
    }
    if merged_layer_blocking_repair(state).is_some() {
        return false;
    }
    let open: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .collect();
    let Some(bottom) = open.first() else {
        return true;
    };
    if open.len() < 2 {
        return true;
    }
    // A hole under the bottom survivor: only a repair puts that layer onto the
    // trunk, so a recorded base still naming the vanished layer's head says
    // the repair has not run. The trunk moving on under an intact chain is not
    // this shape - there is no hole beneath the bottom layer then.
    let hole_below = state
        .proposals
        .iter()
        .take_while(|p| p.number != bottom.number)
        .any(|p| p.status != ProposalStatus::Open);
    if hole_below && bottom.base_commit.as_deref() != Some(state.base_commit.as_str()) {
        return true;
    }
    open.windows(2)
        .any(|pair| pair[1].base_commit != pair[0].head_commit)
}

/// The merged layer that makes this chain unrepairable, if it carries one.
///
/// A repair rebuilds every layer above a hole from the layers below it, and a
/// merged layer's content is in neither place yet: it lives on the trunk, and
/// only the next [`pull`] advances `base_commit` and the base snapshot onto
/// it. Replaying over that would rebuild the survivors WITHOUT the merged
/// files, which on the forge reads as those files being deleted - a corrupt
/// review rather than a repaired chain. The way out is the pull that consumes
/// it, which both arms of [`pull`] now perform.
///
/// What blocks is exactly the merged record with an OPEN layer still above it
/// ([`under_an_open_layer`]), which is the whole lossy class and nothing more.
/// Such a record is either below where the replay starts - the survivors are
/// then rebuilt on a parent that predates the merge - or inside the range
/// [`cascade_replays`] walks, where it is skipped as not-open and the layer
/// above it is rebuilt without it.
///
/// A merged record with NO open layer above it is neither, and the invariant
/// that says so is about the records a repair touches rather than about how
/// far its walk runs (the walk may well run past such a record, in a chain
/// like open, declined, open, merged): a replay only ever rebuilds OPEN
/// records, and only ever onto the record below the one it is rebuilding, so
/// a merged record with nothing open above it has no tree rebuilt on top of
/// it. Retargets name only open records too, so it is never pointed anywhere
/// either. A withdrawal below it repairs the chain without touching what
/// merged.
fn merged_layer_blocking_repair(state: &OriginState) -> Option<u64> {
    under_an_open_layer(state)
        .iter()
        .find(|p| p.status == ProposalStatus::Merged)
        .map(|p| p.number)
}

/// The refusal a chain carrying a merged layer earns, with the way out named:
/// the merge has to be pulled in before the chain around it can be rebuilt.
fn merged_layer_blocks_repair(number: u64) -> RemoteError {
    RemoteError::State(format!(
        "proposal #{number} has merged and this domain has not pulled it in yet, so the layers around it cannot be rebuilt without losing what it brought; pull this domain first, then try again"
    ))
}

/// Repairs the open chain around every hole it records, in the one order the
/// forge accepts: dissolve, replay, retarget, recreate.
///
/// A hole is a recorded layer that is no longer open - the layer a withdrawal
/// just closed, or one settled out from under this machine. Every layer above
/// a hole sits on a commit that is no longer the chain, so the first survivor
/// above the LOWEST hole is replayed onto the layer below it (the trunk, for a
/// hole at the bottom) and [`cascade_replays`] carries the rest of the chain
/// up from there. Each survivor that directly follows a hole then has its pull
/// request retargeted at the branch below it, since the branch it pointed at
/// is going away.
///
/// A Merged record is a hole this cannot fill: the refusal
/// ([`merged_layer_blocks_repair`]) comes before the first call, and the pull
/// that consumes the merge is what unblocks it.
///
/// Why the order is not negotiable: a stacked proposal cannot be retargeted at
/// all (the forge answers 422), so the stack has to be dissolved before the
/// first `update_proposal` base call; and a recreate can only succeed once
/// every member's base ref points where the new chain says it does, so it
/// comes after the retargets. A dissolve of a stack that is already gone is a
/// 404 and means the work is done, not that it failed.
///
/// Every step is idempotent, because this runs again from the top whenever a
/// previous attempt died halfway: re-dissolving 404s, a retarget that is
/// already applied is a no-op, and a recreate over the survivors produces the
/// same chain whatever a previous run got through. The caller clears
/// [`crate::state::OriginState::repair_pending`] by letting this finish; a
/// failure leaves it set and the next withdraw or share resumes.
async fn repair_chain(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &mut OriginState,
    state_dir: &Path,
) -> Result<RepairOutcome, RemoteError> {
    // A merged layer is not a hole this can close: refuse before anything is
    // dissolved, replayed or retargeted.
    if let Some(number) = merged_layer_blocking_repair(state) {
        return Err(merged_layer_blocks_repair(number));
    }

    // The plan is read off the recorded chain before a single call is made:
    // where the replay starts, what it starts from, and which survivors have
    // to be pointed at a new base branch.
    let mut first_survivor: Option<usize> = None;
    let mut parent_head: Option<String> = None;
    let mut retargets: Vec<(u64, String)> = Vec::new();
    {
        let mut below: Option<&Proposal> = None;
        let mut after_hole = false;
        for (index, prop) in state.proposals.iter().enumerate() {
            if prop.status != ProposalStatus::Open {
                after_hole = true;
                continue;
            }
            if after_hole {
                if first_survivor.is_none() {
                    first_survivor = Some(index);
                    parent_head = below.and_then(|p| p.head_commit.clone());
                }
                retargets.push((
                    prop.number,
                    below
                        .map(|p| p.branch.clone())
                        .unwrap_or_else(|| spec.branch.clone()),
                ));
                after_hole = false;
            }
            below = Some(prop);
        }
    }

    // Nothing is written until every layer the cascade replays is known to be
    // replayable, so a chain that cannot be healed is refused before the
    // dissolve rather than halfway up.
    if let Some(index) = first_survivor {
        ensure_replayable(state, index)?;
    }

    let mut outcome = RepairOutcome::default();

    if let Some(stack) = state.stack_number {
        match provider.dissolve_stack(spec, stack).await {
            Ok(()) => {}
            // Already gone: an earlier attempt at this repair got this far, or
            // the forge dropped the stack itself. Either way it is done.
            Err(RemoteError::Api { status: 404, .. }) => {
                tracing::debug!("stack {stack} was already dissolved");
            }
            Err(RemoteError::StacksUnsupported) => {
                tracing::debug!("this forge no longer serves stacks; nothing to dissolve");
            }
            Err(e) => return Err(e),
        }
        state.stack_number = None;
        outcome.repaired = true;
        state.save(state_dir)?;
    }

    if let Some(index) = first_survivor {
        let parent = parent_head.unwrap_or_else(|| state.base_commit.clone());
        cascade_replays(provider, spec, state, state_dir, index, parent).await?;
        outcome.repaired = true;
    }

    for (number, base) in &retargets {
        provider
            .update_proposal(spec, *number, None, None, Some(base))
            .await?;
        outcome.repaired = true;
    }

    // A layer this machine withdrew leaves the chain for history here, once
    // the survivors above it stand on their own again.
    let withdrawn: Vec<Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Withdrawn)
        .cloned()
        .collect();
    if !withdrawn.is_empty() {
        state
            .proposals
            .retain(|p| p.status != ProposalStatus::Withdrawn);
        for record in withdrawn {
            state.push_history(record);
        }
    }

    let open: Vec<u64> = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .map(|p| p.number)
        .collect();
    if open.len() >= 2 {
        match provider.create_stack(spec, &open).await {
            Ok(info) => {
                state.stack_number = Some(info.number);
                state.stack_link_pending = false;
                outcome.restacked = Some(info.number);
                outcome.repaired = true;
            }
            Err(e) => {
                // The pull requests are all there and correctly based; only
                // the grouping is missing, which is the degraded chain
                // `stack_link_pending` records and the next operation retries.
                tracing::debug!(
                    "re-linking the repaired chain failed; it stays degraded until the next operation: {e}"
                );
                state.stack_number = None;
                state.stack_link_pending = true;
            }
        }
    } else {
        // One layer is not a chain, so there is no stack to name.
        state.stack_number = None;
        state.stack_link_pending = false;
    }

    state.repair_pending = false;
    state.save(state_dir)?;
    Ok(outcome)
}

/// Finishes a repair a previous operation left half-done, and heals a chain
/// that came apart without one, before the caller does any work of its own.
///
/// Two entry conditions, both cheap to test. [`OriginState::repair_pending`]
/// says a withdrawal died between its dissolve and its recreate; the recorded
/// chain is then re-read against the forge, since a layer this machine closed
/// before dying is still recorded Open here, and one `proposal_state` per open
/// layer settles that. [`stack_shape_broken`] says the chain and the stack
/// disagree with no repair ever having been marked - the residue of a layer
/// settled out of the chain by a share, which leaves a stack the forge will
/// refuse to extend for as long as it stands.
///
/// A layer the forge reports closed while a repair is pending is recorded as
/// [`ProposalStatus::Withdrawn`] rather than Declined: this machine closed it,
/// in the withdrawal that never finished. A layer that merged in the meantime
/// keeps its Merged status and stays for [`pull`] to consume - and blocks the
/// repair until it has been ([`merged_layer_blocking_repair`]).
///
/// One consequence of where this is called from: both call sites sit behind
/// the stacked path, so turning `github.stacks` off strands a pending repair
/// exactly as it stands until the setting is turned back on. That is the
/// intended trade - a repair is stack work, and a machine that has been told
/// not to use stacks should not be making stack calls - but a chain left
/// mid-repair stays mid-repair meanwhile.
async fn finish_pending_repair(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &mut OriginState,
    state_dir: &Path,
) -> Result<RepairOutcome, RemoteError> {
    if !state.repair_pending && !stack_shape_broken(state) {
        return Ok(RepairOutcome::default());
    }

    if state.repair_pending {
        let open: Vec<u64> = state
            .proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Open)
            .map(|p| p.number)
            .collect();
        let mut dirty = false;
        for number in open {
            let live = provider.proposal_state(spec, number).await?;
            let status = match live {
                ProposalState::Open => continue,
                ProposalState::Merged => ProposalStatus::Merged,
                ProposalState::Declined => ProposalStatus::Withdrawn,
            };
            if let Some(record) = state.proposals.iter_mut().find(|p| p.number == number) {
                record.status = status;
                record.pending_head_commit = None;
                dirty = true;
            }
        }
        if dirty {
            state.save(state_dir)?;
        }
    }

    repair_chain(provider, spec, state, state_dir).await
}

/// Withdraws a share proposal: closes its pull request on the forge (an Open
/// one), best-effort deletes its branch, optionally restores the shared files
/// to their pre-share content, and moves the record to history as
/// [`ProposalStatus::Withdrawn`].
///
/// Target: `proposal_number`, or the single Open proposal when `None`; on a
/// stacked chain with no number the target is the TOP open layer, since that
/// is the layer nothing is built on. No open proposal, or a multi-open chain
/// this machine does not know as a stack (residue from before stacking), is
/// [`RemoteError::NoWithdrawTarget`] listing every candidate, and a named
/// number that is not among the open or declined records is
/// [`RemoteError::ProposalNotFound`]. A close failure aborts the whole
/// withdraw with the error and nothing else changed. Without `revert` the
/// working tree is untouched; with it, files still matching what was proposed
/// are restored (base-tree content for Modified/Deleted, deletion for Added)
/// and diverged ones are skipped - newer work is never destroyed. On a chain
/// the pre-share content of a path a LOWER layer added is that layer's rather
/// than the trunk's, so it is fetched back from the blob that layer's record
/// names; a path nothing can speak for is named in
/// [`WithdrawReport::skipped_reverts`] and left exactly as it stands.
///
/// Taking a layer out of a stacked chain is more than a close: the layers
/// above it sit on a commit that is about to be nothing, and the forge refuses
/// to retarget a proposal while it is stacked. So the chain is repaired around
/// the hole, in [`repair_chain`]'s order - dissolve, replay the survivors onto
/// the layer below the hole, retarget the first of them, recreate the stack
/// when two or more layers survive - with
/// [`crate::state::OriginState::repair_pending`] set from before the dissolve
/// until after the recreate, so a process that dies mid-repair is finished by
/// the next withdraw or share rather than leaving a wedged chain. The
/// withdrawn layer's own branch is deleted LAST, once the repair is durable.
///
/// Only the close is atomic: a failure later, inside the revert loop, leaves
/// the pull request closed on the forge while the record is still locally
/// Open, since the state save comes last. That heals itself rather than
/// needing repair - the next [`status`] or [`pull`] classifies the closed
/// pull request and flips the record to Declined, and a retried withdraw then
/// takes the Declined path (no second close, the remaining files reverted).
///
/// `stacks_allowed` carries `github.stacks` through to the stack repair a
/// withdrawal of a stacked layer needs; a fallback withdrawal ignores it.
pub async fn withdraw(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    proposal_number: Option<u64>,
    revert: bool,
    stacks_allowed: bool,
) -> Result<WithdrawReport, RemoteError> {
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;

    // The same capability question a share asks, and the same cached answer:
    // whether this origin's chains are stacks at all decides both how a target
    // is resolved and whether a repair is owed.
    let stacked_path =
        stacks_available(provider, spec, &mut state, state_dir, stacks_allowed).await?;

    // A repair a previous operation left half-done, or a chain that came apart
    // without one, is finished before this withdrawal picks its own target:
    // the target set itself changes when a settled layer leaves the chain.
    let mut report = WithdrawReport::default();
    if stacked_path {
        let entry = finish_pending_repair(provider, spec, &mut state, state_dir).await?;
        report.repaired = entry.repaired;
        report.restacked = entry.restacked;
        // A link still owed to the forge is settled here too (spec 9.2 names
        // share, withdraw and status): the repair below dissolves and
        // recreates, and it can only do that over a chain the forge holds.
        retry_stack_link(provider, spec, &mut state, state_dir).await;
    }

    let proposal = match proposal_number {
        Some(number) => state
            .proposals
            .iter()
            .find(|p| p.number == number)
            .cloned()
            .ok_or(RemoteError::ProposalNotFound { number })?,
        None => {
            let open: Vec<&Proposal> = state
                .proposals
                .iter()
                .filter(|p| p.status == ProposalStatus::Open)
                .collect();
            let stacked_chain = stacked_path && chain_is_stacked(&state);
            match open.as_slice() {
                [single] => (*single).clone(),
                // On a stacked chain the default target is the TOP layer: it
                // is the one nothing is built on, so withdrawing it costs no
                // replay. A multi-open chain this machine does not know as a
                // stack is the legacy shape, and there the caller has to say.
                [.., top] if stacked_chain => (*top).clone(),
                _ => {
                    return Err(RemoteError::NoWithdrawTarget {
                        open: open.iter().map(|p| p.number).collect(),
                        declined: state
                            .proposals
                            .iter()
                            .filter(|p| p.status == ProposalStatus::Declined)
                            .map(|p| p.number)
                            .collect(),
                    });
                }
            }
        }
    };
    if proposal.status == ProposalStatus::Merged {
        // A Merged record can genuinely stand here: `status` flips one to
        // Merged without consuming it, and only the next pull moves it to
        // history. Withdrawing a merged proposal is refused outright.
        return Err(RemoteError::State(format!(
            "proposal #{} has already merged and cannot be withdrawn",
            proposal.number
        )));
    }

    report.number = proposal.number;

    // Where this layer sits, and what stands below it: both are read before a
    // single call, because the repair moves the record out of the chain and
    // the revert still needs the layers underneath it to restore from.
    let index = state
        .proposals
        .iter()
        .position(|p| p.number == proposal.number);
    let below: Vec<Proposal> = match index {
        Some(index) => state.proposals[..index]
            .iter()
            .filter(|p| p.status == ProposalStatus::Open)
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    let open_layers = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .count();
    // A repair is owed when this layer really is part of a chain: a stack the
    // forge holds, or a second open layer that would be left dangling.
    let repair_owed = stacked_path
        && proposal.status == ProposalStatus::Open
        && chain_is_stacked(&state)
        && (state.stack_number.is_some() || open_layers >= 2);

    if let (true, Some(index)) = (repair_owed, index) {
        // Nothing is written until every layer above this one can be replayed:
        // a chain left half re-based is worse than one never touched. A merged
        // layer this domain has not pulled in is the other refusal, and it
        // belongs here rather than inside the repair, so the pull request is
        // never closed for a repair that then cannot run.
        ensure_replayable(&state, index + 1)?;
        if let Some(number) = merged_layer_blocking_repair(&state) {
            return Err(merged_layer_blocks_repair(number));
        }
        provider.close_proposal(spec, proposal.number).await?;
        report.closed = true;
        // From here the chain is knowingly inconsistent, and the flag says so
        // until the recreate lands. The withdrawn record stays in the chain,
        // marked, so the repair can see where the hole is; `repair_chain`
        // settles it to history once the survivors stand on their own.
        state.proposals[index].status = ProposalStatus::Withdrawn;
        state.repair_pending = true;
        state.save(state_dir)?;

        let outcome = repair_chain(provider, spec, &mut state, state_dir).await?;
        report.repaired |= outcome.repaired;
        report.restacked = outcome.restacked.or(report.restacked);

        if revert {
            revert_layer_files(
                provider,
                spec,
                domain_root,
                state_dir,
                &proposal,
                &below,
                &mut report,
            )
            .await?;
        }

        // The branch goes last, once the repaired chain is durable: a survivor
        // replayed onto a branch that is already gone would have nothing to
        // sit on if this ran earlier and the repair then failed.
        let _ = provider.delete_branch(spec, &proposal.branch).await;
        return Ok(report);
    }

    // Close first: a failure here aborts with nothing else changed. A
    // Declined proposal is already closed on the forge, so only the branch
    // cleanup applies to it.
    if proposal.status == ProposalStatus::Open {
        provider.close_proposal(spec, proposal.number).await?;
        report.closed = true;
    }
    let _ = provider.delete_branch(spec, &proposal.branch).await;

    if revert {
        revert_layer_files(
            provider,
            spec,
            domain_root,
            state_dir,
            &proposal,
            &below,
            &mut report,
        )
        .await?;
    }

    state.proposals.retain(|p| p.number != proposal.number);
    let mut record = proposal.clone();
    record.status = ProposalStatus::Withdrawn;
    state.push_history(record);
    state.save(state_dir)?;

    Ok(report)
}

/// Puts the working tree back the way it stood before `proposal` was shared,
/// for every file the proposal itself changed and no other.
///
/// A file that diverged since sharing is never touched: the recorded digest is
/// what says whether the local copy is still the one that was proposed, and
/// anything else is newer work. What a restore reads from is the base
/// snapshot - except where the trunk never carried the file at all, which is
/// the shape a stacked chain makes possible: a layer below added the path and
/// this layer modified or retired it, so the pre-share content is that lower
/// layer's, and the only copy of it is the blob its record names. That blob is
/// fetched by sha and CHECKED against the lower record's digest before it is
/// written; a record with no blob sha, a fetch that fails and content that
/// does not hash to what was recorded all leave the path exactly as it stands
/// and name it in [`WithdrawReport::skipped_reverts`]. A withdrawal is never
/// failed over a file that cannot be put back.
async fn revert_layer_files(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    proposal: &Proposal,
    below: &[Proposal],
    report: &mut WithdrawReport,
) -> Result<(), RemoteError> {
    for pf in &proposal.files {
        let wt_path = checked_working_path(state_dir, domain_root, &pf.path)?;
        let current = read_optional_file(&wt_path)?;
        let current_sha = current.as_deref().map(state::sha256_hex);

        let diverged = match pf.change {
            ProposedChange::Added | ProposedChange::Modified => {
                current_sha.as_deref() != pf.sha256.as_deref()
            }
            ProposedChange::Deleted => current.is_some(),
        };
        if diverged {
            report.skipped_diverged.push(pf.path.clone());
            continue;
        }

        match pf.change {
            ProposedChange::Added => {
                remove_working_file(&wt_path)?;
                report.deleted.push(pf.path.clone());
            }
            ProposedChange::Modified | ProposedChange::Deleted => {
                match state::read_base_file(state_dir, &pf.path)? {
                    Some(bytes) => {
                        write_working_file(&wt_path, &bytes)?;
                        report.restored.push(pf.path.clone());
                    }
                    // The trunk has no copy. On a chain that means a layer
                    // below owns the path, and its recorded blob is the
                    // pre-share content; anywhere else there is simply nothing
                    // to restore from and the file is left alone.
                    None => match layer_below_content(provider, spec, below, &pf.path).await {
                        Some(bytes) => {
                            write_working_file(&wt_path, &bytes)?;
                            report.restored.push(pf.path.clone());
                        }
                        None => report.skipped_reverts.push(pf.path.clone()),
                    },
                }
            }
        }
    }
    Ok(())
}

/// The content the nearest layer below carries at `path`, fetched by blob sha
/// and verified against that layer's recorded digest, or `None` when no layer
/// below can speak for the path with content this function is willing to
/// write.
///
/// Nearest first, since a higher layer's version is the one that stood
/// directly beneath the withdrawn layer. Every way this can go wrong answers
/// `None` rather than an error: a record from before blob shas existed, a
/// record with no digest to check against, a blob the forge no longer has, and
/// content that does not hash to what was recorded. The caller reports the
/// path as unrestored, which is the honest outcome.
async fn layer_below_content(
    provider: &dyn Provider,
    spec: &OriginSpec,
    below: &[Proposal],
    path: &str,
) -> Option<Vec<u8>> {
    for layer in below.iter().rev() {
        let Some(file) = layer.files.iter().find(|f| f.path == path) else {
            continue;
        };
        match file.change {
            ProposedChange::Added | ProposedChange::Modified => {}
            // This layer retired the path too, so it is not what stood below.
            ProposedChange::Deleted => return None,
        }
        let (Some(blob_sha), Some(recorded)) = (file.blob_sha.as_deref(), file.sha256.as_deref())
        else {
            tracing::debug!(
                "layer #{} records {path} without a blob to restore it from",
                layer.number
            );
            return None;
        };
        match provider.blob(spec, blob_sha).await {
            Ok(bytes) if state::sha256_hex(&bytes) == recorded => return Some(bytes),
            Ok(_) => {
                tracing::debug!(
                    "the blob layer #{} records for {path} does not hash to its recorded digest",
                    layer.number
                );
                return None;
            }
            Err(e) => {
                tracing::debug!(
                    "fetching layer #{}'s copy of {path} failed: {e}",
                    layer.number
                );
                return None;
            }
        }
    }
    None
}

/// Resolves one recorded conflict at `path`: settles it per `resolution`,
/// clears its recorded conflict copies and drops it from state.
///
/// Errors with [`RemoteError::ConflictNotFound`], naming `path` and listing
/// every currently open conflict path, when there is no open conflict there.
/// Offline: this never talks to a provider.
pub fn resolve(
    domain_root: &Path,
    state_dir: &Path,
    path: &str,
    resolution: Resolution<'_>,
) -> Result<ResolveReport, RemoteError> {
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;

    let conflict = state
        .conflicts
        .iter()
        .find(|c| c.path == path)
        .cloned()
        .ok_or_else(|| RemoteError::ConflictNotFound {
            path: path.to_string(),
            open: state.conflicts.iter().map(|c| c.path.clone()).collect(),
        })?;

    let wt_path = checked_working_path(state_dir, domain_root, path)?;
    match resolution {
        Resolution::Mine => {}
        Resolution::Theirs => {
            let (_, upstream) = state::read_conflict_files(state_dir, &conflict.id)?;
            match upstream {
                Some(bytes) => write_working_file(&wt_path, &bytes)?,
                None => remove_working_file(&wt_path)?,
            }
        }
        Resolution::Merged(content) => write_working_file(&wt_path, content)?,
    }

    state::clear_conflict(state_dir, &conflict.id)?;
    state.conflicts.retain(|c| c.id != conflict.id);
    state.save(state_dir)?;

    Ok(ResolveReport {
        resolved: path.to_string(),
        remaining: state.conflicts.len(),
    })
}

/// Handles the "nothing new upstream" outcome of a pull: refresh open
/// proposals (a proposal can still be declined without the branch moving),
/// consume any merged record, persist the result and report up to date.
///
/// The consumption is not an optimization. A merged record still sitting in
/// the chain blocks every repair around it ([`merged_layer_blocking_repair`]),
/// and the refusal's way out is "pull this domain first" - so a pull that
/// finds nothing new upstream has to finish that record off too, or the advice
/// names an operation that cannot clear it. No rebase is adopted here: the
/// trunk never moved, so nothing above the merge was rebased onto it.
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
    let (transitions, touched) = refresh_proposals(provider, spec, &mut state).await?;
    let consumed = consume_merged(&mut state);
    let mut dirty = touched || !consumed.is_empty();
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
    // Best-effort branch cleanup once the state is durable, exactly as the
    // moved-trunk arm does it.
    for prop in &consumed {
        let _ = provider.delete_branch(spec, &prop.branch).await;
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

    // Rebuild the artifact mirror from the same fetched tree, before the base
    // is replaced, so a head that turned a decl hostile fails the re-baseline
    // with the previous mirror intact.
    refresh_artifact_mirror(state_dir, spec.subpath.as_deref(), &bytes)?;

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

/// Refreshes the status of every open proposal against the provider, records
/// the transitions and pulls fresh review feedback onto every record still
/// open afterwards. Merged proposals are marked but not yet consumed; the
/// caller decides when to move them to history.
///
/// Returns the changed proposals as `(number, new status)` together with
/// whether any record was touched at all. The two are not the same thing: a
/// feedback refresh with no status transition still changes a record, and the
/// caller has to know that to persist it.
async fn refresh_proposals(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &mut OriginState,
) -> Result<(Vec<(u64, ProposalStatus)>, bool), RemoteError> {
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

    // Feedback rides on every still-open proposal, best-effort: a failed
    // fetch keeps the previous feedback and never fails the pull.
    let mut touched = !transitions.is_empty();
    for prop in state.proposals.iter_mut() {
        if prop.status != ProposalStatus::Open {
            continue;
        }
        match provider.proposal_feedback(spec, prop.number).await {
            Ok(feedback) => {
                prop.review_state = feedback.review_state;
                let mut items = feedback.items;
                items.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
                items.truncate(crate::state::FEEDBACK_CAP);
                prop.feedback = items;
                prop.updated_at = Some(Utc::now());
                touched = true;
            }
            Err(e) => {
                tracing::debug!(
                    "feedback fetch for proposal #{} failed; keeping the previous feedback: {e}",
                    prop.number
                );
            }
        }
    }
    Ok((transitions, touched))
}

/// Builds the domain-relative upstream change set from a compare result,
/// filtering to the domain subtree and enforcing the shared-file size cap on
/// upstream content. Falls back to a whole-tree tarball diff when the compare
/// is truncated or lists more than [`MAX_COMPARE_FILES`] files.
///
/// The third element of the return is the tarball bytes the fallback path
/// fetched, `Some` when the whole-tree diff ran and `None` for a plain
/// compare, so the caller can refresh the artifact mirror from the very same
/// bytes rather than fetching a second tarball.
async fn upstream_edits(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &OriginState,
    head: &str,
    cmp: CompareResult,
) -> Result<(Vec<UpstreamEdit>, Vec<(String, u64)>, Option<Vec<u8>>), RemoteError> {
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
        let (edits, skipped, bytes) =
            upstream_edits_from_tarball(provider, spec, state, head).await?;
        return Ok((edits, skipped, Some(bytes)));
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
    Ok((edits, skipped_large, None))
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
///
/// Returns the fetched tarball bytes alongside the change set so the caller
/// can refresh the artifact mirror from the same download.
async fn upstream_edits_from_tarball(
    provider: &dyn Provider,
    spec: &OriginSpec,
    state: &OriginState,
    head: &str,
) -> Result<(Vec<UpstreamEdit>, Vec<(String, u64)>, Vec<u8>), RemoteError> {
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
    Ok((edits, skipped_large, bytes))
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
/// `None` when it lies outside the subtree or names a hidden path. Mirrors
/// the prefix stripping [`crate::archive::extract_tarball`] applies, so the
/// compare and tarball paths agree on which files belong to the domain, and
/// applies the same [`crate::changes::is_excluded_path`] rule that function
/// does: a hidden or reserved upstream change is dropped here, before the caller ever
/// fetches a blob for it or stamps it into
/// [`crate::state::OriginState::files`], so a compare-driven pull can never
/// disagree with a tarball-driven one about which files are hidden.
fn to_domain_relative(repo_rel: &str, subpath: Option<&str>) -> Option<String> {
    let rel = match subpath {
        None => repo_rel.to_string(),
        Some(sub) => {
            let prefix = format!("{}/", sub.trim_matches('/'));
            repo_rel.strip_prefix(&prefix).map(str::to_string)?
        }
    };
    if crate::changes::is_excluded_path(&rel) {
        return None;
    }
    Some(rel)
}

/// Maps a domain-relative path to its repo-relative form under `subpath`,
/// prefixing `subpath` back on. The inverse of [`to_domain_relative`], built
/// from the exact same prefix so the two stay in agreement; a change to
/// either's stripping or prefixing rule must be made to both.
fn to_repo_relative(domain_rel: &str, subpath: Option<&str>) -> String {
    match subpath {
        None => domain_rel.to_string(),
        Some(sub) => format!("{}/{domain_rel}", sub.trim_matches('/')),
    }
}

/// The out-of-subtree provisioning roots a MANIFEST declares, each as
/// `(kind id, repo-relative root)`, with the repository-root escape check
/// already applied.
///
/// The decl set is read from the fetched MANIFEST's own bytes, never the
/// local working tree: the mirror is canonical upstream content arriving
/// through the same trusted pull channel as engrams, so a local-only decl a
/// user has not shared yet simply resolves to an empty mirror dir until it is
/// shared. A decl that stays inside the subtree (no `..` climb) is omitted: the
/// working tree already serves it. A decl that climbs out of the subtree has
/// its `subpath + decl.path` normalized against the repository root; one that
/// climbs past the repository root is a hard [`RemoteError::State`] naming the
/// decl, since a repo-bounded mirror is a security invariant. An unreadable or
/// unparseable MANIFEST, or one with no Provisioning section, declares no
/// roots.
fn mirror_roots(
    manifest_source: Option<&str>,
    subpath: Option<&str>,
) -> Result<Vec<(&'static str, String)>, RemoteError> {
    let Some(source) = manifest_source else {
        return Ok(Vec::new());
    };
    let Ok(engram) = parse_engram(source) else {
        return Ok(Vec::new());
    };
    let manifest = Manifest::from_engram(&engram, source);
    let Some(section) = manifest.provisioning() else {
        return Ok(Vec::new());
    };

    let mut roots = Vec::new();
    for decl in &section.decls {
        let (_, climbs) = crystalline_core::manifest::normalize_relative(&decl.path);
        if climbs == 0 {
            // In-subtree (or root-landing): the working tree serves it, exactly
            // as `resolve_source_roots` resolves it against the domain root.
            continue;
        }
        // Combine with the subtree's own repo-relative location and normalize
        // against the repository root.
        let combined = match subpath {
            Some(sub) => format!("{}/{}", sub.trim_matches('/'), decl.path),
            None => decl.path.clone(),
        };
        let (kept, climbs) = crystalline_core::manifest::normalize_relative(&combined);
        if climbs > 0 {
            return Err(RemoteError::State(format!(
                "provisioning decl `{}: {}` escapes the repository root and cannot be mirrored",
                decl.kind.id(),
                decl.path
            )));
        }
        roots.push((decl.kind.id(), kept.join("/")));
    }
    Ok(roots)
}

/// Writes the team-domain artifact mirror under `state_dir/artifacts` from a
/// tarball's bytes, driven by `manifest_source`'s out-of-subtree decls.
///
/// Each declared out-of-subtree folder is sliced out of the same tarball and
/// clear-then-written into `artifacts/<kind>`; every kind no longer declared
/// is pruned, and the whole `artifacts` directory falls away once nothing is
/// declared (a MANIFEST that dropped its Provisioning section). The escape
/// check in [`mirror_roots`] runs before any directory is touched, so a
/// hostile decl fails the whole operation with the previous mirror intact.
fn write_artifact_mirror(
    state_dir: &Path,
    subpath: Option<&str>,
    manifest_source: Option<&str>,
    tarball_bytes: &[u8],
) -> Result<(), RemoteError> {
    let roots = mirror_roots(manifest_source, subpath)?;
    let desired: BTreeSet<&str> = roots.iter().map(|(kind, _)| *kind).collect();
    for (kind_id, repo_root) in &roots {
        let (files, _skipped_large) = extract_repo_subtree(tarball_bytes, repo_root)?;
        state::replace_artifact_kind(state_dir, kind_id, &files)?;
    }
    state::prune_artifact_kinds(state_dir, &desired)?;
    Ok(())
}

/// Refreshes the artifact mirror from a fetched tarball whose MANIFEST is read
/// back out of the same bytes. The pull-side entry point where only the
/// tarball is in hand (the compare refresh, both tarball fallbacks); subscribe
/// passes its already-extracted MANIFEST straight to [`write_artifact_mirror`].
fn refresh_artifact_mirror(
    state_dir: &Path,
    subpath: Option<&str>,
    tarball_bytes: &[u8],
) -> Result<(), RemoteError> {
    let (subtree, _skipped_large) = extract_tarball(tarball_bytes, subpath)?;
    let manifest_source = subtree
        .get("MANIFEST.md")
        .and_then(|bytes| std::str::from_utf8(bytes).ok());
    write_artifact_mirror(state_dir, subpath, manifest_source, tarball_bytes)
}

/// Whether a compare result could have moved the artifact mirror, decided
/// against the base snapshot's MANIFEST decls (repo-relative, before subpath
/// filtering): true when a changed path falls under a declared out-of-subtree
/// root, or when the subtree MANIFEST itself changed (its decls may have
/// shifted, adding or dropping a mirrored folder). A base MANIFEST that
/// somehow no longer parses cleanly is treated as needing a refresh, which
/// then re-validates the fetched MANIFEST.
fn mirror_refresh_needed(
    cmp: &CompareResult,
    state_dir: &Path,
    subpath: Option<&str>,
) -> Result<bool, RemoteError> {
    let base_manifest = state::read_base_file(state_dir, "MANIFEST.md")?;
    let roots = match base_manifest
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    {
        Some(source) => match mirror_roots(Some(source), subpath) {
            Ok(roots) => roots,
            Err(_) => return Ok(true),
        },
        None => Vec::new(),
    };
    let manifest_key = to_repo_relative("MANIFEST.md", subpath);
    for change in &cmp.files {
        let mut paths = vec![change.path.as_str()];
        if let ChangeKind::Renamed { previous } = &change.kind {
            paths.push(previous.as_str());
        }
        for path in paths {
            if path == manifest_key {
                return Ok(true);
            }
            if roots.iter().any(|(_, root)| path_under_root(path, root)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether repo-relative `path` sits at or under repo-relative `root`. An
/// empty `root` (a decl resolving onto the repository root itself) covers the
/// whole tree.
fn path_under_root(path: &str, root: &str) -> bool {
    root.is_empty() || path == root || path.starts_with(&format!("{root}/"))
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

/// Per-kind change entries carrying enough content for [`generate_body`] to
/// derive an engram's frontmatter title: `(domain-relative path, content)`
/// for added and modified files (the same bytes already read from disk to
/// build their blob) and `(path, base-tree content)` for deleted files
/// (their last known content, read back from the base snapshot since the
/// working copy is already gone by the time a deletion is proposed).
#[derive(Default)]
struct ChangeEntries {
    added: Vec<(String, Vec<u8>)>,
    updated: Vec<(String, Vec<u8>)>,
    deleted: Vec<(String, Option<Vec<u8>>)>,
}

/// Builds a share branch name: `crystalline/share-<slug>-<timestamp>-<hex4>`.
///
/// `slug` is `domain_name` lowercased with every character outside
/// `[a-z0-9-]` replaced, one for one, by `-`: a direct character map, not
/// [`crystalline_core::slugify`]'s segment-aware collapsing (consecutive
/// replaced characters are not merged into one hyphen). `timestamp` is the
/// current UTC time as `yymmddHHMMSS`, keeping repeated shares of the same
/// domain from colliding on the same branch name, and a 4-hex-char suffix
/// drawn from a random UUID v4 keeps two shares in the same second from
/// colliding on one branch name.
///
/// GitHub's client does not percent-encode URL path segments, so a branch
/// name carrying any character outside `[a-z0-9-]` would break a proposal's
/// browser URL; the fixed `crystalline/share-` prefix's own `/` is safe
/// because it sits outside the sanitized segment, not inside it.
fn share_branch_name(domain_name: &str) -> String {
    let slug: String = domain_name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let timestamp = Utc::now().format("%y%m%d%H%M%S");
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..4];
    format!("crystalline/share-{slug}-{timestamp}-{suffix}")
}

/// `singular` when `count == 1`, else `plural`. Every noun this module
/// pluralizes ("engram"/"engrams") is regular, so nothing richer than this
/// is needed.
fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

/// Generates a proposal title from the change mix, by three simple,
/// deterministic rules:
///
/// - only additions -> `"Share N new engram(s) from <domain>"`
/// - only modifications -> `"Refine N engram(s) in <domain>"`
/// - anything else (deletions alone, or any mix of two or three kinds) ->
///   `"Share updates from <domain>"`
///
/// Used as the proposal title, and (unless the caller supplies their own
/// title) the commit message too.
fn generate_title(added: usize, updated: usize, deleted: usize, domain_name: &str) -> String {
    match (added > 0, updated > 0, deleted > 0) {
        (true, false, false) => format!(
            "Share {added} new {} from {domain_name}",
            pluralize(added, "engram", "engrams")
        ),
        (false, true, false) => format!(
            "Refine {updated} {} in {domain_name}",
            pluralize(updated, "engram", "engrams")
        ),
        _ => format!("Share updates from {domain_name}"),
    }
}

/// The proposal body's first line (and [`ProposeReport::summary`]):
/// `"Shares X new engram(s), refines Y engram(s) and retires Z engram(s)."`
/// with a zero-count clause omitted entirely, singular or plural chosen per
/// count, and no Oxford comma before the final "and" (see
/// [`join_clauses`]).
fn generate_summary_line(added: usize, updated: usize, deleted: usize) -> String {
    let mut clauses = Vec::new();
    if added > 0 {
        clauses.push(format!(
            "shares {added} new {}",
            pluralize(added, "engram", "engrams")
        ));
    }
    if updated > 0 {
        clauses.push(format!(
            "refines {updated} {}",
            pluralize(updated, "engram", "engrams")
        ));
    }
    if deleted > 0 {
        clauses.push(format!(
            "retires {deleted} {}",
            pluralize(deleted, "engram", "engrams")
        ));
    }
    format!("{}.", capitalize_first(&join_clauses(&clauses)))
}

/// Joins clauses with no Oxford comma: `["a"]` -> `"a"`, `["a", "b"]` ->
/// `"a and b"`, `["a", "b", "c"]` -> `"a, b and c"`.
fn join_clauses(clauses: &[String]) -> String {
    match clauses.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Uppercases the first character of `s`, leaving the rest as is.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Generates a proposal body when the caller supplies no description of
/// their own: `summary`, then a bulleted list per change kind naming each
/// file by its engram title where one can be found (see [`engram_title`]),
/// ending with a plain footer line naming the domain. No AI attribution
/// anywhere: nothing here, or anywhere else in this crate, credits a tool
/// for the content.
fn generate_body(summary: &str, entries: &ChangeEntries, domain_name: &str) -> String {
    let mut body = String::new();
    body.push_str(summary);
    body.push('\n');
    append_section(
        &mut body,
        "Added",
        entries
            .added
            .iter()
            .map(|(path, content)| engram_title(path, Some(content))),
    );
    append_section(
        &mut body,
        "Modified",
        entries
            .updated
            .iter()
            .map(|(path, content)| engram_title(path, Some(content))),
    );
    append_section(
        &mut body,
        "Deleted",
        entries
            .deleted
            .iter()
            .map(|(path, content)| engram_title(path, content.as_deref())),
    );
    body.push_str(&format!("\nDomain: {domain_name}\n"));
    body
}

/// Appends a `<header>:` section listing `entries` as markdown bullets, or
/// nothing at all when `entries` is empty (a change kind with no files gets
/// no section header, rather than an empty one).
fn append_section(body: &mut String, header: &str, entries: impl Iterator<Item = String>) {
    let mut entries = entries.peekable();
    if entries.peek().is_none() {
        return;
    }
    body.push_str(&format!("\n{header}:\n"));
    for entry in entries {
        body.push_str(&format!("- {entry}\n"));
    }
}

/// The display entry for one changed file in a generated proposal body:
/// `"<title> (<path>)"` when `content` is markdown with a non-empty
/// frontmatter title, else the bare path. One fallback covers three cases at
/// once: a non-`.md` asset, content that fails to parse as an engram, and
/// content absent entirely (a deleted file whose base copy could not be
/// read back).
fn engram_title(path: &str, content: Option<&[u8]>) -> String {
    if !path.ends_with(".md") {
        return path.to_string();
    }
    let title = content
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| parse_engram(text).ok())
        .map(|engram| engram.frontmatter.title)
        .filter(|title| !title.is_empty());
    match title {
        Some(title) => format!("{title} ({path})"),
        None => path.to_string(),
    }
}
