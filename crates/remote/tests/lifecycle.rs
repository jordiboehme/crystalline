//! End-to-end lifecycle tests for the pull-side and share-side orchestration
//! in `crystalline_remote::ops` (`subscribe`, `pull`, `status`, `propose`,
//! `withdraw`, `resolve`), driven by an in-memory forge
//! ([`mock::MockProvider`]) rather than a live GitHub. Each test is a
//! scenario over throwaway tempdirs: subscribe a domain, move the mock forge
//! or edit the working tree, run the operation under test and assert what
//! landed on disk, in the origin state and (for `propose`) in the calls the
//! mock recorded.
//!
//! The mock is a faithful stand-in for a forge, read and write sides both: a
//! fake commit graph with parent links, per-branch ETags that bump on every
//! branch move, a compare computed from two commit snapshots, blobs
//! addressed by content hash, tarballs wrapped in the single top-level
//! directory GitHub uses, a settable proposal registry and a working
//! create-blob/tree/commit/branch/proposal path that produces genuine new
//! commits a later `pull` can merge in. It never reaches the network and
//! never panics on an injected fault (a garbage-collected base commit, a
//! forced truncation).

mod mock;

use std::collections::BTreeMap;
use std::path::Path;

use crystalline_remote::ops::{
    PlannedAction, ProposeOutcome, PullReport, Resolution, ShareOptions, SubscribeReport, propose,
    propose_preview, pull, resolve, status, subscribe, withdraw,
};
use crystalline_remote::provider::{
    Feedback, OriginSpec, ProposalRequest, ProposalState, Provider,
};
use crystalline_remote::state::{
    BaseStamp, FeedbackItem, FeedbackKind, OriginState, Proposal, ProposalStatus, ProposedChange,
    ProposedFile, read_conflict_files,
};

use mock::{MockProvider, sha256_hex};

/// The origin every scenario tracks: one repository, the whole repository as
/// the domain (no subpath) and a `main` branch.
fn spec() -> OriginSpec {
    OriginSpec {
        repo: "team/knowledge".to_string(),
        subpath: None,
        branch: "main".to_string(),
    }
}

/// Builds a repo-relative path -> content map from string/bytes pairs.
fn commit_files(pairs: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(p, c)| (p.to_string(), c.to_vec()))
        .collect()
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// Subscribes a fresh domain at `commit`, returning the working-tree root, the
/// state directory (both kept alive by the returned tempdirs) and the report.
struct Subscribed {
    _work: tempfile::TempDir,
    _state: tempfile::TempDir,
    domain_root: std::path::PathBuf,
    state_dir: std::path::PathBuf,
}

async fn subscribe_at(mock: &MockProvider, commit: &str) -> (Subscribed, SubscribeReport) {
    mock.set_branch("main", commit);
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let domain_root = work.path().join("domain");
    let state_dir = state.path().join("origin");
    let report = subscribe(mock, &spec(), &domain_root, &state_dir)
        .await
        .expect("subscribe should succeed");
    (
        Subscribed {
            _work: work,
            _state: state,
            domain_root,
            state_dir,
        },
        report,
    )
}

fn load_state(state_dir: &Path) -> OriginState {
    OriginState::load(state_dir).unwrap().unwrap()
}

/// Whether a recorded mock call mutated the forge: the creating and updating
/// calls plus the two cleanup calls a supersede performs. Every read-only
/// operation (a probe, a compare, a blob or tarball fetch, a state or feedback
/// read, an open-proposal listing) is excluded, so a filter over this counts
/// exactly the writes a preview must never make.
fn is_write_call(call: &str) -> bool {
    call.starts_with("create_")
        || call.starts_with("update_")
        || call.starts_with("delete_branch")
        || call.starts_with("close_proposal")
}

/// Subscribes a fresh domain at `commit` against `spec`, with the working
/// tree rooted at a directory named `domain_name` (rather than the fixed
/// `"domain"` name [`subscribe_at`] uses), for share-side tests that need a
/// distinctively named working tree or a subpath spec. The domain's display
/// name for `propose`'s branch slug and generated title and body is a
/// separate argument passed straight to `propose`, not derived from this
/// basename.
async fn subscribe_named(
    mock: &MockProvider,
    spec: &OriginSpec,
    commit: &str,
    domain_name: &str,
) -> Subscribed {
    mock.set_branch(&spec.branch, commit);
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let domain_root = work.path().join(domain_name);
    let state_dir = state.path().join("origin");
    subscribe(mock, spec, &domain_root, &state_dir)
        .await
        .expect("subscribe should succeed");
    Subscribed {
        _work: work,
        _state: state,
        domain_root,
        state_dir,
    }
}

/// Subscribe a fresh single-file domain, make one local edit and share it,
/// returning the subscription and the created proposal's report. The starting
/// point for every living-proposal scenario: one open proposal, one commit on
/// its branch, one recorded head.
async fn shared_once(mock: &MockProvider) -> (Subscribed, crystalline_remote::ops::ProposeReport) {
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(mock, &c1).await;
    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");
    let outcome = propose(
        mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };
    (sub, report)
}

/// Overwrites the saved base commit, the corruption scenario 11 needs to force
/// the compare-404 re-baseline path.
fn set_base_commit(state_dir: &Path, commit: &str) {
    let mut st = load_state(state_dir);
    st.base_commit = commit.to_string();
    st.save(state_dir).unwrap();
}

/// Seeds an open, single-file proposal into the saved state so a later pull can
/// reconcile it once the mock marks the pull request merged or declined.
fn seed_proposal(state_dir: &Path, number: u64, path: &str, sha256: Option<String>) {
    let mut st = load_state(state_dir);
    st.proposals.push(Proposal {
        number,
        url: format!("https://example.test/pull/{number}"),
        branch: format!("crystalline/share-{number}"),
        title: format!("Share proposal {number}"),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Open,
        files: vec![ProposedFile {
            path: path.to_string(),
            change: ProposedChange::Added,
            sha256,
            blob_sha: None,
            size: None,
        }],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
    });
    st.save(state_dir).unwrap();
}

// Scenario 1: subscribe lays down the working tree, the base snapshot and the
// origin state; a missing MANIFEST is refused without touching the target; a
// non-empty target is adopted in place, keeping every local file.

#[tokio::test]
async fn scenario_01_subscribe_writes_tree_base_and_state() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha"),
            ("assets/logo.png", b"PNGDATA"),
        ]),
        None,
    );
    let (sub, report) = subscribe_at(&mock, &c1).await;

    assert_eq!(report.base_commit, c1);
    assert_eq!(report.files_written, 3);
    assert_eq!(report.engrams, 2, "two .md files, the png is not an engram");
    assert!(report.skipped_large.is_empty());

    // Working tree.
    assert_eq!(read(&sub.domain_root.join("MANIFEST.md")), b"# Manifest");
    assert_eq!(read(&sub.domain_root.join("notes/a.md")), b"alpha");
    assert_eq!(read(&sub.domain_root.join("assets/logo.png")), b"PNGDATA");

    // Base snapshot mirrors the working tree.
    assert_eq!(
        crystalline_remote::state::read_base_file(&sub.state_dir, "notes/a.md").unwrap(),
        Some(b"alpha".to_vec())
    );

    // Origin state.
    let st = OriginState::load(&sub.state_dir).unwrap().unwrap();
    assert_eq!(st.base_commit, c1);
    assert_eq!(st.repo, "team/knowledge");
    assert_eq!(st.branch, "main");
    assert_eq!(st.files.len(), 3);
    assert_eq!(
        st.files.get("notes/a.md"),
        Some(&BaseStamp {
            sha256: sha256_hex(b"alpha"),
            size: 5
        })
    );
    assert!(st.ref_etag.is_some());
}

#[tokio::test]
async fn scenario_01_subscribe_without_manifest_is_not_a_domain_and_writes_nothing() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(commit_files(&[("notes/a.md", b"alpha")]), None);
    mock.set_branch("main", &c1);

    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let domain_root = work.path().join("domain");
    let state_dir = state.path().join("origin");

    let err = subscribe(&mock, &spec(), &domain_root, &state_dir)
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_remote::RemoteError::NotADomain { .. }),
        "{err:?}"
    );
    assert!(!domain_root.exists(), "target must be untouched");
    assert!(OriginState::load(&state_dir).unwrap().is_none());
}

#[tokio::test]
async fn scenario_01_subscribe_into_a_non_empty_directory_adopts_in_place() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha"),
            ("notes/team.md", b"team version"),
        ]),
        None,
    );
    mock.set_branch("main", &c1);

    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let domain_root = work.path().join("domain");
    // Pre-existing local knowledge: one file identical upstream, one whose
    // content differs and one upstream does not know about.
    write(&domain_root.join("MANIFEST.md"), b"# Manifest");
    write(&domain_root.join("notes/team.md"), b"local version");
    write(&domain_root.join("notes/local-only.md"), b"mine");
    let state_dir = state.path().join("origin");

    let report = subscribe(&mock, &spec(), &domain_root, &state_dir)
        .await
        .expect("a non-empty target is connected in place");

    assert!(report.adopted);
    assert_eq!(report.base_commit, c1);
    assert_eq!(
        report.files_written, 1,
        "only the upstream file with no local counterpart is materialized"
    );
    assert_eq!(
        report.local_changes, 2,
        "the differing file and the local-only file"
    );

    // Local content is never overwritten; the missing upstream file arrives.
    assert_eq!(read(&domain_root.join("notes/team.md")), b"local version");
    assert_eq!(read(&domain_root.join("notes/local-only.md")), b"mine");
    assert_eq!(read(&domain_root.join("notes/a.md")), b"alpha");

    // The base snapshot records upstream's side, so the kept local file is an
    // ordinary Modified change and the local-only file an Added one, exactly
    // what status, share and pull already understand.
    assert_eq!(
        crystalline_remote::state::read_base_file(&state_dir, "notes/team.md").unwrap(),
        Some(b"team version".to_vec())
    );
    let st = OriginState::load(&state_dir).unwrap().unwrap();
    assert_eq!(st.base_commit, c1);
    assert_eq!(st.files.len(), 3, "the base manifest is upstream's tree");
    let local = crystalline_remote::changes::detect_local_changes(&domain_root, &st.files).unwrap();
    let mut classified: Vec<(&str, &str)> = local
        .changes
        .iter()
        .map(|c| {
            let kind = match c {
                crystalline_remote::changes::LocalChange::Added { .. } => "added",
                crystalline_remote::changes::LocalChange::Modified { .. } => "modified",
                crystalline_remote::changes::LocalChange::Deleted { .. } => "deleted",
            };
            (kind, c.path())
        })
        .collect();
    classified.sort();
    assert_eq!(
        classified,
        vec![
            ("added", "notes/local-only.md"),
            ("modified", "notes/team.md"),
        ]
    );
}

// Scenario 2: a pull with no upstream movement reports up to date and writes
// nothing.

#[tokio::test]
async fn scenario_02_pull_with_no_movement_is_up_to_date() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert_eq!(
        report,
        PullReport {
            up_to_date: true,
            applied: vec![],
            merged: vec![],
            conflicts: vec![],
            proposals: vec![],
            skipped_large: vec![],
            re_baselined: false,
        }
    );
    // Working tree unchanged.
    assert_eq!(read(&sub.domain_root.join("notes/a.md")), b"alpha");
}

// Scenario 3: upstream edits a file the working tree never touched. The edit
// applies cleanly, the working tree matches upstream and the base advances.

#[tokio::test]
async fn scenario_03_upstream_edit_of_untouched_file_applies() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha revised upstream\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(!report.up_to_date);
    assert_eq!(report.applied, vec!["notes/a.md".to_string()]);
    assert!(report.merged.is_empty(), "a plain take is not a merge");
    assert!(report.conflicts.is_empty());

    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"alpha revised upstream\n"
    );

    let st = OriginState::load(&sub.state_dir).unwrap().unwrap();
    assert_eq!(st.base_commit, c2);
    assert_eq!(
        crystalline_remote::state::read_base_file(&sub.state_dir, "notes/a.md").unwrap(),
        Some(b"alpha revised upstream\n".to_vec())
    );
}

// Scenario 4: disjoint edits merge cleanly. A file only the working tree
// touched is left alone, a file only upstream touched is taken plainly and a
// file both sides touched in different regions is three-way merged; only the
// last counts as "merged".

#[tokio::test]
async fn scenario_04_disjoint_edits_merge_cleanly() {
    let base_c = b"# C\n\nSection A: base\n\nSection B: base\n";
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"A base\n"),
            ("notes/b.md", b"B base\n"),
            ("notes/c.md", base_c),
        ]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // Local edits: file A (upstream leaves it alone) and file C section A.
    write(&sub.domain_root.join("notes/a.md"), b"A local\n");
    let local_c = b"# C\n\nSection A: LOCAL\n\nSection B: base\n";
    write(&sub.domain_root.join("notes/c.md"), local_c);

    // Upstream edits: file B and file C section B; file A unchanged upstream.
    let upstream_c = b"# C\n\nSection A: base\n\nSection B: UPSTREAM\n";
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"A base\n"),
            ("notes/b.md", b"B upstream\n"),
            ("notes/c.md", upstream_c),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(!report.up_to_date);
    assert_eq!(
        report.applied,
        vec!["notes/b.md".to_string(), "notes/c.md".to_string()]
    );
    assert_eq!(report.merged, vec!["notes/c.md".to_string()]);
    assert!(report.conflicts.is_empty());

    // File A keeps the local edit, B takes upstream, C carries both edits.
    assert_eq!(read(&sub.domain_root.join("notes/a.md")), b"A local\n");
    assert_eq!(read(&sub.domain_root.join("notes/b.md")), b"B upstream\n");
    assert_eq!(
        read(&sub.domain_root.join("notes/c.md")),
        b"# C\n\nSection A: LOCAL\n\nSection B: UPSTREAM\n"
    );
}

// Scenario 5: a same-line conflict leaves the local file byte-identical,
// records the conflict with readable copies, still advances the base to head
// and does not duplicate the conflict on a second, movement-free pull.

#[tokio::test]
async fn scenario_05_same_line_conflict_records_and_advances_base() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one\n"),
        ]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    write(&sub.domain_root.join("notes/a.md"), b"line one LOCAL\n");

    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one UPSTREAM\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(report.applied.is_empty());
    assert_eq!(report.conflicts.len(), 1);
    let conflict = &report.conflicts[0];
    assert_eq!(conflict.path, "notes/a.md");
    assert_eq!(
        conflict.kind,
        crystalline_remote::merge::ConflictKind::EditEdit
    );
    assert_eq!(conflict.base_commit, c1);
    assert_eq!(conflict.upstream_commit, c2);

    // Local file untouched.
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"line one LOCAL\n"
    );

    // Conflict copies readable, both sides preserved.
    let (base_copy, upstream_copy) = read_conflict_files(&sub.state_dir, &conflict.id).unwrap();
    assert_eq!(base_copy, Some(b"line one\n".to_vec()));
    assert_eq!(upstream_copy, Some(b"line one UPSTREAM\n".to_vec()));

    // Base still advanced to head, conflicted path included.
    let st = load_state(&sub.state_dir);
    assert_eq!(st.base_commit, c2);
    assert_eq!(
        crystalline_remote::state::read_base_file(&sub.state_dir, "notes/a.md").unwrap(),
        Some(b"line one UPSTREAM\n".to_vec())
    );

    // A second pull with no upstream movement records no duplicate conflict.
    let report2 = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert!(report2.up_to_date);
    assert!(report2.conflicts.is_empty());
    assert_eq!(load_state(&sub.state_dir).conflicts.len(), 1);
}

