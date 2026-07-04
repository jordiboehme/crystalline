//! End-to-end lifecycle tests for the pull-side orchestration in
//! `crystalline_remote::ops`, driven by an in-memory forge ([`mock::MockProvider`])
//! rather than a live GitHub. Each test is a scenario over throwaway tempdirs:
//! subscribe a domain, move the mock forge, pull and assert what landed on
//! disk and in the origin state.
//!
//! The mock is a faithful stand-in for the read side of a forge: a fake commit
//! graph with parent links, per-branch ETags that bump on every branch move,
//! a compare computed from two commit snapshots, blobs addressed by content
//! hash, tarballs wrapped in the single top-level directory GitHub uses and a
//! settable proposal registry. It never reaches the network and never panics
//! on an injected fault (a garbage-collected base commit, a forced truncation).

mod mock;

use std::collections::BTreeMap;
use std::path::Path;

use crystalline_remote::ops::{PullReport, SubscribeReport, pull, status, subscribe};
use crystalline_remote::provider::{OriginSpec, ProposalState};
use crystalline_remote::state::{
    BaseStamp, OriginState, Proposal, ProposalStatus, ProposedChange, ProposedFile,
    read_conflict_files,
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
        }],
    });
    st.save(state_dir).unwrap();
}

// Scenario 1: subscribe lays down the working tree, the base snapshot and the
// origin state; a missing MANIFEST is refused without touching the target; a
// non-empty target is refused.

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
async fn scenario_01_subscribe_into_a_non_empty_directory_is_refused() {
    let mock = MockProvider::new();
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", b"# Manifest")]), None);
    mock.set_branch("main", &c1);

    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let domain_root = work.path().join("domain");
    write(&domain_root.join("pre-existing.md"), b"already here");
    let state_dir = state.path().join("origin");

    let err = subscribe(&mock, &spec(), &domain_root, &state_dir)
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_remote::RemoteError::State(_)),
        "{err:?}"
    );
    // The pre-existing file is left alone.
    assert_eq!(read(&domain_root.join("pre-existing.md")), b"already here");
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
    let status_report = status(&spec(), &sub.domain_root, &sub.state_dir, None)
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
    let status_report = status(&spec(), &sub.domain_root, &sub.state_dir, None)
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
    let offline = status(&spec(), &sub.domain_root, &sub.state_dir, None)
        .await
        .unwrap();
    assert_eq!(offline.behind, None);
    assert_eq!(offline.repo, "team/knowledge");
    assert_eq!(offline.branch, "main");
    assert_eq!(offline.base_commit, c1);

    // Online, branch unmoved: not behind.
    let online_unmoved = status(&spec(), &sub.domain_root, &sub.state_dir, Some(&mock))
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

    let online_moved = status(&spec(), &sub.domain_root, &sub.state_dir, Some(&mock))
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
