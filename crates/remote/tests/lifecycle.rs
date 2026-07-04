//! End-to-end lifecycle tests for the pull-side and share-side orchestration
//! in `crystalline_remote::ops` (`subscribe`, `pull`, `status`, `propose`,
//! `discard`, `resolve`), driven by an in-memory forge
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
    ProposeOutcome, PullReport, Resolution, SubscribeReport, discard, propose, pull, resolve,
    status, subscribe,
};
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

// --- share-side: propose, discard, resolve ------------------------------------

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
        None,
        None,
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
            },
            ProposedFile {
                path: "notes/edit.md".to_string(),
                change: ProposedChange::Modified,
                sha256: Some(sha256_hex(b"after\n")),
            },
            ProposedFile {
                path: "notes/gone.md".to_string(),
                change: ProposedChange::Deleted,
                sha256: None,
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
        None,
        None,
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
        None,
        None,
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
        None,
        None,
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
        None,
        None,
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
        None,
        None,
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

// Scenario 21 (g): discard. A declined proposal touching three files: one
// verbatim (restored to its base content), one diverged since sharing
// (skipped, untouched) and one proposed addition (deleted). The record
// lands in history with its declined status preserved.

#[tokio::test]
async fn scenario_21_discard_restores_verbatim_deletes_added_skips_diverged() {
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
            },
            ProposedFile {
                path: "notes/diverge.md".to_string(),
                change: ProposedChange::Modified,
                sha256: Some(sha256_hex(b"shared diverge v2\n")),
            },
            ProposedFile {
                path: "notes/added.md".to_string(),
                change: ProposedChange::Added,
                sha256: Some(sha256_hex(b"newly added\n")),
            },
        ],
    });
    state.save(&sub.state_dir).unwrap();

    // The user edits notes/diverge.md again after sharing it.
    write(
        &sub.domain_root.join("notes/diverge.md"),
        b"further edited after sharing\n",
    );

    let report = discard(&sub.domain_root, &sub.state_dir, 9).unwrap();
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
    assert_eq!(st.history[0].status, ProposalStatus::Declined);
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
        None,
        None,
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
        None,
        None,
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
        None,
        None,
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
        Some("My own title"),
        Some("My own description, written by hand."),
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