// Scenario 6: upstream deletes a file the working tree edited. The result is an
// edit/delete conflict with the local file left intact.

#[tokio::test]
async fn scenario_06_upstream_delete_of_locally_edited_file_conflicts() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"content\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    write(&sub.domain_root.join("notes/a.md"), b"locally edited\n");

    let c2 = mock.add_commit(commit_files(&[("MANIFEST.md", b"# Manifest")]), Some(&c1));
    mock.set_branch("main", &c2);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(
        report.conflicts[0].kind,
        crystalline_remote::merge::ConflictKind::EditDelete
    );
    // Local file intact.
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"locally edited\n"
    );

    // Base advanced: the path is gone from the base snapshot.
    let st = load_state(&sub.state_dir);
    assert_eq!(st.base_commit, c2);
    assert!(!st.files.contains_key("notes/a.md"));
    let (base_copy, upstream_copy) =
        read_conflict_files(&sub.state_dir, &report.conflicts[0].id).unwrap();
    assert_eq!(base_copy, Some(b"content\n".to_vec()));
    assert_eq!(upstream_copy, None);
}

// Scenario 7: a proposal merged verbatim. The local file already equals both
// the proposed hash and the merged upstream content, so the pull consumes the
// proposal without conflict, moves it to history and attempts a branch delete.

#[tokio::test]
async fn scenario_07_proposal_merged_verbatim_is_consumed() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/e.md", b"existing\n"),
        ]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // The shared content already lives in the working tree as a local addition.
    let shared = b"shared content\n";
    write(&sub.domain_root.join("notes/new.md"), shared);
    seed_proposal(&sub.state_dir, 1, "notes/new.md", Some(sha256_hex(shared)));

    // The merged pull request lands exactly the proposed content upstream.
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/e.md", b"existing\n"),
            ("notes/new.md", shared),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    mock.set_proposal_state(1, ProposalState::Merged);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(report.conflicts.is_empty());
    assert_eq!(report.proposals, vec![(1, ProposalStatus::Merged)]);

    let st = load_state(&sub.state_dir);
    assert!(st.proposals.is_empty(), "consumed out of the open list");
    assert_eq!(st.history.len(), 1);
    assert_eq!(st.history[0].number, 1);
    assert_eq!(st.history[0].status, ProposalStatus::Merged);

    assert!(
        mock.calls()
            .contains(&"delete_branch:crystalline/share-1".to_string()),
        "{:?}",
        mock.calls()
    );
    assert_eq!(read(&sub.domain_root.join("notes/new.md")), shared);
}

// Scenario 8: a reviewer amended the proposal before merging. The local file
// still equals the proposed hash, so the amended upstream content wins silently
// with no conflict.

#[tokio::test]
async fn scenario_08_reviewer_amended_proposal_takes_upstream() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let (sub, _) = subscribe_at(&mock, &c1).await;

    let proposed = b"proposed content\n";
    write(&sub.domain_root.join("notes/new.md"), proposed);
    seed_proposal(
        &sub.state_dir,
        1,
        "notes/new.md",
        Some(sha256_hex(proposed)),
    );

    // Upstream landed a reviewer-amended version, different from the proposal.
    let amended = b"amended by the reviewer\n";
    let c2 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/new.md", amended)]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    mock.set_proposal_state(1, ProposalState::Merged);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(report.conflicts.is_empty(), "amendment wins silently");
    assert_eq!(report.applied, vec!["notes/new.md".to_string()]);
    assert_eq!(read(&sub.domain_root.join("notes/new.md")), amended);

    let st = load_state(&sub.state_dir);
    assert!(st.proposals.is_empty());
    assert_eq!(st.history[0].status, ProposalStatus::Merged);
}

// Scenario 9: the user edited the shared file after opening the proposal. The
// local file no longer equals the proposed hash and upstream differs too, so
// the override does not fire and the merge conflicts.

#[tokio::test]
async fn scenario_09_edited_after_share_falls_through_to_conflict() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // The proposal recorded one hash, but the working tree has since diverged.
    let local_after_share = b"user edited after sharing\n";
    write(&sub.domain_root.join("notes/new.md"), local_after_share);
    seed_proposal(
        &sub.state_dir,
        1,
        "notes/new.md",
        Some(sha256_hex(b"originally proposed\n")),
    );

    let upstream = b"reviewer merged version\n";
    let c2 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/new.md", upstream)]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    mock.set_proposal_state(1, ProposalState::Merged);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(
        report.conflicts[0].kind,
        crystalline_remote::merge::ConflictKind::AddAdd
    );
    // Local file untouched by the conflict.
    assert_eq!(
        read(&sub.domain_root.join("notes/new.md")),
        local_after_share
    );
    // The proposal still merged upstream, so it is consumed to history.
    let st = load_state(&sub.state_dir);
    assert!(st.proposals.is_empty());
    assert_eq!(st.history[0].status, ProposalStatus::Merged);
}

// Scenario 10: a proposal is declined without the branch moving. The pull stays
// up to date, records the declined transition and keeps the proposal in the
// open list marked declined.

#[tokio::test]
async fn scenario_10_declined_proposal_without_movement() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    write(&sub.domain_root.join("notes/new.md"), b"was proposed\n");
    seed_proposal(
        &sub.state_dir,
        7,
        "notes/new.md",
        Some(sha256_hex(b"was proposed\n")),
    );
    mock.set_proposal_state(7, ProposalState::Declined);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(report.up_to_date);
    assert_eq!(report.proposals, vec![(7, ProposalStatus::Declined)]);

    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals.len(), 1, "declined stays in the open list");
    assert_eq!(st.proposals[0].status, ProposalStatus::Declined);
    assert!(st.history.is_empty());

    // Status surfaces it as a declined proposal.
    let status_report = status(&spec(), &sub.domain_root, &sub.state_dir, None, false)
        .await
        .unwrap();
    assert_eq!(status_report.declined_proposals.len(), 1);
    assert!(status_report.open_proposals.is_empty());
}

// Scenario 11: the base commit is gone upstream (history rewritten). The pull
// re-baselines onto head: upstream-only files materialize, a locally differing
// file is left untouched and later shows as a local change.

#[tokio::test]
async fn scenario_11_missing_base_commit_re_baselines() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"a v1\n"),
            ("notes/b.md", b"b v1\n"),
        ]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // The working tree has a local edit to a.md.
    write(&sub.domain_root.join("notes/a.md"), b"a LOCAL\n");

    // Head moves and carries an extra upstream-only file.
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"a v1\n"),
            ("notes/b.md", b"b v1\n"),
            ("notes/extra.md", b"extra upstream\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    // The recorded base commit is now an unknown id the mock 404s on compare.
    set_base_commit(&sub.state_dir, "ghost-commit");
    mock.gc_commit("ghost-commit");

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(report.re_baselined);
    assert_eq!(report.applied, vec!["notes/extra.md".to_string()]);

    // Upstream-only file materialized, locally differing file untouched.
    assert_eq!(
        read(&sub.domain_root.join("notes/extra.md")),
        b"extra upstream\n"
    );
    assert_eq!(read(&sub.domain_root.join("notes/a.md")), b"a LOCAL\n");

    let st = load_state(&sub.state_dir);
    assert_eq!(st.base_commit, c2);
    assert_eq!(
        crystalline_remote::state::read_base_file(&sub.state_dir, "notes/a.md").unwrap(),
        Some(b"a v1\n".to_vec()),
        "base re-baselined to the head content"
    );

    // Subsequent status reports a.md as a local change against the new base.
    let status_report = status(&spec(), &sub.domain_root, &sub.state_dir, None, false)
        .await
        .unwrap();
    assert_eq!(status_report.local_changes, 1);
}

// Scenario 12: an oversized upstream file is skipped with a warning, never
// written and never recorded in the base manifest.

#[tokio::test]
async fn scenario_12_oversized_upstream_file_is_skipped() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    let oversized = vec![b'x'; (crystalline_remote::changes::MAX_SHARED_FILE_BYTES + 1) as usize];
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha"),
            ("notes/huge.md", &oversized),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert_eq!(
        report.skipped_large,
        vec![("notes/huge.md".to_string(), oversized.len() as u64)]
    );
    assert!(report.applied.is_empty());
    assert!(!sub.domain_root.join("notes/huge.md").exists());

    let st = load_state(&sub.state_dir);
    assert!(!st.files.contains_key("notes/huge.md"));
    assert_eq!(st.base_commit, c2);
}

// Scenario 13: status works offline (behind is None) and, with a provider,
// reports whether the branch has moved ahead of the base.

#[tokio::test]
async fn scenario_13_status_offline_and_online() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // Offline: no probe, behind is unknown.
    let offline = status(&spec(), &sub.domain_root, &sub.state_dir, None, false)
        .await
        .unwrap();
    assert_eq!(offline.behind, None);
    assert_eq!(offline.repo, "team/knowledge");
    assert_eq!(offline.branch, "main");
    assert_eq!(offline.base_commit, c1);

    // Online, branch unmoved: not behind.
    let online_unmoved = status(
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(&mock),
        false,
    )
    .await
    .unwrap();
    assert_eq!(online_unmoved.behind, Some(false));

    // Move the branch, then probe again: now behind.
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha revised\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let online_moved = status(
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(&mock),
        false,
    )
    .await
    .unwrap();
    assert_eq!(online_moved.behind, Some(true));

    // A status probe that found the branch moved must not poison the stored
    // etag marker: a following pull still integrates the upstream change
    // rather than seeing a stale "unchanged" and skipping it.
    let after = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert!(!after.up_to_date);
    assert_eq!(after.applied, vec!["notes/a.md".to_string()]);
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"alpha revised\n"
    );
}

// Scenario 14: when compare reports truncation, the pull falls back to a
// whole-tree tarball diff and produces the same add/modify/remove change set.

#[tokio::test]
async fn scenario_14_truncated_compare_falls_back_to_tarball_diff() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"a1\n"),
            ("notes/b.md", b"b1\n"),
        ]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // Upstream modifies a.md, adds c.md and removes b.md.
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"a2\n"),
            ("notes/c.md", b"c new\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    // Force the tarball-diff fallback path.
    mock.set_truncate(true);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(!report.up_to_date);
    let mut applied = report.applied.clone();
    applied.sort();
    assert_eq!(
        applied,
        vec![
            "notes/a.md".to_string(),
            "notes/b.md".to_string(),
            "notes/c.md".to_string()
        ]
    );
    assert!(report.merged.is_empty());

    assert_eq!(read(&sub.domain_root.join("notes/a.md")), b"a2\n");
    assert_eq!(read(&sub.domain_root.join("notes/c.md")), b"c new\n");
    assert!(!sub.domain_root.join("notes/b.md").exists());

    let st = load_state(&sub.state_dir);
    assert_eq!(st.base_commit, c2);
    assert!(!st.files.contains_key("notes/b.md"));
    assert_eq!(
        st.files.get("notes/a.md").unwrap().sha256,
        sha256_hex(b"a2\n")
    );
}

// --- share-side: propose, withdraw, resolve -----------------------------------

/// The origin every share-side scenario tracks, rooted at a `knowledge/`
/// subpath so tree writes exercise contract 3's repo-relative prefixing.
fn share_spec() -> OriginSpec {
    OriginSpec {
        repo: "team/knowledge".to_string(),
        subpath: Some("knowledge".to_string()),
        branch: "main".to_string(),
    }
}

fn sub_commit_files(pairs: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(p, c)| (format!("knowledge/{p}"), c.to_vec()))
        .collect()
}

// Scenario 15 (a): propose happy path. Edit, add and delete locally, then
// propose: two blobs uploaded, a tree with three writes at repo-relative
// paths (the "knowledge/" subpath prefixed back on), the deletion carried as
// a `blob_sha: None` write, the commit parented on the base commit, the
// branch name matching the slug contract for a domain name needing
// sanitization, the PR opened against the tracked branch, the Proposal
// recorded with domain-relative paths and hashes, and the local files left
// exactly as they are.

#[tokio::test]
async fn scenario_15_propose_happy_path_creates_pr_and_records_proposal() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/keep.md", b"keep\n"),
            ("notes/edit.md", b"before\n"),
            ("notes/gone.md", b"bye\n"),
        ]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "Brand Team").await;

    write(&sub.domain_root.join("notes/edit.md"), b"after\n");
    write(&sub.domain_root.join("notes/added.md"), b"brand new\n");
    std::fs::remove_file(sub.domain_root.join("notes/gone.md")).unwrap();

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "Brand Team",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };

    // Branch slug: "Brand Team" lowercased, the space replaced with '-'.
    assert!(
        report.branch.starts_with("crystalline/share-brand-team-"),
        "{}",
        report.branch
    );
    assert_eq!(report.number, 1);
    assert_eq!(report.url, "https://github.test/pulls/1");
    assert_eq!(report.added, vec!["notes/added.md".to_string()]);
    assert_eq!(report.updated, vec!["notes/edit.md".to_string()]);
    assert_eq!(report.deleted, vec!["notes/gone.md".to_string()]);
    assert!(report.skipped_large.is_empty());

    // Two blobs uploaded, for the edited and the added file's content.
    let calls = mock.calls();
    assert!(
        calls.contains(&format!("create_blob:{}", sha256_hex(b"after\n"))),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("create_blob:{}", sha256_hex(b"brand new\n"))),
        "{calls:?}"
    );

    // The PR request targets the tracked branch and carries the created
    // branch name.
    let req = mock.proposal_request(1).unwrap();
    assert_eq!(req.branch, report.branch);
    assert_eq!(req.base_branch, "main");

    // The tree carries repo-relative paths: the "knowledge/" subpath is
    // prefixed back onto every write, the deletion is gone and an untouched
    // file carried over unchanged from the parent tree (proving the tree was
    // built on top of the base commit, not from scratch).
    let branch_commit = mock.branch_commit(&report.branch).unwrap();
    let tree = mock.commit_tree(&branch_commit).unwrap();
    assert_eq!(
        tree.get("knowledge/notes/edit.md"),
        Some(&b"after\n".to_vec())
    );
    assert_eq!(
        tree.get("knowledge/notes/added.md"),
        Some(&b"brand new\n".to_vec())
    );
    assert!(!tree.contains_key("knowledge/notes/gone.md"));
    assert_eq!(
        tree.get("knowledge/notes/keep.md"),
        Some(&b"keep\n".to_vec()),
        "an untouched file must carry over from the base commit's tree"
    );

    // State records the Proposal with domain-relative paths and hashes.
    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals.len(), 1);
    let recorded = &st.proposals[0];
    assert_eq!(recorded.number, 1);
    assert_eq!(recorded.status, ProposalStatus::Open);
    let mut files = recorded.files.clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        files,
        vec![
            ProposedFile {
                path: "notes/added.md".to_string(),
                change: ProposedChange::Added,
                sha256: Some(sha256_hex(b"brand new\n")),
                // The mock provider hands back the content's own sha256 as
                // its blob sha, so the two match here by construction.
                blob_sha: Some(sha256_hex(b"brand new\n")),
                size: Some(b"brand new\n".len() as u64),
            },
            ProposedFile {
                path: "notes/edit.md".to_string(),
                change: ProposedChange::Modified,
                sha256: Some(sha256_hex(b"after\n")),
                blob_sha: Some(sha256_hex(b"after\n")),
                size: Some(b"after\n".len() as u64),
            },
            ProposedFile {
                path: "notes/gone.md".to_string(),
                change: ProposedChange::Deleted,
                sha256: None,
                blob_sha: None,
                size: None,
            },
        ]
    );

    // Local files are left exactly as they are.
    assert_eq!(read(&sub.domain_root.join("notes/edit.md")), b"after\n");
    assert_eq!(
        read(&sub.domain_root.join("notes/added.md")),
        b"brand new\n"
    );
    assert!(!sub.domain_root.join("notes/gone.md").exists());
}

