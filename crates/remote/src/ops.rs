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
}

/// What [`propose`] did with a domain's local changes.
#[derive(Debug, Clone, PartialEq)]
pub enum ProposeOutcome {
    /// A pull request was opened.
    Proposed(ProposeReport),
    /// Success-shaped, not an error: the team already has everything this
    /// domain knows, so there was nothing to open a pull request for.
    NothingToShare {
        /// Working-tree files skipped for exceeding
        /// [`MAX_SHARED_FILE_BYTES`], each with its size in bytes.
        skipped_large: Vec<(String, u64)>,
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

/// What [`discard`] did with a declined or still-open proposal's files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscardReport {
    /// Domain-relative paths restored to their base-tree content (a
    /// proposed `Modified` or `Deleted` file whose local copy still matched
    /// what was proposed).
    pub restored: Vec<String>,
    /// Domain-relative paths deleted (a proposed `Added` file whose local
    /// copy still matched what was proposed).
    pub deleted: Vec<String>,
    /// Domain-relative paths left untouched because the local file no
    /// longer matches what was proposed: newer work is never destroyed.
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
/// are none; upload each added or modified file's content as a blob, build a
/// tree from the base commit with every domain-relative path re-prefixed to
/// its repo-relative form (see [`to_repo_relative`]), commit it, open a
/// share branch named per [`share_branch_name`] and open the pull request;
/// finally record the proposal in state (status `Open`) and save. Local
/// files are never touched: a share only ever reads them.
pub async fn propose(
    provider: &dyn Provider,
    spec: &OriginSpec,
    domain_root: &Path,
    domain_name: &str,
    state_dir: &Path,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<ProposeOutcome, RemoteError> {
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

    let local = detect_local_changes(domain_root, &state.files)?;
    if local.changes.is_empty() {
        return Ok(ProposeOutcome::NothingToShare {
            skipped_large: local.skipped_large,
        });
    }

    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut deleted = Vec::new();
    let mut writes = Vec::new();
    let mut files = Vec::new();
    let mut entries = ChangeEntries::default();

    for change in &local.changes {
        match change {
            LocalChange::Added { path, sha256 } => {
                let wt_path = checked_working_path(state_dir, domain_root, path)?;
                let bytes = std::fs::read(&wt_path)?;
                let blob_sha = provider.create_blob(spec, &bytes).await?;
                writes.push(TreeWrite {
                    path: to_repo_relative(path, spec.subpath.as_deref()),
                    blob_sha: Some(blob_sha),
                });
                entries.added.push((path.clone(), bytes));
                added.push(path.clone());
                files.push(ProposedFile {
                    path: path.clone(),
                    change: ProposedChange::Added,
                    sha256: Some(sha256.clone()),
                });
            }
            LocalChange::Modified { path, sha256 } => {
                let wt_path = checked_working_path(state_dir, domain_root, path)?;
                let bytes = std::fs::read(&wt_path)?;
                let blob_sha = provider.create_blob(spec, &bytes).await?;
                writes.push(TreeWrite {
                    path: to_repo_relative(path, spec.subpath.as_deref()),
                    blob_sha: Some(blob_sha),
                });
                entries.updated.push((path.clone(), bytes));
                updated.push(path.clone());
                files.push(ProposedFile {
                    path: path.clone(),
                    change: ProposedChange::Modified,
                    sha256: Some(sha256.clone()),
                });
            }
            LocalChange::Deleted { path } => {
                writes.push(TreeWrite {
                    path: to_repo_relative(path, spec.subpath.as_deref()),
                    blob_sha: None,
                });
                // Only worth reading back the retired file's last known
                // content (for its engram title in the generated body) when
                // there is a generated body to put it in at all.
                if description.is_none() {
                    let base_content = state::read_base_file(state_dir, path)?;
                    entries.deleted.push((path.clone(), base_content));
                }
                deleted.push(path.clone());
                files.push(ProposedFile {
                    path: path.clone(),
                    change: ProposedChange::Deleted,
                    sha256: None,
                });
            }
        }
    }

    let generated_title = generate_title(added.len(), updated.len(), deleted.len(), domain_name);
    let effective_title = title.map(str::to_string).unwrap_or(generated_title);
    let summary = generate_summary_line(added.len(), updated.len(), deleted.len());
    let body = description
        .map(str::to_string)
        .unwrap_or_else(|| generate_body(&summary, &entries, domain_name));

    let tree_sha = provider
        .create_tree(spec, &state.base_commit, &writes)
        .await?;
    let commit_sha = provider
        .create_commit(spec, &effective_title, &tree_sha, &state.base_commit)
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
        files,
    });
    state.save(state_dir)?;

    Ok(ProposeOutcome::Proposed(ProposeReport {
        url: handle.url,
        number: handle.number,
        branch,
        added,
        updated,
        deleted,
        skipped_large: local.skipped_large,
        summary,
    }))
}

/// Discards a declined, or still-open ("never mind"), share proposal:
/// restores each proposed file that still matches what was proposed to its
/// pre-proposal content (the base tree, or plain deletion for a proposed
/// addition), leaves any file whose local content has since diverged
/// untouched, then moves the proposal record to history with whatever
/// status it currently holds (discarding never rewrites `status` itself).
///
/// Offline: this never talks to a provider. Discarding an `Open` proposal
/// does not close its pull request on the origin; that happens on GitHub
/// itself, and the next pull's proposal refresh marks it `Declined` once it
/// is closed there.
pub fn discard(
    domain_root: &Path,
    state_dir: &Path,
    proposal_number: u64,
) -> Result<DiscardReport, RemoteError> {
    let mut state = OriginState::load(state_dir)?.ok_or_else(|| {
        RemoteError::State(
            "this domain has no origin state; add the domain from its origin first".to_string(),
        )
    })?;

    let proposal = state
        .proposals
        .iter()
        .find(|p| p.number == proposal_number)
        .cloned()
        .ok_or(RemoteError::ProposalNotFound {
            number: proposal_number,
        })?;
    if proposal.status == ProposalStatus::Merged {
        // Not reachable through the normal pull/propose lifecycle (a merged
        // proposal is consumed straight to history), but guarded explicitly
        // since a discard must never apply to one.
        return Err(RemoteError::State(format!(
            "proposal #{proposal_number} has already merged and cannot be discarded"
        )));
    }

    let mut restored = Vec::new();
    let mut deleted = Vec::new();
    let mut skipped_diverged = Vec::new();

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

    state.proposals.retain(|p| p.number != proposal_number);
    state.push_history(proposal);
    state.save(state_dir)?;

    Ok(DiscardReport {
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
/// applies the same [`crate::changes::is_hidden_path`] rule that function
/// does: a hidden upstream change is dropped here, before the caller ever
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
    if crate::changes::is_hidden_path(&rel) {
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

/// Builds a share branch name: `crystalline/share-<slug>-<timestamp>`.
///
/// `slug` is `domain_name` lowercased with every character outside
/// `[a-z0-9-]` replaced, one for one, by `-`: a direct character map, not
/// [`crystalline_core::slugify`]'s segment-aware collapsing (consecutive
/// replaced characters are not merged into one hyphen). `timestamp` is the
/// current UTC time as `yymmddHHMMSS`, keeping repeated shares of the same
/// domain from colliding on the same branch name.
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
    format!("crystalline/share-{slug}-{timestamp}")
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
