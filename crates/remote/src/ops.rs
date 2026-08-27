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

    // Consume merged proposals in memory, then persist base advance,
    // conflicts and history together in one atomic save so a crash cannot
    // leave a merged proposal half-consumed.
    for prop in &merged_to_consume {
        state.proposals.retain(|p| p.number != prop.number);
        let mut consumed = prop.clone();
        consumed.status = ProposalStatus::Merged;
        state.push_history(consumed);
    }
    state.save(state_dir)?;

    // Best-effort branch cleanup, after the state is durable; errors are
    // ignored entirely (the branch lingering upstream harms nothing).
    for prop in &merged_to_consume {
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
/// `stacks_allowed` carries `github.stacks` through to the stack-link retry a
/// status is allowed to make; nothing in the report itself depends on it.
pub async fn status(
    spec: &OriginSpec,
    domain_root: &Path,
    state_dir: &Path,
    probe: Option<&dyn Provider>,
    stacks_allowed: bool,
) -> Result<OriginStatusReport, RemoteError> {
    let _ = stacks_allowed;
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
    })
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
        Err(e) => return Err(e),
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
#[allow(dead_code)] // The stacked share path is this helper's only caller.
fn chain_is_stacked(state: &OriginState) -> bool {
    let open = state
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .count();
    open <= 1 || state.stack_number.is_some() || state.stack_link_pending
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
/// A domain has at most one open proposal at a time. When one is open, this
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
    let _stacked_path = stacks_available(
        provider,
        spec,
        &mut state,
        state_dir,
        options.stacks_allowed,
    )
    .await?;

    let local = detect_local_changes(domain_root, &state.files)?;
    if local.changes.is_empty() {
        return Ok(ProposeOutcome::NothingToShare {
            skipped_large: local.skipped_large,
        });
    }

    // 2. A declined proposal is superseded by this share: record to history
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

    // 3. An open proposal is updated in place, never paralleled.
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

    // 4. Create, as today, plus head_commit/base_commit on the fresh record.
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
    let state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;
    let local = detect_local_changes(domain_root, &state.files)?;

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

    // Classification mirrors propose's, read-only: a declined record would be
    // superseded (so it reads as Create), an open one is an Update unless a
    // reviewer amended its branch, and a gone ref would be re-created.
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
                });
            }
            LocalChange::Modified { path, sha256 } => {
                let wt_path = checked_working_path(state_dir, domain_root, path)?;
                let bytes = std::fs::read(&wt_path)?;
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
    }))
}

/// Withdraws a share proposal: closes its pull request on the forge (an Open
/// one), best-effort deletes its branch, optionally restores the shared files
/// to their pre-share content, and moves the record to history as
/// [`ProposalStatus::Withdrawn`].
///
/// Target: `proposal_number`, or the single Open proposal when `None`;
/// no open proposal (or more than one, possible only in pre-living-proposal
/// state) is [`RemoteError::NoWithdrawTarget`] listing every candidate, and a
/// named number that is not among the open or declined records is
/// [`RemoteError::ProposalNotFound`]. A close failure aborts the whole
/// withdraw with the error and nothing else changed. Without `revert` the
/// working tree is untouched; with it, files still matching what was proposed
/// are restored (base-tree content for Modified/Deleted, deletion for Added)
/// and diverged ones are skipped - newer work is never destroyed.
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
    let _ = stacks_allowed;
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;

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
            match open.as_slice() {
                [single] => (*single).clone(),
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

    // Close first: a failure here aborts with nothing else changed. A
    // Declined proposal is already closed on the forge, so only the branch
    // cleanup applies to it.
    let closed = if proposal.status == ProposalStatus::Open {
        provider.close_proposal(spec, proposal.number).await?;
        true
    } else {
        false
    };
    let _ = provider.delete_branch(spec, &proposal.branch).await;

    let mut restored = Vec::new();
    let mut deleted = Vec::new();
    let mut skipped_diverged = Vec::new();
    if revert {
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
                skipped_diverged.push(pf.path.clone());
                continue;
            }

            match pf.change {
                ProposedChange::Added => {
                    remove_working_file(&wt_path)?;
                    deleted.push(pf.path.clone());
                }
                ProposedChange::Modified | ProposedChange::Deleted => {
                    match state::read_base_file(state_dir, &pf.path)? {
                        Some(bytes) => {
                            write_working_file(&wt_path, &bytes)?;
                            restored.push(pf.path.clone());
                        }
                        // No base copy to restore from (should not happen: a
                        // Modified or Deleted change always had a base entry);
                        // never destroy the local file over it, so this is left
                        // alone like a genuine divergence.
                        None => skipped_diverged.push(pf.path.clone()),
                    }
                }
            }
        }
    }

    state.proposals.retain(|p| p.number != proposal.number);
    let mut record = proposal.clone();
    record.status = ProposalStatus::Withdrawn;
    state.push_history(record);
    state.save(state_dir)?;

    Ok(WithdrawReport {
        number: proposal.number,
        closed,
        restored,
        deleted,
        skipped_diverged,
    })
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
    let (transitions, touched) = refresh_proposals(provider, spec, &mut state).await?;
    let mut dirty = touched;
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