// Scenario 16 (b): conflicts pending refuses the share outright, before any
// provider write call.

#[tokio::test]
async fn scenario_16_propose_with_conflicts_pending_refuses_without_provider_writes() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one\n"),
        ]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    // A same-line conflict from a previous pull.
    write(&sub.domain_root.join("notes/a.md"), b"line one LOCAL\n");
    let c2 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one UPSTREAM\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    pull(&mock, &spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert_eq!(load_state(&sub.state_dir).conflicts.len(), 1);

    // Share another, unrelated local change; the outstanding conflict alone
    // must refuse the share.
    write(&sub.domain_root.join("notes/new.md"), b"brand new\n");

    let err = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap_err();
    match err {
        crystalline_remote::RemoteError::ConflictsPending { count } => assert_eq!(count, 1),
        other => panic!("expected ConflictsPending, got {other:?}"),
    }

    // No write call was ever logged: the refusal happens before any blob,
    // tree, commit, branch or proposal is created.
    let calls = mock.calls();
    assert!(!calls.iter().any(|c| c.starts_with("create_")), "{calls:?}");
}

// Scenario 18 (d): nothing to share when the working tree already matches
// the base exactly.

#[tokio::test]
async fn scenario_18_propose_with_no_local_changes_is_nothing_to_share() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    match outcome {
        ProposeOutcome::NothingToShare { skipped_large } => assert!(skipped_large.is_empty()),
        other => panic!("expected NothingToShare, got {other:?}"),
    }

    let calls = mock.calls();
    assert!(!calls.iter().any(|c| c.starts_with("create_")), "{calls:?}");
    assert!(load_state(&sub.state_dir).proposals.is_empty());
}

// Scenario 17 (c): freshness. Upstream moved with a mergeable edit; propose
// pulls it in first, then builds its commit on the new base.

#[tokio::test]
async fn scenario_17_propose_freshness_pulls_first_then_proposes_on_new_base() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"a v1\n")]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    // A local addition to share.
    write(&sub.domain_root.join("notes/local.md"), b"brand new\n");

    // Upstream moves with a plain, mergeable edit to a file the working tree
    // never touched.
    let c2 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"a v2 upstream\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };

    // The inline pull applied the upstream edit before proposing.
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"a v2 upstream\n"
    );
    assert_eq!(load_state(&sub.state_dir).base_commit, c2);

    // The commit is parented on the new base c2, not the stale c1: its tree
    // carries both the upstream edit to a.md and the newly proposed file.
    let branch_commit = mock.branch_commit(&report.branch).unwrap();
    let tree = mock.commit_tree(&branch_commit).unwrap();
    assert_eq!(
        tree.get("knowledge/notes/a.md"),
        Some(&b"a v2 upstream\n".to_vec())
    );
    assert_eq!(
        tree.get("knowledge/notes/local.md"),
        Some(&b"brand new\n".to_vec())
    );
}

// Scenario 19 (e): full circle. The mock merges the proposed branch into
// main verbatim; a later pull consumes the proposal to history as Merged
// with no conflicts, through real propose output rather than seeded state.

#[tokio::test]
async fn scenario_19_propose_full_circle_merged_verbatim_is_consumed_by_pull() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(sub_commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    write(&sub.domain_root.join("notes/new.md"), b"shared content\n");

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };

    // The mock "merges" the proposed branch into main verbatim: a
    // fast-forward onto exactly the commit propose created.
    let branch_commit = mock.branch_commit(&report.branch).unwrap();
    mock.set_branch("main", &branch_commit);
    mock.set_proposal_state(report.number, ProposalState::Merged);

    let pull_report = pull(&mock, &spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert!(pull_report.conflicts.is_empty());
    assert_eq!(
        pull_report.proposals,
        vec![(report.number, ProposalStatus::Merged)]
    );

    let st = load_state(&sub.state_dir);
    assert!(st.proposals.is_empty());
    assert_eq!(st.history.len(), 1);
    assert_eq!(st.history[0].number, report.number);
    assert_eq!(st.history[0].status, ProposalStatus::Merged);
    assert_eq!(
        read(&sub.domain_root.join("notes/new.md")),
        b"shared content\n"
    );
}

// Scenario 20 (f): amended circle. The mock merges an amended version of the
// proposal; the pull's override path applies since the local hash still
// matches what was proposed, so the amendment wins silently.

#[tokio::test]
async fn scenario_20_propose_amended_merge_upstream_wins_silently() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(sub_commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    write(&sub.domain_root.join("notes/new.md"), b"proposed content\n");

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };

    // A reviewer amends the content before merging: a new commit parented on
    // c1 (not the proposed branch commit) landing different bytes at the
    // same path.
    let amended = b"amended by the reviewer\n";
    let c2 = mock.add_commit(
        sub_commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/new.md", amended)]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    mock.set_proposal_state(report.number, ProposalState::Merged);

    let pull_report = pull(&mock, &spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert!(pull_report.conflicts.is_empty(), "amendment wins silently");
    assert_eq!(read(&sub.domain_root.join("notes/new.md")), amended);

    let st = load_state(&sub.state_dir);
    assert!(st.proposals.is_empty());
    assert_eq!(st.history[0].status, ProposalStatus::Merged);
}

// Scenario 20: withdraw. Closing an open proposal on the forge, the optional
// revert, the declined and merged cases, a close failure and the targeting
// rules.

#[tokio::test]
async fn scenario_20_withdraw_open_closes_the_pr_and_keeps_files() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;

    let report = withdraw(
        &mock,
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(report.number, first.number);
    assert!(report.closed);
    assert!(report.restored.is_empty() && report.deleted.is_empty());

    let calls = mock.calls();
    assert!(
        calls.contains(&format!("close_proposal:{}", first.number)),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("delete_branch:{}", first.branch)),
        "{calls:?}"
    );

    let st = load_state(&sub.state_dir);
    assert!(st.proposals.is_empty());
    assert_eq!(st.history[0].number, first.number);
    assert_eq!(st.history[0].status, ProposalStatus::Withdrawn);
    // The local edit survives: withdraw without revert never touches files.
    assert_eq!(read(&sub.domain_root.join("notes/a.md")), b"alpha v2\n");
}

#[tokio::test]
async fn scenario_20_withdraw_revert_restores_undiverged_files() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/edit.md", b"before\n"),
            ("notes/gone.md", b"bye\n"),
        ]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;
    write(&sub.domain_root.join("notes/edit.md"), b"after\n");
    write(&sub.domain_root.join("notes/added.md"), b"brand new\n");
    std::fs::remove_file(sub.domain_root.join("notes/gone.md")).unwrap();
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("{other:?}"),
    };
    // One file diverges after sharing: it must be left alone.
    write(
        &sub.domain_root.join("notes/added.md"),
        b"kept working on it\n",
    );

    let w = withdraw(
        &mock,
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(report.number),
        true,
        false,
    )
    .await
    .unwrap();
    let mut restored = w.restored.clone();
    restored.sort();
    assert_eq!(
        restored,
        vec!["notes/edit.md".to_string(), "notes/gone.md".to_string()]
    );
    assert!(w.deleted.is_empty());
    assert_eq!(w.skipped_diverged, vec!["notes/added.md".to_string()]);
    assert_eq!(read(&sub.domain_root.join("notes/edit.md")), b"before\n");
    assert_eq!(read(&sub.domain_root.join("notes/gone.md")), b"bye\n");
    assert_eq!(
        read(&sub.domain_root.join("notes/added.md")),
        b"kept working on it\n"
    );
}

#[tokio::test]
async fn scenario_20_withdraw_declined_skips_the_close() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    mock.set_proposal_state(first.number, ProposalState::Declined);
    pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    let report = withdraw(
        &mock,
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(first.number),
        false,
        false,
    )
    .await
    .unwrap();
    assert!(!report.closed);
    let calls = mock.calls();
    assert!(
        !calls.contains(&format!("close_proposal:{}", first.number)),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("delete_branch:{}", first.branch)),
        "{calls:?}"
    );
    let st = load_state(&sub.state_dir);
    assert_eq!(st.history[0].status, ProposalStatus::Withdrawn);
}

#[tokio::test]
async fn scenario_20_withdraw_refuses_a_merged_proposal() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    // A status flip can leave a Merged record standing in `proposals` until
    // the next pull consumes it; withdrawing one is refused outright.
    let mut st = load_state(&sub.state_dir);
    st.proposals[0].status = ProposalStatus::Merged;
    st.save(&sub.state_dir).unwrap();

    let err = withdraw(
        &mock,
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(first.number),
        false,
        false,
    )
    .await
    .unwrap_err();
    match err {
        crystalline_remote::RemoteError::State(msg) => {
            assert!(msg.contains("already merged"), "{msg}");
        }
        other => panic!("expected State, got {other:?}"),
    }
    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals.len(), 1, "the record is untouched");
    assert!(st.history.is_empty());
}

#[tokio::test]
async fn scenario_20_withdraw_close_failure_aborts_untouched() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    mock.fail_close_proposal(first.number);

    let err = withdraw(
        &mock,
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        None,
        true,
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            crystalline_remote::RemoteError::Api { status: 500, .. }
        ),
        "{err:?}"
    );
    let st = load_state(&sub.state_dir);
    assert_eq!(
        st.proposals[0].status,
        ProposalStatus::Open,
        "record untouched"
    );
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"alpha v2\n",
        "no revert happened"
    );
}

#[tokio::test]
async fn scenario_20_withdraw_targeting_names_candidates() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // No open proposal, no number: the error lists candidates (none).
    let err = withdraw(
        &mock,
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    match err {
        crystalline_remote::RemoteError::NoWithdrawTarget { open, declined } => {
            assert!(open.is_empty() && declined.is_empty());
        }
        other => panic!("expected NoWithdrawTarget, got {other:?}"),
    }

    // An unknown number stays ProposalNotFound.
    let err = withdraw(
        &mock,
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(99),
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            crystalline_remote::RemoteError::ProposalNotFound { number: 99 }
        ),
        "{err:?}"
    );
}

// Scenario 21 (g): withdraw --revert. A declined proposal touching three
// files: one verbatim (restored to its base content), one diverged since
// sharing (skipped, untouched) and one proposed addition (deleted). The
// record lands in history as withdrawn.

#[tokio::test]
async fn scenario_21_withdraw_restores_verbatim_deletes_added_skips_diverged() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/keep.md", b"base keep\n"),
            ("notes/diverge.md", b"base diverge\n"),
        ]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    // What a previous share proposed, without going through a real propose
    // call: two modifications and one addition.
    write(&sub.domain_root.join("notes/keep.md"), b"shared keep v2\n");
    write(
        &sub.domain_root.join("notes/diverge.md"),
        b"shared diverge v2\n",
    );
    write(&sub.domain_root.join("notes/added.md"), b"newly added\n");

    let mut state = load_state(&sub.state_dir);
    state.proposals.push(Proposal {
        number: 9,
        url: "https://github.test/pulls/9".to_string(),
        branch: "crystalline/share-brand-000101000000".to_string(),
        title: "Share updates from brand".to_string(),
        created_at: chrono::Utc::now(),
        status: ProposalStatus::Declined,
        files: vec![
            ProposedFile {
                path: "notes/keep.md".to_string(),
                change: ProposedChange::Modified,
                sha256: Some(sha256_hex(b"shared keep v2\n")),
                blob_sha: None,
                size: None,
            },
            ProposedFile {
                path: "notes/diverge.md".to_string(),
                change: ProposedChange::Modified,
                sha256: Some(sha256_hex(b"shared diverge v2\n")),
                blob_sha: None,
                size: None,
            },
            ProposedFile {
                path: "notes/added.md".to_string(),
                change: ProposedChange::Added,
                sha256: Some(sha256_hex(b"newly added\n")),
                blob_sha: None,
                size: None,
            },
        ],
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
    });
    state.save(&sub.state_dir).unwrap();

    // The user edits notes/diverge.md again after sharing it.
    write(
        &sub.domain_root.join("notes/diverge.md"),
        b"further edited after sharing\n",
    );

    let report = withdraw(
        &mock,
        &spec,
        &sub.domain_root,
        &sub.state_dir,
        Some(9),
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(report.number, 9);
    assert!(!report.closed, "a declined proposal is already closed");
    assert_eq!(report.restored, vec!["notes/keep.md".to_string()]);
    assert_eq!(report.deleted, vec!["notes/added.md".to_string()]);
    assert_eq!(
        report.skipped_diverged,
        vec!["notes/diverge.md".to_string()]
    );

    assert_eq!(read(&sub.domain_root.join("notes/keep.md")), b"base keep\n");
    assert!(!sub.domain_root.join("notes/added.md").exists());
    assert_eq!(
        read(&sub.domain_root.join("notes/diverge.md")),
        b"further edited after sharing\n",
        "a diverged file must be left untouched"
    );

    let st = load_state(&sub.state_dir);
    assert!(st.proposals.is_empty());
    assert_eq!(st.history.len(), 1);
    assert_eq!(st.history[0].number, 9);
    assert_eq!(st.history[0].status, ProposalStatus::Withdrawn);
}

// Scenario 22 (h): resolve. Mine, theirs (both EditEdit and the
// EditDelete-theirs-means-delete case) and a caller-supplied merge, plus the
// remaining count and the unknown-path error listing open conflicts.

/// Subscribes a fresh domain, then drives a real `EditEdit` conflict at
/// `notes/a.md` through an actual pull: base "line one", local "line one
/// LOCAL", upstream "line one UPSTREAM".
async fn seeded_edit_edit_conflict(mock: &MockProvider, spec: &OriginSpec) -> Subscribed {
    let c1 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one\n"),
        ]),
        None,
    );
    let sub = subscribe_named(mock, spec, &c1, "brand").await;
    write(&sub.domain_root.join("notes/a.md"), b"line one LOCAL\n");
    let c2 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one UPSTREAM\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    pull(mock, spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert_eq!(load_state(&sub.state_dir).conflicts.len(), 1);
    sub
}

#[tokio::test]
async fn scenario_22_resolve_mine_keeps_local_content_untouched() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let sub = seeded_edit_edit_conflict(&mock, &spec).await;

    let report = resolve(
        &sub.domain_root,
        &sub.state_dir,
        "notes/a.md",
        Resolution::Mine,
    )
    .unwrap();
    assert_eq!(report.resolved, "notes/a.md");
    assert_eq!(report.remaining, 0);
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"line one LOCAL\n"
    );
    assert!(load_state(&sub.state_dir).conflicts.is_empty());
}

#[tokio::test]
async fn scenario_22_resolve_theirs_edit_edit_takes_upstream_content() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let sub = seeded_edit_edit_conflict(&mock, &spec).await;

    let report = resolve(
        &sub.domain_root,
        &sub.state_dir,
        "notes/a.md",
        Resolution::Theirs,
    )
    .unwrap();
    assert_eq!(report.remaining, 0);
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"line one UPSTREAM\n"
    );
    assert!(load_state(&sub.state_dir).conflicts.is_empty());
}

#[tokio::test]
async fn scenario_22_resolve_theirs_edit_delete_deletes_the_local_file() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"content\n")]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;
    write(&sub.domain_root.join("notes/a.md"), b"locally edited\n");
    let c2 = mock.add_commit(
        sub_commit_files(&[("MANIFEST.md", b"# Manifest")]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    pull(&mock, &spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert_eq!(load_state(&sub.state_dir).conflicts.len(), 1);

    let report = resolve(
        &sub.domain_root,
        &sub.state_dir,
        "notes/a.md",
        Resolution::Theirs,
    )
    .unwrap();
    assert_eq!(report.remaining, 0);
    assert!(!sub.domain_root.join("notes/a.md").exists());
    assert!(load_state(&sub.state_dir).conflicts.is_empty());
}

#[tokio::test]
async fn scenario_22_resolve_merged_writes_the_supplied_content() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let sub = seeded_edit_edit_conflict(&mock, &spec).await;

    let merged: &[u8] = b"merged by hand\n";
    let report = resolve(
        &sub.domain_root,
        &sub.state_dir,
        "notes/a.md",
        Resolution::Merged(merged),
    )
    .unwrap();
    assert_eq!(report.remaining, 0);
    assert_eq!(read(&sub.domain_root.join("notes/a.md")), merged);
}

#[tokio::test]
async fn scenario_22_resolve_unknown_path_errors_and_lists_open_conflicts() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let sub = seeded_edit_edit_conflict(&mock, &spec).await;

    let err = resolve(
        &sub.domain_root,
        &sub.state_dir,
        "notes/missing.md",
        Resolution::Mine,
    )
    .unwrap_err();
    match err {
        crystalline_remote::RemoteError::ConflictNotFound { path, open } => {
            assert_eq!(path, "notes/missing.md");
            assert_eq!(open, vec!["notes/a.md".to_string()]);
        }
        other => panic!("expected ConflictNotFound, got {other:?}"),
    }
    // Untouched: the error refused before any write.
    assert_eq!(
        read(&sub.domain_root.join("notes/a.md")),
        b"line one LOCAL\n"
    );
    assert_eq!(load_state(&sub.state_dir).conflicts.len(), 1);
}

// Scenario 23: the generated title and summary rules, across singular and
// plural counts and every change mix, checked against the actual PR request
// the mock recorded (title and body are otherwise internal to `propose`).

#[tokio::test]
async fn scenario_23_generated_title_pluralizes_additions_only() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(sub_commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    write(&sub.domain_root.join("notes/one.md"), b"one\n");
    write(&sub.domain_root.join("notes/two.md"), b"two\n");

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };
    let req = mock.proposal_request(report.number).unwrap();
    assert_eq!(req.title, "Share 2 new engrams from brand");
    assert_eq!(req.body.lines().next().unwrap(), "Shares 2 new engrams.");
}

#[tokio::test]
async fn scenario_23_generated_title_singular_modification_only() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"v1\n")]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    write(&sub.domain_root.join("notes/a.md"), b"v2\n");

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };
    let req = mock.proposal_request(report.number).unwrap();
    assert_eq!(req.title, "Refine 1 engram in brand");
    assert_eq!(req.body.lines().next().unwrap(), "Refines 1 engram.");
}

#[tokio::test]
async fn scenario_23_generated_summary_joins_three_plural_clauses_without_an_oxford_comma() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(
        sub_commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/m1.md", b"v1\n"),
            ("notes/m2.md", b"v1\n"),
            ("notes/d1.md", b"v1\n"),
            ("notes/d2.md", b"v1\n"),
        ]),
        None,
    );
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    write(&sub.domain_root.join("notes/a1.md"), b"new\n");
    write(&sub.domain_root.join("notes/a2.md"), b"new\n");
    write(&sub.domain_root.join("notes/m1.md"), b"v2\n");
    write(&sub.domain_root.join("notes/m2.md"), b"v2\n");
    std::fs::remove_file(sub.domain_root.join("notes/d1.md")).unwrap();
    std::fs::remove_file(sub.domain_root.join("notes/d2.md")).unwrap();

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };

    // A mixed change set always titles as a generic update, regardless of
    // how many files each kind touches.
    let req = mock.proposal_request(report.number).unwrap();
    assert_eq!(req.title, "Share updates from brand");
    assert_eq!(
        report.summary,
        "Shares 2 new engrams, refines 2 engrams and retires 2 engrams."
    );
    assert_eq!(req.body.lines().next().unwrap(), report.summary);
}

#[tokio::test]
async fn scenario_23_caller_supplied_title_and_description_are_used_verbatim() {
    let mock = MockProvider::new();
    let spec = share_spec();
    let c1 = mock.add_commit(sub_commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let sub = subscribe_named(&mock, &spec, &c1, "brand").await;

    write(&sub.domain_root.join("notes/new.md"), b"content\n");

    let outcome = propose(
        &mock,
        &spec,
        &sub.domain_root,
        "brand",
        &sub.state_dir,
        ShareOptions {
            title: Some("My own title"),
            description: Some("My own description, written by hand."),
            proposal: None,
            stacks_allowed: false,
        },
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };

    let req = mock.proposal_request(report.number).unwrap();
    assert_eq!(req.title, "My own title");
    assert_eq!(req.body, "My own description, written by hand.");
    // The state also records the caller's title, not a generated one.
    let recorded = &load_state(&sub.state_dir).proposals[0];
    assert_eq!(recorded.title, "My own title");
}

// Scenario 24: a domain whose origin carries hidden upstream paths (a
// dot-file, a dot-directory, the domain config file) never extracts the
// hidden ones to the working tree or the base snapshot; the domain config
// file is the one dot-prefixed exception, since it travels with the domain
// like any other tracked file. Status then reports zero local changes (the
// hidden paths are invisible to both the base snapshot and the local-change
// walk, so nothing looks deleted), and a later share proposal for a trivial
// visible edit proposes only that edit, never a `Deleted` entry for a hidden
// path the domain never tracked in the first place.

#[tokio::test]
async fn scenario_24_hidden_upstream_paths_never_extract_status_or_share_clean() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha"),
            (".gitignore", b"target/\n"),
            (".github/workflows/ci.yml", b"name: ci\n"),
            (".crystalline.yaml", b"config: true\n"),
        ]),
        None,
    );
    let (sub, report) = subscribe_at(&mock, &c1).await;

    // Working tree: the visible file and the domain config file land, the
    // hidden dot-file and dot-directory never do.
    assert_eq!(read(&sub.domain_root.join("notes/a.md")), b"alpha");
    assert_eq!(
        read(&sub.domain_root.join(".crystalline.yaml")),
        b"config: true\n"
    );
    assert!(!sub.domain_root.join(".gitignore").exists());
    assert!(!sub.domain_root.join(".github").exists());
    assert_eq!(
        report.files_written, 3,
        "MANIFEST.md, notes/a.md and .crystalline.yaml; the two hidden paths never count"
    );

    // Origin state: the same three paths, nothing hidden stamped.
    let st = load_state(&sub.state_dir);
    let mut stamped: Vec<&str> = st.files.keys().map(String::as_str).collect();
    stamped.sort();
    assert_eq!(
        stamped,
        vec![".crystalline.yaml", "MANIFEST.md", "notes/a.md"]
    );
    assert!(!st.files.contains_key(".gitignore"));
    assert!(!st.files.contains_key(".github/workflows/ci.yml"));

    // Status: the hidden paths this domain never tracked cannot show up as
    // local changes, since the base snapshot never claimed them either.
    let status_report = status(&spec(), &sub.domain_root, &sub.state_dir, None, false)
        .await
        .unwrap();
    assert_eq!(
        status_report.local_changes, 0,
        "{:?}",
        status_report.local_changes
    );

    // A trivial visible edit proposes only that edit: no `Deleted` entries
    // for hidden paths the domain never tracked.
    write(&sub.domain_root.join("notes/a.md"), b"alpha revised\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "team-knowledge",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let share_report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };
    assert_eq!(share_report.updated, vec!["notes/a.md".to_string()]);
    assert!(share_report.added.is_empty(), "{:?}", share_report.added);
    assert!(
        share_report.deleted.is_empty(),
        "no hidden path may be proposed for deletion: {:?}",
        share_report.deleted
    );
}

// Scenario 25: upstream adds a hidden file (a compare-driven pull, the
// ordinary path when the change set is small). The pull ignores it entirely:
// not written to the working tree, not stamped into the base snapshot and not
// reported in `applied`, even though the base commit still advances to head.

#[tokio::test]
async fn scenario_25_pull_ignores_a_hidden_upstream_addition_via_compare() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha"),
            (".github/workflows/ci.yml", b"name: ci\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(!report.up_to_date);
    assert!(report.applied.is_empty(), "{:?}", report.applied);
    assert!(!sub.domain_root.join(".github").exists());

    let st = load_state(&sub.state_dir);
    assert!(!st.files.contains_key(".github/workflows/ci.yml"));
    assert_eq!(st.base_commit, c2, "base still advances to head");
}

// Scenario 26: the same hidden-addition pull, forced through the whole-tree
// tarball diff fallback (a truncated compare) instead of the compare-based
// path scenario 25 exercises, so both routes into `extract_tarball` agree.

#[tokio::test]
async fn scenario_26_pull_ignores_a_hidden_upstream_addition_via_tarball_fallback() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha revised\n"),
            (".env", b"SECRET=upstream\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    mock.set_truncate(true);

    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    assert!(!report.up_to_date);
    assert_eq!(report.applied, vec!["notes/a.md".to_string()]);
    assert!(!sub.domain_root.join(".env").exists());

    let st = load_state(&sub.state_dir);
    assert!(!st.files.contains_key(".env"));
    assert_eq!(st.base_commit, c2);
}

// --- M10: team-domain out-of-subtree artifact mirror --------------------------
//
// A team domain materializes only its subtree into the working tree, so an
// out-of-subtree provisioning decl (`skills: ../skills`) is served from a
// mirror `subscribe` and `pull` maintain under `<state_dir>/artifacts/<kind>`,
// exactly where `crystalline_core::provision::resolve_source_roots` points a
// team domain's out-of-subtree decls. The mirror's decl set comes from the
// MANIFEST bytes inside the fetched tarball, never the local working tree.

/// A team origin whose domain lives at the `knowledge/` subpath, so a
/// `../skills` decl points at a sibling folder at the repository root rather
/// than climbing out of the repository.
fn team_spec() -> OriginSpec {
    OriginSpec {
        repo: "team/knowledge".to_string(),
        subpath: Some("knowledge".to_string()),
        branch: "main".to_string(),
    }
}

/// A valid MANIFEST engram carrying `provisioning` as its Provisioning bullets.
fn manifest_md(provisioning: &str) -> Vec<u8> {
    let mut source = crystalline_core::manifest_template("Team", "2026-07-10");
    source.push_str("\n## Provisioning\n\n");
    source.push_str(provisioning);
    source.into_bytes()
}

/// A repo-relative path to owned-bytes commit map, for fixtures that mix the
/// domain subtree with out-of-subtree artifact folders in one commit.
fn commit_map(pairs: Vec<(&str, Vec<u8>)>) -> BTreeMap<String, Vec<u8>> {
    pairs
        .into_iter()
        .map(|(path, content)| (path.to_string(), content))
        .collect()
}

#[tokio::test]
async fn subscribe_materializes_out_of_subtree_artifact_mirror() {
    let mock = MockProvider::new();
    let manifest = manifest_md("- skills: ../skills\n- mcps: ../mcps\n- agents: agents\n");
    let c1 = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", manifest.clone()),
            (
                "knowledge/agents/local.md",
                b"served from the working tree".to_vec(),
            ),
            ("skills/tide-tables/SKILL.md", b"# Tide Tables\n".to_vec()),
            (
                "skills/tide-tables/scripts/chart.sh",
                b"echo chart\n".to_vec(),
            ),
            (
                "mcps/lighthouse.json",
                br#"{"server":{"command":"x"}}"#.to_vec(),
            ),
        ]),
        None,
    );
    let spec = team_spec();
    let sub = subscribe_named(&mock, &spec, &c1, "team-knowledge").await;

    // The out-of-subtree folders land under artifacts/<kind>, keyed by kind.
    let artifacts = sub.state_dir.join("artifacts");
    assert_eq!(
        read(&artifacts.join("skills/tide-tables/SKILL.md")),
        b"# Tide Tables\n"
    );
    assert_eq!(
        read(&artifacts.join("skills/tide-tables/scripts/chart.sh")),
        b"echo chart\n"
    );
    assert_eq!(
        read(&artifacts.join("mcps/lighthouse.json")),
        br#"{"server":{"command":"x"}}"#
    );

    // An in-subtree decl creates no mirror dir; the working tree serves it.
    assert!(!artifacts.join("agents").exists());
    assert_eq!(
        read(&sub.domain_root.join("agents/local.md")),
        b"served from the working tree"
    );
    // The out-of-subtree folders never leak into the working tree.
    assert!(!sub.domain_root.join("skills").exists());
    assert!(!sub.domain_root.join("mcps").exists());
}

#[tokio::test]
async fn pull_refreshes_mirror_when_artifact_files_change() {
    let mock = MockProvider::new();
    let manifest = manifest_md("- skills: ../skills\n");
    let c1 = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", manifest.clone()),
            ("skills/tide-tables/SKILL.md", b"# v1\n".to_vec()),
        ]),
        None,
    );
    let spec = team_spec();
    let sub = subscribe_named(&mock, &spec, &c1, "team-knowledge").await;
    let mirrored = sub.state_dir.join("artifacts/skills/tide-tables/SKILL.md");
    assert_eq!(read(&mirrored), b"# v1\n");

    // Upstream changes a mirrored file only; the MANIFEST is unchanged, so the
    // refresh is driven by the changed path falling under the declared root
    // (the compare path, since the change set is small).
    let c2 = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", manifest.clone()),
            ("skills/tide-tables/SKILL.md", b"# v2 upstream\n".to_vec()),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let report = pull(&mock, &spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert!(!report.up_to_date);
    assert_eq!(read(&mirrored), b"# v2 upstream\n");
    assert_eq!(load_state(&sub.state_dir).base_commit, c2);
}

#[tokio::test]
async fn pull_manifest_change_reshapes_mirror() {
    let mock = MockProvider::new();
    let m1 = manifest_md("- skills: ../skills\n");
    let c1 = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", m1.clone()),
            ("skills/tide-tables/SKILL.md", b"skill\n".to_vec()),
            ("agents/pilot.md", b"pilot\n".to_vec()),
        ]),
        None,
    );
    let spec = team_spec();
    let sub = subscribe_named(&mock, &spec, &c1, "team-knowledge").await;
    assert!(
        sub.state_dir
            .join("artifacts/skills/tide-tables/SKILL.md")
            .exists()
    );
    assert!(!sub.state_dir.join("artifacts/agents").exists());

    // Upstream drops the skills decl and adds an agents decl. The MANIFEST
    // changed, so the mirror is rebuilt from the new decl set.
    let m2 = manifest_md("- agents: ../agents\n");
    let c2 = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", m2.clone()),
            ("skills/tide-tables/SKILL.md", b"skill\n".to_vec()),
            ("agents/pilot.md", b"pilot\n".to_vec()),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    pull(&mock, &spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    // The dropped kind is pruned, the added kind is materialized.
    assert!(!sub.state_dir.join("artifacts/skills").exists());
    assert_eq!(
        read(&sub.state_dir.join("artifacts/agents/pilot.md")),
        b"pilot\n"
    );
}

#[tokio::test]
async fn escaping_decl_fails_subscribe_and_pull() {
    let spec = team_spec();
    let hostile = manifest_md("- skills: ../../evil\n");

    // Subscribe: a decl normalizing outside the repository root fails outright
    // with the target untouched.
    let mock = MockProvider::new();
    let bad = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", hostile.clone()),
            ("evil/x.md", b"nope\n".to_vec()),
        ]),
        None,
    );
    mock.set_branch("main", &bad);
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let domain_root = work.path().join("domain");
    let state_dir = state.path().join("origin");
    let err = subscribe(&mock, &spec, &domain_root, &state_dir)
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_remote::RemoteError::State(_)),
        "{err:?}"
    );
    assert!(
        !domain_root.exists(),
        "subscribe must leave the target untouched"
    );
    assert!(OriginState::load(&state_dir).unwrap().is_none());

    // Pull: a clean subscribe, then a later commit whose MANIFEST turns a decl
    // hostile fails the pull and leaves the previous mirror and base intact.
    let mock2 = MockProvider::new();
    let good = manifest_md("- skills: ../skills\n");
    let c1 = mock2.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", good.clone()),
            ("skills/tide-tables/SKILL.md", b"good\n".to_vec()),
        ]),
        None,
    );
    let sub = subscribe_named(&mock2, &spec, &c1, "team-knowledge").await;
    let mirrored = sub.state_dir.join("artifacts/skills/tide-tables/SKILL.md");
    assert_eq!(read(&mirrored), b"good\n");

    let c2 = mock2.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", hostile.clone()),
            ("skills/tide-tables/SKILL.md", b"good\n".to_vec()),
            ("evil/x.md", b"nope\n".to_vec()),
        ]),
        Some(&c1),
    );
    mock2.set_branch("main", &c2);

    let err = pull(&mock2, &spec, &sub.domain_root, &sub.state_dir)
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_remote::RemoteError::State(_)),
        "{err:?}"
    );
    assert_eq!(
        read(&mirrored),
        b"good\n",
        "the previous mirror stays intact"
    );
    assert_eq!(
        load_state(&sub.state_dir).base_commit,
        c1,
        "the base is not advanced when the mirror refresh fails"
    );
}

// Domain removal (section 3): the mirror lives entirely inside the origin
// state directory and never in the working tree. `crystalline` removes a
// domain by dropping it from the config and leaving its files and index rows
// untouched (see `crystalline_cli::cmd::domain_remove`); nothing deletes the
// origin state directory today, so the mirror shares the exact fate of the
// base snapshot and state.json - reclaimed whenever origin state is. This test
// proves the containment that makes any origin-state reclamation sufficient.
#[tokio::test]
async fn domain_removal_drops_the_mirror() {
    let mock = MockProvider::new();
    let manifest = manifest_md("- skills: ../skills\n");
    let c1 = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", manifest.clone()),
            ("skills/tide-tables/SKILL.md", b"skill\n".to_vec()),
        ]),
        None,
    );
    let spec = team_spec();
    let sub = subscribe_named(&mock, &spec, &c1, "team-knowledge").await;

    let artifacts = sub.state_dir.join("artifacts");
    assert!(artifacts.join("skills/tide-tables/SKILL.md").exists());
    // The mirror is never in the working tree, so reclaiming origin state is
    // enough to drop it: no stray artifact folder lingers beside the engrams.
    assert!(!sub.domain_root.join("skills").exists());

    std::fs::remove_dir_all(&sub.state_dir).unwrap();
    assert!(!artifacts.exists());
}

/// Installs a scratch `HOME` and `XDG_STATE_HOME` for a test and restores the
/// previous values on drop, even if an assertion panics. Env must stay
/// installed across the whole test, since both
/// `crystalline_core::config::origin_state_dir` and `resolve_source_roots`
/// (which recomputes it) must resolve to the same scratch state directory.
/// This is the only test in this binary that mutates process environment, so
/// there is no other env-mutating test to serialize against.
struct EnvGuard {
    home: Option<std::ffi::OsString>,
    xdg_state: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn install(home: &Path) -> EnvGuard {
        let guard = EnvGuard {
            home: std::env::var_os("HOME"),
            xdg_state: std::env::var_os("XDG_STATE_HOME"),
        };
        // SAFETY: no other test in this binary reads or writes HOME or
        // XDG_STATE_HOME, and the guard restores both on drop.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("XDG_STATE_HOME", home.join("state"));
        }
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see `install` - this binary has no concurrent env access.
        unsafe {
            match &self.home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.xdg_state {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }
}

// End-to-end at the core boundary: a subscribe-shaped mirror is visible to the
// core provisioning chain. With `origin_state_dir` pointed at a scratch state
// directory, `resolve_source_roots` resolves the `../skills` decl into the
// mirror, `scan_domain` reads the mirrored skill and `desired_set` surfaces its
// rel key sourced from the mirror.
#[tokio::test]
async fn mirror_flows_through_resolve_scan_and_desired_set() {
    let home = tempfile::tempdir().unwrap();
    let _env = EnvGuard::install(home.path());

    let domain = "harbor-team";
    let spec = team_spec();
    let mock = MockProvider::new();
    let manifest = manifest_md("- skills: ../skills\n");
    let c1 = mock.add_commit(
        commit_map(vec![
            ("knowledge/MANIFEST.md", manifest.clone()),
            ("skills/tide-tables/SKILL.md", b"# Tide Tables\n".to_vec()),
            ("skills/tide-tables/scripts/chart.sh", b"echo\n".to_vec()),
        ]),
        None,
    );
    mock.set_branch("main", &c1);

    let state_dir = crystalline_core::config::origin_state_dir(domain).unwrap();
    let work = tempfile::tempdir().unwrap();
    let domain_root = work.path().join("harbor");
    subscribe(&mock, &spec, &domain_root, &state_dir)
        .await
        .unwrap();

    // A team-domain entry pointing at the materialized working tree and origin.
    let mut entry = crystalline_core::config::DomainEntry::file(&domain_root);
    entry.origin = Some(crystalline_core::config::OriginConfig {
        repo: "team/knowledge".to_string(),
        path: Some("knowledge".to_string()),
        branch: None,
        poll_secs: None,
    });

    let roots = crystalline_core::provision::resolve_source_roots(domain, &entry);
    let mirror_skills = state_dir.join("artifacts").join("skills");
    assert!(
        roots.iter().any(
            |(kind, path)| *kind == crystalline_core::ArtifactType::Skills
                && *path == mirror_skills
        ),
        "resolve_source_roots should point the skills decl at the mirror: {roots:?}"
    );

    let (artifacts, _notices) = crystalline_core::provision::scan_domain(domain, &roots);
    let (desired, _notices) = crystalline_core::provision::desired_set(
        crystalline_core::HarnessKind::ClaudeCode,
        std::slice::from_ref(&artifacts),
    );

    let key = "skills/tide-tables/SKILL.md";
    assert!(
        desired.files.contains_key(key),
        "desired set should carry the mirrored skill: {:?}",
        desired.files.keys().collect::<Vec<_>>()
    );
    let source = desired.files[key]
        .source_path()
        .expect("a passthrough skill keeps its source path");
    assert!(
        source.starts_with(&mirror_skills),
        "the winning source should be the mirror: {source:?}"
    );

    // Keep the scratch directories alive until every assertion has run.
    drop(home);
    drop(work);
}

// Scenario 27: a second share while the first proposal is open updates it in
// place: one new commit on the same branch, ref advanced, body PATCHed, still
// exactly one Proposal record and no second create_proposal.

#[tokio::test]
async fn scenario_27_consecutive_shares_update_one_proposal() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    let head_after_create = mock.branch_commit(&first.branch).unwrap();

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let updated = match outcome {
        ProposeOutcome::Updated(r) => r,
        other => panic!("expected Updated, got {other:?}"),
    };
    assert_eq!(updated.number, first.number, "same PR");
    assert_eq!(updated.url, first.url, "same URL");
    assert_eq!(updated.branch, first.branch, "same branch");

    // The branch advanced by exactly one new commit whose parent is the
    // previous head, and the record moved with it.
    let head_after_update = mock.branch_commit(&first.branch).unwrap();
    assert_ne!(head_after_update, head_after_create);
    assert_eq!(
        mock.commit_parents(&head_after_update).unwrap(),
        vec![head_after_create.clone()]
    );
    let calls = mock.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("create_proposal:"))
            .count(),
        1,
        "never a second PR: {calls:?}"
    );
    assert!(
        calls.contains(&format!(
            "update_branch:{}:{head_after_update}:force=false",
            first.branch
        )),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("update_proposal:{}", first.number)),
        "{calls:?}"
    );
    // The body was regenerated from the fresh change list.
    let req = mock.proposal_request(first.number).unwrap();
    assert!(req.body.contains("notes/b.md"), "{}", req.body);

    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals.len(), 1);
    let prop = &st.proposals[0];
    assert_eq!(
        prop.head_commit.as_deref(),
        Some(head_after_update.as_str())
    );
    assert_eq!(prop.base_commit.as_deref(), Some(st.base_commit.as_str()));
    assert!(prop.updated_at.is_some());
    // Both changed files are on the record now.
    let mut paths: Vec<_> = prop.files.iter().map(|f| f.path.clone()).collect();
    paths.sort();
    assert_eq!(
        paths,
        vec!["notes/a.md".to_string(), "notes/b.md".to_string()]
    );
}

// Scenario 28: when upstream advanced between shares, the update commit gets
// two parents (branch head first, then the new base) so the PR's merge base
// moves forward and the diff never shows upstream changes as ours.

#[tokio::test]
async fn scenario_28_share_update_after_upstream_advance_makes_a_merge_commit() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    let head_after_create = mock.branch_commit(&first.branch).unwrap();

    // Upstream gains an unrelated file; the pull inside the next propose
    // integrates it and advances base_commit past the recorded one.
    let old_base = load_state(&sub.state_dir).base_commit.clone();
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha\n"),
            ("notes/upstream.md", b"from the team\n"),
        ]),
        Some(&old_base),
    );
    mock.set_branch("main", &c2);

    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, ProposeOutcome::Updated(_)), "{outcome:?}");

    let head_after_update = mock.branch_commit(&first.branch).unwrap();
    let st = load_state(&sub.state_dir);
    assert_eq!(
        mock.commit_parents(&head_after_update).unwrap(),
        vec![head_after_create, st.base_commit.clone()],
        "branch head first, then the advanced base"
    );
    assert_eq!(
        st.proposals[0].base_commit.as_deref(),
        Some(st.base_commit.as_str()),
        "the record's base moved with the merge"
    );
}

// Scenario 29: a reviewer-amended branch refuses the update with no writes.

#[tokio::test]
async fn scenario_29_diverged_branch_refuses_with_no_writes() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;

    // A reviewer pushes a commit onto the proposal branch: the live head no
    // longer matches the recorded head_commit.
    let reviewer = mock.add_commit(commit_files(&[("MANIFEST.md", b"# amended")]), None);
    mock.set_branch(&first.branch, &reviewer);
    let writes_before = mock
        .calls()
        .iter()
        .filter(|c| {
            c.starts_with("create_blob:")
                || c.starts_with("create_tree:")
                || c.starts_with("create_commit:")
                || c.starts_with("create_branch:")
                || c.starts_with("update_branch:")
                || c.starts_with("update_proposal:")
                || c.starts_with("create_proposal:")
        })
        .count();

    write(&sub.domain_root.join("notes/d.md"), b"delta\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    match outcome {
        ProposeOutcome::ProposalDiverged {
            number,
            url,
            branch,
        } => {
            assert_eq!(number, first.number);
            assert_eq!(url, first.url);
            assert_eq!(branch, first.branch);
        }
        other => panic!("expected ProposalDiverged, got {other:?}"),
    }
    let writes_after = mock
        .calls()
        .iter()
        .filter(|c| {
            c.starts_with("create_blob:")
                || c.starts_with("create_tree:")
                || c.starts_with("create_commit:")
                || c.starts_with("create_branch:")
                || c.starts_with("update_branch:")
                || c.starts_with("update_proposal:")
                || c.starts_with("create_proposal:")
        })
        .count();
    assert_eq!(writes_after, writes_before, "no provider write happened");
    // The record is untouched: still Open, still the old head.
    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals[0].status, ProposalStatus::Open);
}

// Scenario 30: a declined proposal is superseded by the next share - moved to
// history keeping Declined, its branch best-effort deleted, and a fresh PR
// opened.

#[tokio::test]
async fn scenario_30_declined_proposal_is_superseded_on_next_share() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    mock.set_proposal_state(first.number, ProposalState::Declined);
    // The pull inside the next propose marks it Declined first.

    write(&sub.domain_root.join("notes/e.md"), b"epsilon\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let second = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected a fresh Proposed, got {other:?}"),
    };
    assert_ne!(second.number, first.number);

    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals.len(), 1, "only the new proposal remains");
    assert_eq!(st.proposals[0].number, second.number);
    assert_eq!(st.history[0].number, first.number);
    assert_eq!(st.history[0].status, ProposalStatus::Declined);
    assert!(
        mock.calls()
            .contains(&format!("delete_branch:{}", first.branch)),
        "{:?}",
        mock.calls()
    );
}

// Scenario 31: an open record whose branch ref is gone refreshes once; still
// open with no branch means it is treated as declined and a new share opens.

#[tokio::test]
async fn scenario_31_gone_ref_with_open_pr_treats_as_declined_and_creates_new() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    // Someone deleted the branch out from under the still-open PR.
    let _ = mock.delete_branch(&spec(), &first.branch).await;

    write(&sub.domain_root.join("notes/f.md"), b"zeta\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let second = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("expected Proposed, got {other:?}"),
    };
    assert_ne!(second.number, first.number);
    let st = load_state(&sub.state_dir);
    assert_eq!(st.history[0].number, first.number);
    assert_eq!(st.history[0].status, ProposalStatus::Declined);
}

// Scenario 32 (migration): a pre-extension record with head_commit None
// adopts the live branch head silently on its first update.

#[tokio::test]
async fn scenario_32_migration_none_head_commit_adopts_live_head() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    // Erase the recorded head, simulating a record written before the field.
    let mut st = load_state(&sub.state_dir);
    st.proposals[0].head_commit = None;
    st.proposals[0].base_commit = None;
    st.save(&sub.state_dir).unwrap();

    write(&sub.domain_root.join("notes/g.md"), b"eta\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, ProposeOutcome::Updated(_)), "{outcome:?}");
    let st = load_state(&sub.state_dir);
    assert_eq!(
        st.proposals[0].head_commit.as_deref(),
        Some(mock.branch_commit(&first.branch).unwrap().as_str())
    );
    assert_eq!(st.proposals[0].number, first.number);
}

// An interrupted share-update - the branch moved, the proposal patch failed -
// heals on the next share instead of blaming a reviewer forever.

#[tokio::test]
async fn an_interrupted_update_heals_on_the_next_share() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    let head_before = mock.branch_commit(&first.branch).unwrap();

    // Cut the update in half at the worst possible place: `update_branch`
    // lands, `update_proposal` fails, so the branch is ahead of the last head
    // this machine finished recording.
    mock.fail_update_proposal(first.number);
    write(&sub.domain_root.join("notes/h.md"), b"theta\n");
    let err = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            crystalline_remote::RemoteError::Api { status: 500, .. }
        ),
        "{err:?}"
    );
    let pushed = mock.branch_commit(&first.branch).unwrap();
    assert_ne!(pushed, head_before, "the branch really did move");
    let st = load_state(&sub.state_dir);
    assert_eq!(
        st.proposals[0].pending_head_commit.as_deref(),
        Some(pushed.as_str()),
        "the push was announced before it was made, so the record can name it"
    );

    // The retry is an ordinary update, not a divergence, and it leaves the
    // record settled again.
    mock.heal_update_proposal(first.number);
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        matches!(plan.action, PlannedAction::Update { number, .. } if number == first.number),
        "the preview reads our own half-finished push as ours: {:?}",
        plan.action
    );
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    match outcome {
        ProposeOutcome::Updated(r) => assert_eq!(r.number, first.number),
        other => panic!("expected Updated, got {other:?}"),
    }
    let st = load_state(&sub.state_dir);
    assert_eq!(
        st.proposals[0].head_commit.as_deref(),
        Some(mock.branch_commit(&first.branch).unwrap().as_str()),
        "the record caught up with the branch"
    );
    assert_eq!(
        st.proposals[0].pending_head_commit, None,
        "and nothing is left pending"
    );
}

// The heal is narrow: a head that matches neither the recorded one nor the
// announced one is still a reviewer's, and is still refused.

#[tokio::test]
async fn a_foreign_head_still_refuses_after_an_interrupted_update() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;

    mock.fail_update_proposal(first.number);
    write(&sub.domain_root.join("notes/i.md"), b"iota\n");
    let _ = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap_err();
    mock.heal_update_proposal(first.number);

    // A reviewer pushes on top of the interrupted push: the live head is now
    // neither the recorded head nor the announced one.
    let reviewer = mock.add_commit(commit_files(&[("MANIFEST.md", b"# amended")]), None);
    mock.set_branch(&first.branch, &reviewer);

    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        matches!(plan.action, PlannedAction::ProposalDiverged { number, .. } if number == first.number),
        "{:?}",
        plan.action
    );
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, ProposeOutcome::ProposalDiverged { number, .. } if number == first.number),
        "{outcome:?}"
    );
}

// The branch name carries 4 random hex chars after the timestamp, killing the
// same-second 422 on two rapid creates.

#[tokio::test]
async fn scenario_33_branch_name_ends_with_a_hex4_suffix() {
    let mock = MockProvider::new();
    let (_sub, report) = shared_once(&mock).await;
    let mut parts = report.branch.rsplit('-');
    let suffix = parts.next().unwrap();
    let timestamp = parts.next().unwrap();
    assert_eq!(suffix.len(), 4, "{}", report.branch);
    assert!(
        suffix.chars().all(|c| c.is_ascii_hexdigit()),
        "{}",
        report.branch
    );
    assert_eq!(timestamp.len(), 12, "{}", report.branch);
    assert!(
        timestamp.chars().all(|c| c.is_ascii_digit()),
        "{}",
        report.branch
    );
}

// NothingToShare leaves an open proposal exactly as it is.

#[tokio::test]
async fn scenario_34_nothing_to_share_leaves_the_open_proposal_untouched() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    let before = load_state(&sub.state_dir);
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    // Everything local is already on the open proposal's branch? No: the base
    // has not moved, so the same local edit is re-detected. Delete the edit
    // first to make the tree truly clean.
    drop(outcome);
    write(&sub.domain_root.join("notes/a.md"), b"alpha\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, ProposeOutcome::NothingToShare { .. }),
        "{outcome:?}"
    );
    let after = load_state(&sub.state_dir);
    assert_eq!(
        after.proposals.first().map(|p| p.number),
        Some(first.number)
    );
    assert_eq!(before.proposals[0].status, after.proposals[0].status);
}

// Scenario 35: the preview classifies without writing.

#[tokio::test]
async fn scenario_35_preview_reports_update_for_an_open_proposal_without_writing() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    write(&sub.domain_root.join("notes/h.md"), b"theta\n");
    let writes_before = mock.calls().iter().filter(|c| is_write_call(c)).count();

    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    match plan.action {
        PlannedAction::Update { number, ref url } => {
            assert_eq!(number, first.number);
            assert_eq!(url, &first.url);
        }
        other => panic!("expected Update, got {other:?}"),
    }
    assert_eq!(
        plan.changes.changes.len(),
        2,
        "the old edit and the new file"
    );
    assert!(!plan.effective_title.is_empty());

    let writes_after = mock.calls().iter().filter(|c| is_write_call(c)).count();
    assert_eq!(writes_after, writes_before, "a preview never writes");
    // And the record did not move.
    assert_eq!(load_state(&sub.state_dir).proposals[0].number, first.number);
}

#[tokio::test]
async fn scenario_35_preview_reports_create_nothing_and_diverged() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // Clean tree: nothing to share.
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(plan.action, PlannedAction::NothingToShare);

    // A local change with no open proposal: create, and the caller's title
    // wins over the generated one.
    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: Some("My title"),
            description: None,
            proposal: None,
            stacks_allowed: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(plan.action, PlannedAction::Create);
    assert_eq!(plan.effective_title, "My title");

    // Share it, amend the branch, preview again: diverged.
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    let report = match outcome {
        ProposeOutcome::Proposed(r) => r,
        other => panic!("{other:?}"),
    };
    let amended = mock.add_commit(commit_files(&[("MANIFEST.md", b"# amended")]), None);
    mock.set_branch(&report.branch, &amended);
    write(&sub.domain_root.join("notes/i.md"), b"iota\n");
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        matches!(plan.action, PlannedAction::ProposalDiverged { number, .. } if number == report.number),
        "{:?}",
        plan.action
    );
}

#[tokio::test]
async fn scenario_35_preview_still_pulls_first() {
    // Freshness is part of previewing honestly: the pull's working-tree
    // writes DO happen.
    let mock = MockProvider::new();
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    let (sub, _) = subscribe_at(&mock, &c1).await;
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/new.md", b"upstream\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);

    let _ = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(read(&sub.domain_root.join("notes/new.md")), b"upstream\n");
}

// Scenario 35 (b): the two preview branches the first pass left untested -
// conflicts pending, and a declined-only record that reads as Create without
// the supersede cleanup a real share would perform.

#[tokio::test]
async fn scenario_35_preview_reports_conflicts_pending() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one\n"),
        ]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // A same-line conflict from a previous pull.
    write(&sub.domain_root.join("notes/a.md"), b"line one LOCAL\n");
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"line one UPSTREAM\n"),
        ]),
        Some(&c1),
    );
    mock.set_branch("main", &c2);
    pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    assert_eq!(load_state(&sub.state_dir).conflicts.len(), 1);

    // An unrelated local change would otherwise be shareable; the outstanding
    // conflict alone decides the plan.
    write(&sub.domain_root.join("notes/new.md"), b"brand new\n");
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(plan.action, PlannedAction::ConflictsPending { count: 1 });
}

#[tokio::test]
async fn scenario_35_preview_of_a_declined_record_creates_without_cleanup() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    mock.set_proposal_state(first.number, ProposalState::Declined);
    pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals.len(), 1);
    assert_eq!(st.proposals[0].status, ProposalStatus::Declined);

    let writes_before = mock.calls().iter().filter(|c| is_write_call(c)).count();
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions::default(),
    )
    .await
    .unwrap();
    // A real share would supersede the declined record; a preview only says
    // what would happen.
    assert_eq!(plan.action, PlannedAction::Create);
    let writes_after = mock.calls().iter().filter(|c| is_write_call(c)).count();
    assert_eq!(
        writes_after, writes_before,
        "a preview never cleans up: no delete_branch, no close_proposal"
    );
    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals.len(), 1, "the declined record stays put");
    assert!(st.history.is_empty());
}

// Scenario 36: a pull fetches feedback for still-open proposals and caps it.

#[tokio::test]
async fn scenario_36_pull_fetches_and_caps_feedback_on_open_proposals() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    let items: Vec<FeedbackItem> = (0..60)
        .map(|i| FeedbackItem {
            author: "bob".to_string(),
            body: format!("comment {i}"),
            path: None,
            line: None,
            submitted_at: format!("2026-08-21T10:{:02}:00Z", i % 60),
            kind: FeedbackKind::Comment,
        })
        .collect();
    mock.set_feedback(
        first.number,
        Feedback {
            review_state: Some("changes_requested".to_string()),
            items,
        },
    );

    pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();

    let st = load_state(&sub.state_dir);
    let prop = &st.proposals[0];
    assert_eq!(prop.review_state.as_deref(), Some("changes_requested"));
    assert_eq!(prop.feedback.len(), 50, "capped at the 50 newest");
    // Newest by submitted_at survive: comment 59 down to comment 10.
    assert!(prop.feedback.iter().any(|f| f.body == "comment 59"));
    assert!(!prop.feedback.iter().any(|f| f.body == "comment 5"));
    assert!(prop.updated_at.is_some());
}

// Scenario 36 (b): a feedback failure is non-fatal and keeps the previous
// feedback.

#[tokio::test]
async fn scenario_36_feedback_failure_is_non_fatal_and_keeps_previous() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    mock.set_feedback(
        first.number,
        Feedback {
            review_state: Some("commented".to_string()),
            items: vec![FeedbackItem {
                author: "ana".to_string(),
                body: "looks close".to_string(),
                path: None,
                line: None,
                submitted_at: "2026-08-21T09:00:00Z".to_string(),
                kind: FeedbackKind::Comment,
            }],
        },
    );
    pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    mock.fail_feedback(first.number);
    let report = pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .expect("a feedback failure never fails the pull");
    assert!(report.up_to_date);
    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals[0].feedback.len(), 1, "previous feedback kept");
    assert_eq!(st.proposals[0].review_state.as_deref(), Some("commented"));
}

// Scenario 37: status consults the live open-proposal list.

#[tokio::test]
async fn scenario_37_status_flips_a_merged_elsewhere_proposal_without_consuming() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    // Merged on GitHub: gone from the open list, and the single GET says merged.
    mock.set_proposal_state(first.number, ProposalState::Merged);

    let report = status(
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(&mock),
        false,
    )
    .await
    .unwrap();
    assert!(
        report.open_proposals.is_empty(),
        "the merged proposal left the open list"
    );
    let st = load_state(&sub.state_dir);
    let prop = st
        .proposals
        .iter()
        .find(|p| p.number == first.number)
        .expect("still in proposals: status never consumes");
    assert_eq!(prop.status, ProposalStatus::Merged);
    // Consumption stays a pull concern. Merging is what moved the branch
    // upstream in the first place, so let the mock forge reflect that (as
    // every other merge scenario does) and pull: the record lands in history.
    let branch_commit = mock.branch_commit(&first.branch).unwrap();
    mock.set_branch("main", &branch_commit);
    pull(&mock, &spec(), &sub.domain_root, &sub.state_dir)
        .await
        .unwrap();
    let st = load_state(&sub.state_dir);
    assert!(st.proposals.iter().all(|p| p.number != first.number));
    assert_eq!(st.history[0].number, first.number);
}

#[tokio::test]
async fn scenario_37_status_flags_an_amended_branch() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    let amended = mock.add_commit(commit_files(&[("MANIFEST.md", b"# amended")]), None);
    mock.set_branch(&first.branch, &amended);

    let report = status(
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(&mock),
        false,
    )
    .await
    .unwrap();
    assert_eq!(report.amended_upstream, vec![first.number]);
    assert_eq!(report.open_proposals.len(), 1, "still open, just amended");
    // Exactly one live list call answers both questions this status asks.
    assert_eq!(
        mock.calls()
            .iter()
            .filter(|c| *c == "list_open_proposals")
            .count(),
        1
    );
}

#[tokio::test]
async fn scenario_37_status_list_failure_degrades_to_local_state() {
    let mock = MockProvider::new();
    let (sub, first) = shared_once(&mock).await;
    mock.fail_open_proposals();

    let report = status(
        &spec(),
        &sub.domain_root,
        &sub.state_dir,
        Some(&mock),
        false,
    )
    .await
    .expect("a list failure never fails status");
    assert_eq!(report.open_proposals[0].number, first.number);
    assert!(report.amended_upstream.is_empty());
    let st = load_state(&sub.state_dir);
    assert_eq!(st.proposals[0].status, ProposalStatus::Open, "unchanged");
}

// The stack rules, as the 2026-08-27 spike found them on a live repository:
// bottom-to-top chains only, a closed member stays a member, a stacked
// proposal cannot be retargeted and a dissolve takes the stack away
// outright. This test drives the mock forge directly rather than an
// operation, because the mock is what every later stack scenario stands on.

/// Asserts `err` is an API failure carrying `status`, whose message contains
/// `needle` (empty to check the status alone).
fn assert_api(err: &crystalline_remote::RemoteError, status: u16, needle: &str) {
    match err {
        crystalline_remote::RemoteError::Api {
            status: got,
            message,
        } => {
            assert_eq!(*got, status, "{err:?}");
            assert!(
                message.contains(needle),
                "{message:?} does not carry {needle:?}"
            );
        }
        other => panic!("expected an Api {status}, got {other:?}"),
    }
}

/// Opens one layer through the ordinary create path: a commit on top of
/// `base_branch`'s head, a branch pointing at it and a proposal targeting
/// `base_branch`. Returns the new proposal's number.
async fn stacked_layer(mock: &MockProvider, branch: &str, base_branch: &str) -> u64 {
    let parent = mock
        .branch_commit(base_branch)
        .unwrap_or_else(|| panic!("no branch {base_branch}"));
    let path = format!("{branch}.md");
    let commit = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# team"), (path.as_str(), b"layer")]),
        Some(&parent),
    );
    mock.create_branch(&spec(), branch, &commit).await.unwrap();
    mock.create_proposal(
        &spec(),
        &ProposalRequest {
            title: format!("share {branch}"),
            body: "one layer".to_string(),
            branch: branch.to_string(),
            base_branch: base_branch.to_string(),
        },
    )
    .await
    .unwrap()
    .number
}

#[tokio::test]
async fn the_mock_forge_models_the_spiked_stack_rules() {
    let mock = MockProvider::new();
    // Before `enable_stacks` the mock is a forge without the preview: all
    // four verbs answer the way the trait defaults do.
    for err in [
        mock.list_stacks(&spec(), None).await.unwrap_err(),
        mock.create_stack(&spec(), &[1, 2]).await.unwrap_err(),
        mock.extend_stack(&spec(), 1, &[2]).await.unwrap_err(),
        mock.dissolve_stack(&spec(), 1).await.unwrap_err(),
    ] {
        assert!(
            matches!(err, crystalline_remote::RemoteError::StacksUnsupported),
            "{err:?}"
        );
    }
    mock.enable_stacks();

    let root = mock.add_commit(commit_files(&[("MANIFEST.md", b"# team")]), None);
    mock.set_branch("main", &root);
    let p1 = stacked_layer(&mock, "layer-a", "main").await;
    let p2 = stacked_layer(&mock, "layer-b", "layer-a").await;
    assert_eq!((p1, p2), (1, 2));

    // A chain runs bottom to top: reversed, the second member's base ref is
    // not the first member's head ref.
    let err = mock.create_stack(&spec(), &[p2, p1]).await.unwrap_err();
    assert_api(&err, 422, "must form a stack");

    let stack = mock.create_stack(&spec(), &[p1, p2]).await.unwrap();
    assert_eq!(
        stack.number, 3,
        "stack numbers come off the same counter as proposals"
    );
    assert!(stack.open);
    assert_eq!(
        stack.members.iter().map(|m| m.number).collect::<Vec<_>>(),
        vec![p1, p2]
    );
    assert_eq!(stack.members[0].state, "open");
    assert_eq!(
        stack.members[1].head_sha,
        mock.branch_commit("layer-b").unwrap(),
        "a member's head is read live off its branch"
    );

    // A member of a live stack cannot be retargeted: unstack first.
    let err = mock
        .update_proposal(&spec(), p2, None, None, Some("main"))
        .await
        .unwrap_err();
    assert_api(&err, 422, "part of a stack");

    // Extending appends on top of the current top member.
    let p3 = stacked_layer(&mock, "layer-c", "layer-b").await;
    let stack = mock
        .extend_stack(&spec(), stack.number, &[p3])
        .await
        .unwrap();
    assert_eq!(stack.members.len(), 3);
    assert_eq!(stack.members[2].number, p3);

    // A layer branched off trunk does not line up with that top.
    let stray = stacked_layer(&mock, "stray", "main").await;
    let err = mock
        .extend_stack(&spec(), stack.number, &[stray])
        .await
        .unwrap_err();
    assert_api(&err, 422, "must form a stack");

    // The conflict injector answers before any validation, and is spent per
    // call, so a caller's retry path can be driven without a real race.
    let p4 = stacked_layer(&mock, "layer-d", "layer-c").await;
    mock.fail_extend_stack_with_conflicts(1);
    let err = mock
        .extend_stack(&spec(), stack.number, &[p4])
        .await
        .unwrap_err();
    assert_api(&err, 409, "");
    let stack = mock
        .extend_stack(&spec(), stack.number, &[p4])
        .await
        .unwrap();
    assert_eq!(stack.members.len(), 4, "the second attempt went through");

    // Closing a member never touches the registry: it stays a member,
    // reported closed, and one open member keeps the stack open.
    mock.close_proposal(&spec(), p4).await.unwrap();
    let stack = mock.stack(stack.number).expect("the stack is still there");
    assert_eq!(stack.members.len(), 4);
    assert_eq!(stack.members[3].state, "closed");
    assert!(stack.open);

    // A closed top blocks the extend, even for a layer branched off it.
    let p5 = stacked_layer(&mock, "layer-e", "layer-d").await;
    let err = mock
        .extend_stack(&spec(), stack.number, &[p5])
        .await
        .unwrap_err();
    assert_api(&err, 422, "must form a stack");

    // Listing answers the whole registry, or one proposal's stack.
    assert_eq!(mock.list_stacks(&spec(), None).await.unwrap().len(), 1);
    assert_eq!(
        mock.list_stacks(&spec(), Some(p3)).await.unwrap()[0].number,
        stack.number
    );
    assert!(
        mock.list_stacks(&spec(), Some(stray))
            .await
            .unwrap()
            .is_empty()
    );

    // Dissolve takes the entry away outright, and the retarget the stack
    // refused a moment ago then goes through.
    mock.dissolve_stack(&spec(), stack.number).await.unwrap();
    assert!(mock.stack(stack.number).is_none());
    assert!(mock.stacks().is_empty());
    mock.update_proposal(&spec(), p2, None, None, Some("main"))
        .await
        .expect("unstack first, then retarget");

    // The hard create injector, and healing it again.
    mock.update_proposal(&spec(), p2, None, None, Some("layer-a"))
        .await
        .unwrap();
    mock.fail_create_stack();
    let err = mock.create_stack(&spec(), &[p1, p2]).await.unwrap_err();
    assert!(
        matches!(err, crystalline_remote::RemoteError::Api { .. }),
        "{err:?}"
    );
    mock.heal_create_stack();
    let again = mock.create_stack(&spec(), &[p1, p2]).await.unwrap();
    assert!(
        again.number > stack.number,
        "a fresh number off the shared counter"
    );

    // Every stack call is recorded, in the colon-separated shape the rest of
    // the lifecycle assertions read.
    let calls = mock.calls();
    assert!(
        calls.contains(&format!("create_stack:[{p1},{p2}]")),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("extend_stack:{}:[{p3}]", stack.number)),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("dissolve_stack:{}", stack.number)),
        "{calls:?}"
    );
    assert!(calls.contains(&"list_stacks".to_string()), "{calls:?}");
    assert!(calls.contains(&format!("list_stacks:{p3}")), "{calls:?}");
}

// The cached stacks probe: whether this origin's forge serves stacks at all is
// asked once per origin and remembered, and the `github.stacks` config gate
// short-circuits it before a single call leaves the machine.

#[tokio::test]
async fn the_probe_runs_once_and_caches_the_verdict() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // A forge without `enable_stacks`: the probe answers StacksUnsupported and
    // the share takes the ordinary living-proposal path.
    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: true,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, ProposeOutcome::Proposed(_)),
        "a forge without stacks still shares: {outcome:?}"
    );
    assert_eq!(
        load_state(&sub.state_dir).stacks_available,
        Some(false),
        "the verdict is cached in origin state"
    );
    assert_eq!(
        mock.calls().iter().filter(|c| *c == "list_stacks").count(),
        1,
        "exactly one probe: {:?}",
        mock.calls()
    );

    // A second share reads the cache instead of probing again.
    write(&sub.domain_root.join("notes/a.md"), b"alpha v3\n");
    propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        mock.calls().iter().filter(|c| *c == "list_stacks").count(),
        1,
        "still one probe after a second share: {:?}",
        mock.calls()
    );
}

#[tokio::test]
async fn config_off_never_probes() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;
    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");

    propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: false,
        },
    )
    .await
    .unwrap();

    assert!(
        !mock.calls().iter().any(|c| c.starts_with("list_stacks")),
        "github.stacks off never asks the forge: {:?}",
        mock.calls()
    );
    assert_eq!(
        load_state(&sub.state_dir).stacks_available,
        None,
        "the config gate leaves the cache untouched"
    );
}

// The stacked share: while a chain is open, every share opens a NEW proposal
// layered on top of it rather than rewriting the one below. These scenarios
// drive `propose` end to end against the stack-serving mock forge.

/// Shares with stacking allowed, the call every stacked scenario makes.
async fn stacked_share(mock: &MockProvider, sub: &Subscribed) -> ProposeOutcome {
    propose(
        mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: true,
        },
    )
    .await
    .expect("a stacked share should succeed")
}

/// The report of a share that opened a proposal, or a panic naming what came
/// back instead.
fn proposed(outcome: ProposeOutcome) -> crystalline_remote::ops::ProposeReport {
    match outcome {
        ProposeOutcome::Proposed(report) => report,
        other => panic!("expected Proposed, got {other:?}"),
    }
}

/// Subscribes a single-file domain against a stack-serving forge and shares
/// one local edit, the bottom layer every stacked scenario builds on.
async fn stacked_bottom_layer(
    mock: &MockProvider,
) -> (Subscribed, crystalline_remote::ops::ProposeReport) {
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(mock, &c1).await;
    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");
    let report = proposed(stacked_share(mock, &sub).await);
    (sub, report)
}

#[tokio::test]
async fn a_second_share_stacks_a_new_proposal_on_the_open_one() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, first) = stacked_bottom_layer(&mock).await;

    // The bottom layer is an ordinary create: there was no chain to stack on.
    assert_eq!(first.stack_number, None);
    assert_eq!(first.stack_position, None);
    assert_eq!(load_state(&sub.state_dir).proposals.len(), 1);

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(&mock, &sub).await);

    assert_ne!(second.number, first.number, "a new proposal, not an update");
    let state = load_state(&sub.state_dir);
    let stack = state.stack_number.expect("the chain is linked");
    assert_eq!(second.stack_number, Some(stack));
    assert_eq!(second.stack_position, Some((2, 2)));
    assert_eq!(state.proposals.len(), 2);
    assert_eq!(
        state.stacks_available,
        Some(true),
        "the probe's verdict is cached"
    );

    // The new pull request targets the layer below it, and its commit has
    // exactly one parent: that layer's head, never a two-parent merge.
    let request = mock
        .proposal_request(second.number)
        .expect("the second layer was opened through the provider");
    assert_eq!(request.base_branch, first.branch);
    let top_head = mock.branch_commit(&first.branch).expect("the layer's head");
    let layer_head = mock.branch_commit(&second.branch).expect("the new head");
    assert_eq!(mock.commit_parents(&layer_head), Some(vec![top_head]));

    let calls = mock.calls();
    assert!(
        calls.contains(&format!(
            "create_stack:[{},{}]",
            first.number, second.number
        )),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_third_share_extends_the_stack() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, _first) = stacked_bottom_layer(&mock).await;

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(&mock, &sub).await);
    let stack = second.stack_number.expect("the chain is linked");

    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let third = proposed(stacked_share(&mock, &sub).await);

    assert_eq!(third.stack_number, Some(stack), "the same stack number");
    assert_eq!(third.stack_position, Some((3, 3)));
    let calls = mock.calls();
    assert!(
        calls.contains(&format!("extend_stack:{stack}:[{}]", third.number)),
        "{calls:?}"
    );
    let state = load_state(&sub.state_dir);
    assert_eq!(state.proposals.len(), 3);
    assert_eq!(state.stack_number, Some(stack));
    assert!(!state.stack_link_pending);
}

#[tokio::test]
async fn a_failed_stack_link_degrades_and_heals_on_the_next_share() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    mock.fail_create_stack();
    let (sub, first) = stacked_bottom_layer(&mock).await;

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(&mock, &sub).await);
    // The pull request exists; only the linking call failed.
    assert_eq!(second.stack_number, None);
    assert_eq!(second.stack_position, Some((2, 2)));
    let state = load_state(&sub.state_dir);
    assert!(state.stack_link_pending, "the chain is degraded, not wrong");
    assert_eq!(state.stack_number, None);
    assert_eq!(state.proposals.len(), 2);

    mock.heal_create_stack();
    let before = mock.calls().len();
    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let third = proposed(stacked_share(&mock, &sub).await);

    let delta = mock.calls().split_off(before);
    let retried = format!("create_stack:[{},{}]", first.number, second.number);
    let retry_at = delta
        .iter()
        .position(|c| *c == retried)
        .unwrap_or_else(|| panic!("no retried link in {delta:?}"));
    let stack = third.stack_number.expect("the retry linked the chain");
    let extend_at = delta
        .iter()
        .position(|c| *c == format!("extend_stack:{stack}:[{}]", third.number))
        .unwrap_or_else(|| panic!("no extend in {delta:?}"));
    assert!(
        retry_at < extend_at,
        "the owed link is settled before the new layer's own: {delta:?}"
    );
    let first_write = delta
        .iter()
        .position(|c| c.starts_with("create_blob"))
        .unwrap_or_else(|| panic!("no share write in {delta:?}"));
    assert!(
        retry_at < first_write,
        "the retry comes before any new share work: {delta:?}"
    );

    let state = load_state(&sub.state_dir);
    assert!(!state.stack_link_pending, "the flag is cleared");
    assert_eq!(state.stack_number, Some(stack));
    assert_eq!(state.proposals.len(), 3);
}

#[tokio::test]
async fn divergence_on_the_top_layer_refuses_the_stacked_share() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, first) = stacked_bottom_layer(&mock).await;

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(&mock, &sub).await);

    // A reviewer pushes onto the TOP layer's branch.
    let foreign = mock.add_commit(commit_files(&[("MANIFEST.md", b"# reviewer")]), None);
    mock.set_branch(&second.branch, &foreign);

    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let outcome = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: true,
        },
    )
    .await
    .unwrap();
    match outcome {
        ProposeOutcome::ProposalDiverged { number, branch, .. } => {
            assert_eq!(number, second.number, "the TOP layer, not the bottom");
            assert_ne!(number, first.number);
            assert_eq!(branch, second.branch);
        }
        other => panic!("expected ProposalDiverged, got {other:?}"),
    }
    assert_eq!(
        load_state(&sub.state_dir).proposals.len(),
        2,
        "nothing was stacked"
    );
}

#[tokio::test]
async fn preview_names_the_stack_action() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, first) = stacked_bottom_layer(&mock).await;
    let top_title = load_state(&sub.state_dir).proposals[0].title.clone();

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let before = mock.calls().len();
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: true,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        plan.action,
        PlannedAction::StackOnTop {
            top_number: first.number,
            top_title,
        }
    );
    let delta = mock.calls().split_off(before);
    assert!(
        !delta.iter().any(|c| is_write_call(c)),
        "a preview writes nothing: {delta:?}"
    );
}

#[tokio::test]
async fn legacy_multi_open_state_keeps_the_living_proposal_flow() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // Two open records with no stack behind them: residue from before
    // stacking existed, which the living-proposal flow still owns.
    for number in [41u64, 42] {
        seed_proposal(&sub.state_dir, number, "notes/a.md", None);
        let branch = format!("crystalline/share-{number}");
        mock.register_proposal_branch(number, &branch);
        mock.set_branch(&branch, &c1);
        mock.set_proposal_state(number, ProposalState::Open);
    }
    let state = load_state(&sub.state_dir);
    assert_eq!(state.stack_number, None);
    assert!(!state.stack_link_pending);

    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");
    let outcome = stacked_share(&mock, &sub).await;
    match outcome {
        ProposeOutcome::Updated(report) => {
            assert_eq!(report.number, 41, "the oldest layer, updated in place");
            assert_eq!(report.stack_number, None);
            assert_eq!(report.stack_position, None);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
    // The capability probe still runs (it is a property of the origin), but
    // no stack is ever created, extended or dissolved over legacy residue.
    let calls = mock.calls();
    assert!(
        !calls.iter().any(|c| c.starts_with("create_stack")
            || c.starts_with("extend_stack")
            || c.starts_with("dissolve_stack")),
        "{calls:?}"
    );
    assert_eq!(load_state(&sub.state_dir).proposals.len(), 2);
}

#[tokio::test]
async fn a_failing_probe_falls_back_without_caching_a_verdict() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    mock.fail_list_stacks();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;
    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");

    let outcome = stacked_share(&mock, &sub).await;
    assert!(
        matches!(outcome, ProposeOutcome::Proposed(_)),
        "a broken probe never fails the share: {outcome:?}"
    );
    assert_eq!(
        load_state(&sub.state_dir).stacks_available,
        None,
        "a transport failure is not a verdict worth remembering"
    );

    // The next share asks again rather than trusting a cache it never wrote.
    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    stacked_share(&mock, &sub).await;
    assert_eq!(
        mock.calls().iter().filter(|c| *c == "list_stacks").count(),
        2,
        "{:?}",
        mock.calls()
    );
}

// A layer records its delta against the chain tip - the trunk snapshot with
// every open layer's own files laid over it - not against the trunk. Without
// that, every layer re-proposes what the layers below it already carry, a
// share with nothing new opens an empty layer, and a delete of a file only a
// lower layer holds is invisible.

#[tokio::test]
async fn a_zero_delta_share_on_an_open_chain_is_nothing_to_share() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, _first) = stacked_bottom_layer(&mock).await;

    // Nothing was touched since the layer went up: the working tree IS the
    // chain tip, so there is no second layer to open.
    let before = mock.calls().len();
    let outcome = stacked_share(&mock, &sub).await;
    assert!(
        matches!(outcome, ProposeOutcome::NothingToShare { .. }),
        "expected NothingToShare, got {outcome:?}"
    );
    assert_eq!(
        load_state(&sub.state_dir).proposals.len(),
        1,
        "no empty layer was opened"
    );
    let delta = mock.calls().split_off(before);
    assert!(
        !delta.iter().any(|c| c.starts_with("create_stack")
            || c.starts_with("extend_stack")
            || c.starts_with("dissolve_stack")
            || c.starts_with("create_proposal")),
        "{delta:?}"
    );
}

#[tokio::test]
async fn a_layer_records_only_its_own_delta_against_the_tip() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, _first) = stacked_bottom_layer(&mock).await;

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(&mock, &sub).await);

    // The bottom layer's own change to notes/a.md is part of the tip now, so
    // it is not this layer's work and never appears in it.
    assert_eq!(second.added, vec!["notes/b.md".to_string()]);
    assert!(second.updated.is_empty(), "{:?}", second.updated);
    assert!(second.deleted.is_empty(), "{:?}", second.deleted);

    let state = load_state(&sub.state_dir);
    let files = &state.proposals[1].files;
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].path, "notes/b.md");
    assert_eq!(files[0].change, ProposedChange::Added);
    assert_eq!(
        files[0].size,
        Some(b"beta\n".len() as u64),
        "the size travels with the record so the tip stamp is exact"
    );
}

#[tokio::test]
async fn a_layer_can_delete_a_file_the_layer_below_added() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // The bottom layer adds a file the trunk has never seen.
    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let first = proposed(stacked_share(&mock, &sub).await);
    let first_head = mock.branch_commit(&first.branch).expect("the layer's head");
    assert!(
        mock.commit_tree(&first_head)
            .unwrap()
            .contains_key("notes/b.md")
    );

    // Retiring it again is a change against the tip, invisible against the
    // trunk: the trunk never carried the file at all.
    std::fs::remove_file(sub.domain_root.join("notes/b.md")).unwrap();
    let second = proposed(stacked_share(&mock, &sub).await);

    assert_eq!(second.deleted, vec!["notes/b.md".to_string()]);
    assert!(second.added.is_empty(), "{:?}", second.added);
    assert!(second.updated.is_empty(), "{:?}", second.updated);

    let state = load_state(&sub.state_dir);
    let files = &state.proposals[1].files;
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].change, ProposedChange::Deleted);

    // The layer's own tree drops the file the layer below it added, so the
    // diff a reviewer reads is exactly that deletion.
    let second_head = mock.branch_commit(&second.branch).expect("the new head");
    assert!(
        !mock
            .commit_tree(&second_head)
            .unwrap()
            .contains_key("notes/b.md"),
        "the layer's tree still carries the retired file"
    );
    assert_eq!(mock.commit_parents(&second_head), Some(vec![first_head]));
}

#[tokio::test]
async fn preview_stacks_on_the_surviving_layer_when_the_top_ref_is_gone() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, first) = stacked_bottom_layer(&mock).await;
    let top_title = load_state(&sub.state_dir).proposals[0].title.clone();

    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let second = proposed(stacked_share(&mock, &sub).await);

    // The top layer's branch is gone: a real share settles that record and
    // stacks onto the layer below, so a preview has to say the same.
    mock.delete_branch(&spec(), &second.branch).await.unwrap();

    let before = mock.calls().len();
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: true,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        plan.action,
        PlannedAction::StackOnTop {
            top_number: first.number,
            top_title,
        }
    );
    // The settled layer's files leave the tip with it, so its content reads
    // as this share's work again - the same list a real share would carry.
    let mut paths: Vec<&str> = plan.changes.changes.iter().map(|c| c.path()).collect();
    paths.sort();
    assert_eq!(paths, vec!["notes/c.md"], "{paths:?}");
    let delta = mock.calls().split_off(before);
    assert!(
        !delta.iter().any(|c| is_write_call(c)),
        "a preview writes nothing: {delta:?}"
    );
}

#[tokio::test]
async fn preview_of_a_diverged_top_still_measures_against_the_tip() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, _first) = stacked_bottom_layer(&mock).await;

    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(&mock, &sub).await);

    // A reviewer moves the top layer's branch: the share would refuse, but
    // the change list it reports is still the chain's own delta, not the
    // whole chain re-read against the trunk.
    let foreign = mock.add_commit(commit_files(&[("MANIFEST.md", b"# reviewer")]), None);
    mock.set_branch(&second.branch, &foreign);

    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: None,
            stacks_allowed: true,
        },
    )
    .await
    .unwrap();

    assert!(
        matches!(plan.action, PlannedAction::ProposalDiverged { .. }),
        "{:?}",
        plan.action
    );
    let mut paths: Vec<&str> = plan.changes.changes.iter().map(|c| c.path()).collect();
    paths.sort();
    assert_eq!(
        paths,
        vec!["notes/c.md"],
        "the layers below already carry the rest: {paths:?}"
    );
}

// Amending a named layer: the share lands on that layer's own branch instead
// of opening a new one, and every layer above it is replayed onto the amended
// head so the chain heals itself.

/// Shares with an explicit layer to amend, the call every amend scenario
/// makes.
async fn amend_share(mock: &MockProvider, sub: &Subscribed, number: u64) -> ProposeOutcome {
    propose(
        mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: Some(number),
            stacks_allowed: true,
        },
    )
    .await
    .expect("an amend should succeed")
}

/// The report of a share that updated a proposal, or a panic naming what came
/// back instead.
fn updated(outcome: ProposeOutcome) -> crystalline_remote::ops::ProposeReport {
    match outcome {
        ProposeOutcome::Updated(report) => report,
        other => panic!("expected Updated, got {other:?}"),
    }
}

/// A three-layer chain over one subscribed domain: `notes/a.md` refined by the
/// bottom layer, `notes/b.md` added by the middle one and `notes/c.md` by the
/// top one. Returns the layers bottom first.
async fn stacked_three_layers(
    mock: &MockProvider,
) -> (Subscribed, Vec<crystalline_remote::ops::ProposeReport>) {
    let (sub, first) = stacked_bottom_layer(mock).await;
    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(mock, &sub).await);
    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let third = proposed(stacked_share(mock, &sub).await);
    (sub, vec![first, second, third])
}

#[tokio::test]
async fn amending_the_bottom_layer_cascades_force_replays_above() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, layers) = stacked_three_layers(&mock).await;
    let (first, second, third) = (&layers[0], &layers[1], &layers[2]);
    let old_bottom_head = mock.branch_commit(&first.branch).expect("the bottom head");

    write(&sub.domain_root.join("notes/a.md"), b"alpha v3\n");
    let before = mock.calls().len();
    let report = updated(amend_share(&mock, &sub, first.number).await);
    assert_eq!(report.number, first.number, "the named layer, amended");
    assert_eq!(
        report.stack_position,
        Some((1, 3)),
        "the amended layer's own position"
    );

    let delta = mock.calls().split_off(before);
    let new_bottom = mock.branch_commit(&first.branch).expect("the new head");
    let new_middle = mock.branch_commit(&second.branch).expect("the new head");
    let new_top = mock.branch_commit(&third.branch).expect("the new head");

    // The amended layer moves fast-forward; every layer above it is re-based
    // and forced.
    assert!(
        delta.contains(&format!(
            "update_branch:{}:{new_bottom}:force=false",
            first.branch
        )),
        "{delta:?}"
    );
    assert!(
        delta.contains(&format!(
            "update_branch:{}:{new_middle}:force=true",
            second.branch
        )),
        "{delta:?}"
    );
    assert!(
        delta.contains(&format!(
            "update_branch:{}:{new_top}:force=true",
            third.branch
        )),
        "{delta:?}"
    );

    // The chain stays linear, every layer re-parented onto the one below it.
    assert_eq!(
        mock.commit_parents(&new_bottom),
        Some(vec![old_bottom_head])
    );
    assert_eq!(
        mock.commit_parents(&new_middle),
        Some(vec![new_bottom.clone()])
    );
    assert_eq!(
        mock.commit_parents(&new_top),
        Some(vec![new_middle.clone()])
    );

    // Trees are snapshots, so the top carries every layer's file - with the
    // amended content - while each layer's own tree carries only its own work.
    let bottom_tree = mock.commit_tree(&new_bottom).expect("the amended tree");
    assert_eq!(
        bottom_tree.get("notes/a.md").map(Vec::as_slice),
        Some(b"alpha v3\n".as_slice())
    );
    assert!(
        !bottom_tree.contains_key("notes/b.md") && !bottom_tree.contains_key("notes/c.md"),
        "the amended layer swallowed the layers above it: {:?}",
        bottom_tree.keys().collect::<Vec<_>>()
    );
    let top_tree = mock.commit_tree(&new_top).expect("the replayed tree");
    assert_eq!(
        top_tree.get("notes/a.md").map(Vec::as_slice),
        Some(b"alpha v3\n".as_slice())
    );
    assert_eq!(
        top_tree.get("notes/b.md").map(Vec::as_slice),
        Some(b"beta\n".as_slice())
    );
    assert_eq!(
        top_tree.get("notes/c.md").map(Vec::as_slice),
        Some(b"gamma\n".as_slice()),
        "a replayed layer's own content is untouched"
    );

    let state = load_state(&sub.state_dir);
    assert_eq!(state.proposals.len(), 3);
    for (record, head) in state
        .proposals
        .iter()
        .zip([&new_bottom, &new_middle, &new_top])
    {
        assert_eq!(record.status, ProposalStatus::Open);
        assert_eq!(record.head_commit.as_deref(), Some(head.as_str()));
        assert_eq!(
            record.pending_head_commit, None,
            "every layer's push finished"
        );
    }

    // An amend changes no base ref, so the stack's membership holds and no
    // stack call is made at all.
    assert!(
        !delta.iter().any(|c| c.starts_with("create_stack")
            || c.starts_with("extend_stack")
            || c.starts_with("dissolve_stack")),
        "{delta:?}"
    );
}

#[tokio::test]
async fn amending_a_stacked_layer_never_writes_a_merge_commit() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, first) = stacked_bottom_layer(&mock).await;
    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let _second = proposed(stacked_share(&mock, &sub).await);

    // The trunk moves on under the open chain.
    let trunk = load_state(&sub.state_dir).base_commit;
    let c2 = mock.add_commit(
        commit_files(&[
            ("MANIFEST.md", b"# Manifest"),
            ("notes/a.md", b"alpha\n"),
            ("notes/upstream.md", b"news\n"),
        ]),
        Some(&trunk),
    );
    mock.set_branch("main", &c2);

    write(&sub.domain_root.join("notes/a.md"), b"alpha v3\n");
    let report = updated(amend_share(&mock, &sub, first.number).await);
    assert_eq!(report.number, first.number);

    let state = load_state(&sub.state_dir);
    assert_eq!(state.base_commit, c2, "the amend pulled the trunk in first");
    let new_bottom = mock.branch_commit(&first.branch).expect("the new head");
    assert_eq!(
        mock.commit_parents(&new_bottom).map(|p| p.len()),
        Some(1),
        "a stacked layer is always a single-parent commit"
    );
    // The advanced trunk arrives through the tree the layer is built on, not
    // through a merge commit.
    let tree = mock.commit_tree(&new_bottom).expect("the amended tree");
    assert!(
        tree.contains_key("notes/upstream.md"),
        "{:?}",
        tree.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        tree.get("notes/a.md").map(Vec::as_slice),
        Some(b"alpha v3\n".as_slice())
    );
}

#[tokio::test]
async fn amend_refuses_a_number_that_is_not_an_open_layer() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, first) = stacked_bottom_layer(&mock).await;
    write(&sub.domain_root.join("notes/b.md"), b"beta\n");
    let second = proposed(stacked_share(&mock, &sub).await);

    write(&sub.domain_root.join("notes/c.md"), b"gamma\n");
    let before = mock.calls().len();
    let err = propose(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: Some(999),
            stacks_allowed: true,
        },
    )
    .await
    .expect_err("999 is not an open layer");
    match &err {
        crystalline_remote::RemoteError::State(message) => {
            assert!(message.contains("999"), "{message}");
            assert!(
                message.contains(&format!("#{} (layer 1)", first.number)),
                "{message}"
            );
            assert!(
                message.contains(&format!("#{} (layer 2)", second.number)),
                "{message}"
            );
        }
        other => panic!("expected a State error, got {other:?}"),
    }
    let delta = mock.calls().split_off(before);
    assert!(
        !delta.iter().any(|c| is_write_call(c)),
        "a refused amend writes nothing: {delta:?}"
    );
}

#[tokio::test]
async fn amend_preview_counts_the_layers_above() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let (sub, layers) = stacked_three_layers(&mock).await;
    let middle = &layers[1];

    // A preview needs work to describe: an amend carrying nothing new is
    // NothingToShare, exactly as a stacking share is.
    write(&sub.domain_root.join("notes/b.md"), b"beta v2\n");
    let before = mock.calls().len();
    let plan = propose_preview(
        &mock,
        &spec(),
        &sub.domain_root,
        "eng",
        &sub.state_dir,
        ShareOptions {
            title: None,
            description: None,
            proposal: Some(middle.number),
            stacks_allowed: true,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        plan.action,
        PlannedAction::Amend {
            number: middle.number,
            url: middle.url.clone(),
            layers_above: 1,
        }
    );
    let paths: Vec<&str> = plan.changes.changes.iter().map(|c| c.path()).collect();
    assert_eq!(paths, vec!["notes/b.md"], "{paths:?}");
    let delta = mock.calls().split_off(before);
    assert!(
        !delta.iter().any(|c| is_write_call(c)),
        "a preview writes nothing: {delta:?}"
    );
}

#[tokio::test]
async fn a_deleted_file_replays_only_where_it_exists_below() {
    let mock = MockProvider::new();
    mock.enable_stacks();
    let c1 = mock.add_commit(
        commit_files(&[("MANIFEST.md", b"# Manifest"), ("notes/a.md", b"alpha\n")]),
        None,
    );
    let (sub, _) = subscribe_at(&mock, &c1).await;

    // The bottom layer adds a file the trunk never had; the layer above
    // retires it again.
    write(&sub.domain_root.join("notes/a.md"), b"alpha v2\n");
    write(&sub.domain_root.join("notes/x.md"), b"ex\n");
    let first = proposed(stacked_share(&mock, &sub).await);
    std::fs::remove_file(sub.domain_root.join("notes/x.md")).unwrap();
    let second = proposed(stacked_share(&mock, &sub).await);

    // A retirement of a path nothing below ever carried, crafted by hand: the
    // replay has to drop it rather than write a deletion into the tree.
    {
        let mut state = load_state(&sub.state_dir);
        state.proposals[1].files.push(ProposedFile {
            path: "notes/ghost.md".to_string(),
            change: ProposedChange::Deleted,
            sha256: None,
            blob_sha: None,
            size: None,
        });
        state.save(&sub.state_dir).unwrap();
    }

    write(&sub.domain_root.join("notes/a.md"), b"alpha v3\n");
    let before = mock.calls().len();
    updated(amend_share(&mock, &sub, first.number).await);
    let delta = mock.calls().split_off(before);

    // Two trees: the amended layer's own and the one replay above it.
    let trees: Vec<&str> = delta
        .iter()
        .filter_map(|c| c.strip_prefix("create_tree:"))
        .collect();
    assert_eq!(trees.len(), 2, "{delta:?}");
    let writes = mock.tree_writes(trees[1]).expect("the replayed tree");
    assert!(
        writes
            .iter()
            .any(|(path, blob)| path == "notes/x.md" && blob.is_none()),
        "a retirement of a file the layer below still adds is replayed: {writes:?}"
    );
    assert!(
        !writes.iter().any(|(path, _)| path == "notes/ghost.md"),
        "a retirement of a path nothing below carries is dropped: {writes:?}"
    );

    // And it lands: the amended layer still adds the file, the layer above
    // still retires it.
    let bottom = mock.branch_commit(&first.branch).expect("the new head");
    let top = mock.branch_commit(&second.branch).expect("the new head");
    assert!(
        mock.commit_tree(&bottom)
            .expect("the amended tree")
            .contains_key("notes/x.md"),
        "the amended layer lost the file it adds"
    );
    assert!(
        !mock
            .commit_tree(&top)
            .expect("the replayed tree")
            .contains_key("notes/x.md")
    );
}
