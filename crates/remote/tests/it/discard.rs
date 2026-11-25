//! The provider-free half of "see what changed and discard": resolving a
//! path against the detected delta, reading both sides, and putting one path
//! back the way the base has it. No forge, no network: everything a test
//! here needs is a temp folder, a base snapshot and the state file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crystalline_remote::changes::{
    LocalChange, LocalChanges, detect_local_changes, detect_local_changes_against,
};
use crystalline_remote::ops::{
    ChangeSides, DiscardBase, DiscardRefusal, DiscardReport, DiscardTarget, TipSources,
    Unrestorable, discard_local_files, local_change_sides, resolve_local_change, tip_stamps,
    unshared_base, unshared_sources,
};
use crystalline_remote::state::{
    BaseStamp, OriginState, Proposal, ProposalStatus, ProposedChange, ProposedFile, write_base_file,
};
use sha2::{Digest, Sha256};

const MANIFEST: &str = "---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# kb\n\n## Scope\n\n- kb\n\n## When to Use\n\n- kb\n";

/// The digest a stamp records, encoded the way `state::sha256_hex` encodes
/// it. Written out by byte because this sha2 hands back an array that does
/// not format as hex on its own.
fn hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A domain root and an origin state dir whose base snapshot holds exactly
/// `base_files`, both on disk under `base/` and as stamps in `state.json`.
struct Tree {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    state_dir: PathBuf,
}

fn tree(base_files: &[(&str, &[u8])], working_files: &[(&str, &[u8])]) -> Tree {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kb");
    let state_dir = tmp.path().join("origins").join("kb");
    std::fs::create_dir_all(&root).unwrap();
    let mut state = OriginState::new("acme/kb", "main");
    for (rel, bytes) in base_files {
        write_base_file(&state_dir, rel, bytes).unwrap();
        state.files.insert(
            rel.to_string(),
            BaseStamp {
                sha256: hex(bytes),
                size: bytes.len() as u64,
            },
        );
    }
    state.save(&state_dir).unwrap();
    for (rel, bytes) in working_files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    Tree {
        _tmp: tmp,
        root,
        state_dir,
    }
}

fn base_of(t: &Tree) -> BTreeMap<String, BaseStamp> {
    unshared_base(&OriginState::load(&t.state_dir).unwrap().unwrap())
}

fn sources_of(t: &Tree) -> TipSources {
    unshared_sources(&OriginState::load(&t.state_dir).unwrap().unwrap())
}

/// No fetch was made, so nothing failed.
static NOTHING_FAILED: BTreeMap<String, Unrestorable> = BTreeMap::new();

fn no_fetch(sources: &TipSources) -> DiscardBase<'_> {
    DiscardBase {
        sources,
        unrestorable: &NOTHING_FAILED,
    }
}

fn detect(t: &Tree) -> LocalChanges {
    detect_local_changes(&t.root, &base_of(t)).unwrap()
}

fn target(path: &str, bytes: Option<&[u8]>) -> DiscardTarget {
    DiscardTarget {
        path: path.to_string(),
        sha256: bytes.map(hex),
    }
}

fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

#[test]
fn a_modification_is_restored_to_the_base_copy() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"old\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"new\n")],
    );
    let local = detect(&t);
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &no_fetch(&sources_of(&t)),
        &local,
        &[target("a.md", Some(b"new\n"))],
    )
    .unwrap();
    assert_eq!(report.restored, vec!["a.md".to_string()]);
    assert!(
        report.deleted.is_empty() && report.refused.is_empty(),
        "{report:?}"
    );
    assert_eq!(read(&t.root.join("a.md")), Some(b"old\n".to_vec()));
}

#[test]
fn an_addition_is_deleted_and_a_deletion_is_restored() {
    let t = tree(
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("gone.md", b"kept by the team\n"),
        ],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("new.md", b"mine\n")],
    );
    let local = detect(&t);
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &no_fetch(&sources_of(&t)),
        &local,
        &[target("new.md", Some(b"mine\n")), target("gone.md", None)],
    )
    .unwrap();
    assert_eq!(report.deleted, vec!["new.md".to_string()]);
    assert_eq!(report.restored, vec!["gone.md".to_string()]);
    assert!(!t.root.join("new.md").exists());
    assert_eq!(
        read(&t.root.join("gone.md")),
        Some(b"kept by the team\n".to_vec())
    );
}

#[test]
fn a_side_that_moved_since_the_caller_looked_is_refused_and_the_others_proceed() {
    let t = tree(
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("a.md", b"old\n"),
            ("gone.md", b"g\n"),
        ],
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("a.md", b"new\n"),
            ("new.md", b"n\n"),
        ],
    );
    let local = detect(&t);
    // The caller looked at earlier bytes of every kind: a modification and an
    // addition whose digests no longer match, and a deletion whose file came
    // back. None of them is touched; the one that still matches is.
    std::fs::write(t.root.join("gone.md"), b"back again\n").unwrap();
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &no_fetch(&sources_of(&t)),
        &local,
        &[
            target("a.md", Some(b"stale\n")),
            target("new.md", Some(b"stale\n")),
            target("gone.md", None),
            target("MANIFEST.md", Some(MANIFEST.as_bytes())),
        ],
    )
    .unwrap();
    let refused: Vec<(&str, DiscardRefusal)> = report
        .refused
        .iter()
        .map(|(p, r)| (p.as_str(), *r))
        .collect();
    assert_eq!(
        refused,
        vec![
            ("a.md", DiscardRefusal::ChangedSince),
            ("new.md", DiscardRefusal::ChangedSince),
            ("gone.md", DiscardRefusal::ChangedSince),
            ("MANIFEST.md", DiscardRefusal::NotAChange),
        ]
    );
    assert_eq!(read(&t.root.join("a.md")), Some(b"new\n".to_vec()));
    assert!(t.root.join("new.md").exists());
    assert_eq!(
        read(&t.root.join("gone.md")),
        Some(b"back again\n".to_vec())
    );
}

#[test]
fn a_layer_path_with_no_copy_is_refused_with_its_detail() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes())],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("layer.md", b"v2\n")],
    );
    let mut record = layer(1, ProposalStatus::Open, &[added("layer.md", b"v1\n")]);
    with_layers(&t, Some(true), vec![record.clone()]);
    let sources = sources_of(&t);
    let local = detect_as_the_engine_does(&t, &base_of(&t));
    assert!(matches!(
        resolve_local_change(&local, "layer.md"),
        Some(LocalChange::Modified { .. })
    ));
    let sides = local_change_sides(&t.root, &t.state_dir, &sources, &local, "layer.md")
        .unwrap()
        .unwrap();
    assert_eq!(sides.base, None, "no copy here, and sides never fetch");
    assert_eq!(sides.current, Some(b"v2\n".to_vec()));

    // A blob to fetch it by, and no fetch made.
    let report = discard_as_a_caller_would(&t, &[target("layer.md", Some(b"v2\n"))]);
    assert_eq!(
        report.refused,
        vec![(
            "layer.md".to_string(),
            DiscardRefusal::NoBaseCopy(Unrestorable::NotFetched)
        )]
    );
    assert_eq!(report.refused[0].1.code(), "no_base_copy");
    assert_eq!(report.refused[0].1.detail(), Some("not_fetched"));

    // A fetch that failed names itself.
    let failed = BTreeMap::from([(hex(b"v1\n"), Unrestorable::FetchFailed)]);
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &DiscardBase {
            sources: &sources,
            unrestorable: &failed,
        },
        &local,
        &[target("layer.md", Some(b"v2\n"))],
    )
    .unwrap();
    assert_eq!(report.refused[0].1.detail(), Some("fetch_failed"));

    // A record from before blob shas were kept has nothing to fetch.
    record.files[0].blob_sha = None;
    with_layers(&t, Some(true), vec![record]);
    let report = discard_as_a_caller_would(&t, &[target("layer.md", Some(b"v2\n"))]);
    assert_eq!(
        report.refused,
        vec![(
            "layer.md".to_string(),
            DiscardRefusal::NoBaseCopy(Unrestorable::NoBlobRecorded)
        )]
    );
    assert_eq!(report.refused[0].1.detail(), Some("no_blob_recorded"));
    assert_eq!(
        read(&t.root.join("layer.md")),
        Some(b"v2\n".to_vec()),
        "nothing written"
    );
}

#[test]
fn a_trunk_path_whose_base_copy_is_gone_is_refused_with_no_detail() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v2\n")],
    );
    std::fs::remove_file(t.state_dir.join("base").join("a.md")).unwrap();
    let report = discard_as_a_caller_would(&t, &[target("a.md", Some(b"v2\n"))]);
    assert_eq!(
        report.refused,
        vec![(
            "a.md".to_string(),
            DiscardRefusal::NoBaseCopy(Unrestorable::BaseMissing)
        )]
    );
    assert_eq!(report.refused[0].1.detail(), None);
    assert_eq!(read(&t.root.join("a.md")), Some(b"v2\n".to_vec()));
}

#[test]
fn a_generated_listing_is_never_resolved_listed_or_discarded_even_where_shared() {
    // The policy is a frontmatter key, so it goes beside the other ones.
    let manifest = MANIFEST.replace(
        "status: stable",
        "status: stable\ngenerated_indexes: shared",
    );
    let t = tree(
        &[
            ("MANIFEST.md", manifest.as_bytes()),
            ("index.md", b"# kb\n\n- old\n"),
        ],
        &[
            ("MANIFEST.md", manifest.as_bytes()),
            ("index.md", b"# kb\n\n- new\n"),
            ("a.md", b"a\n"),
        ],
    );
    let local = detect(&t);
    assert!(
        local.changes.iter().any(|c| c.is_generated_index()),
        "detection still sees it: {local:?}"
    );
    assert!(local.substantive().all(|c| !c.is_generated_index()));
    assert!(resolve_local_change(&local, "index.md").is_none());
    assert!(
        local_change_sides(&t.root, &t.state_dir, &sources_of(&t), &local, "index.md")
            .unwrap()
            .is_none()
    );
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &no_fetch(&sources_of(&t)),
        &local,
        &[target("index.md", Some(b"# kb\n\n- new\n"))],
    )
    .unwrap();
    assert_eq!(
        report.refused,
        vec![("index.md".to_string(), DiscardRefusal::UnknownPath)]
    );
    assert_eq!(
        read(&t.root.join("index.md")),
        Some(b"# kb\n\n- new\n".to_vec())
    );
}

#[test]
fn an_unknown_path_is_refused_by_name() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes())],
        &[("MANIFEST.md", MANIFEST.as_bytes())],
    );
    let local = detect(&t);
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &no_fetch(&sources_of(&t)),
        &local,
        &[target("nowhere.md", None)],
    )
    .unwrap();
    assert_eq!(
        report.refused,
        vec![("nowhere.md".to_string(), DiscardRefusal::UnknownPath)]
    );
    assert!(
        local_change_sides(&t.root, &t.state_dir, &sources_of(&t), &local, "nowhere.md")
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_case_only_rename_is_resolved_and_restored_through_the_disk_spelling() {
    let t = tree(
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("Notes/A.md", b"old\n"),
        ],
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("notes/a.md", b"new\n"),
        ],
    );
    let local = detect(&t);
    // Reported at the base spelling, openable at the disk one; both select it.
    let reported = resolve_local_change(&local, "notes/a.md")
        .unwrap()
        .path()
        .to_string();
    assert_eq!(reported, "Notes/A.md");
    assert_eq!(
        resolve_local_change(&local, "Notes/A.md").unwrap().path(),
        "Notes/A.md"
    );
    let sides = local_change_sides(&t.root, &t.state_dir, &sources_of(&t), &local, "notes/a.md")
        .unwrap()
        .unwrap();
    assert_eq!(sides.current, Some(b"new\n".to_vec()));
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &no_fetch(&sources_of(&t)),
        &local,
        &[target("notes/a.md", Some(b"new\n"))],
    )
    .unwrap();
    assert_eq!(report.restored, vec!["Notes/A.md".to_string()]);
    // Written where the file is, never a second copy at the base spelling
    // beside it.
    let on_disk = local.disk_path("Notes/A.md");
    assert_eq!(read(&t.root.join(on_disk)), Some(b"old\n".to_vec()));
    let mut copies = 0;
    for entry in walkdir::WalkDir::new(&t.root).into_iter().flatten() {
        if entry.file_type().is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("a.md")
        {
            copies += 1;
        }
    }
    assert_eq!(copies, 1, "one file under either spelling");
}

#[test]
fn sides_of_every_kind() {
    let t = tree(
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("m.md", b"old\n"),
            ("d.md", b"gone\n"),
        ],
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("m.md", b"new\n"),
            ("a.md", b"added\n"),
        ],
    );
    let local = detect(&t);
    let m = local_change_sides(&t.root, &t.state_dir, &sources_of(&t), &local, "m.md")
        .unwrap()
        .unwrap();
    assert_eq!(
        (m.base, m.current),
        (Some(b"old\n".to_vec()), Some(b"new\n".to_vec()))
    );
    let a = local_change_sides(&t.root, &t.state_dir, &sources_of(&t), &local, "a.md")
        .unwrap()
        .unwrap();
    assert_eq!((a.base, a.current), (None, Some(b"added\n".to_vec())));
    let d = local_change_sides(&t.root, &t.state_dir, &sources_of(&t), &local, "d.md")
        .unwrap()
        .unwrap();
    assert_eq!((d.base, d.current), (Some(b"gone\n".to_vec()), None));
    assert!(matches!(d.change, LocalChange::Deleted { .. }));
}

// A discard on a stacked chain: the base a change is measured against is the
// chain tip, so the content a discard puts back is the tip's too. Every test
// below builds the chain by hand in `state.json`, places each layer's local
// copy under `proposed/<sha256>` with plain `std::fs`, and discards the way
// the engine does: the base from the state, detection against it, then the
// discard.

/// One file of a hand-built layer: `bytes` is its shared content, `None` for
/// a deletion.
struct LayerFile<'a> {
    path: &'a str,
    change: ProposedChange,
    bytes: Option<&'a [u8]>,
}

fn added<'a>(path: &'a str, bytes: &'a [u8]) -> LayerFile<'a> {
    LayerFile {
        path,
        change: ProposedChange::Added,
        bytes: Some(bytes),
    }
}

fn modified<'a>(path: &'a str, bytes: &'a [u8]) -> LayerFile<'a> {
    LayerFile {
        path,
        change: ProposedChange::Modified,
        bytes: Some(bytes),
    }
}

/// A proposal record for `files`, carrying a digest, a blob sha and a size
/// for every file with content, the shape every share since 0.17.0 records.
fn layer(number: u64, status: ProposalStatus, files: &[LayerFile<'_>]) -> Proposal {
    Proposal {
        number,
        url: format!("https://example.test/pull/{number}"),
        branch: format!("crystalline/share-{number}"),
        title: format!("Layer {number}"),
        created_at: crystalline_core::time::now(),
        status,
        files: files
            .iter()
            .map(|f| ProposedFile {
                path: f.path.to_string(),
                change: f.change,
                sha256: f.bytes.map(hex),
                blob_sha: f.bytes.map(hex),
                size: f.bytes.map(|b| b.len() as u64),
            })
            .collect(),
        head_commit: None,
        pending_head_commit: None,
        base_commit: None,
        review_state: None,
        feedback: Vec::new(),
        updated_at: None,
        author_login: None,
    }
}

/// Lays `layers` over the saved state of `t`, bottom first, with
/// `stacks_available` as given. Two or more open layers get a stack number,
/// since only a chain this machine knows as a stack is on the stacked path.
fn with_layers(t: &Tree, stacks_available: Option<bool>, layers: Vec<Proposal>) {
    let mut state = OriginState::load(&t.state_dir).unwrap().unwrap();
    state.stacks_available = stacks_available;
    let open = layers
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .count();
    if open > 1 {
        state.stack_number = Some(99);
    }
    state.proposals = layers;
    state.save(&t.state_dir).unwrap();
}

/// Places `bytes` as a layer's local copy, named by its digest.
fn place_copy(t: &Tree, bytes: &[u8]) {
    place_copy_named(t, &hex(bytes), bytes);
}

/// Places `bytes` under the copy name `name`, which a test may pick to not
/// match the bytes.
fn place_copy_named(t: &Tree, name: &str, bytes: &[u8]) {
    let dir = t.state_dir.join("proposed");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
}

/// Detection the way the engine runs it: against the base and the state dir,
/// so the recorded LF digests take part exactly as they do there.
fn detect_as_the_engine_does(t: &Tree, base: &BTreeMap<String, BaseStamp>) -> LocalChanges {
    detect_local_changes_against(
        &t.root,
        base,
        &t.state_dir,
        crystalline_core::ReflectionMode::Off,
    )
    .unwrap()
}

/// A discard of `targets` the way the engine makes one.
fn discard_as_a_caller_would(t: &Tree, targets: &[DiscardTarget]) -> DiscardReport {
    let sources = sources_of(t);
    let local = detect_as_the_engine_does(t, &tip_stamps(&sources));
    discard_local_files(&t.root, &t.state_dir, &no_fetch(&sources), &local, targets).unwrap()
}

/// Both sides of `raw` the way the engine reads them.
fn sides_as_a_caller_would(t: &Tree, raw: &str) -> Option<ChangeSides> {
    let sources = sources_of(t);
    let local = detect_as_the_engine_does(t, &tip_stamps(&sources));
    local_change_sides(&t.root, &t.state_dir, &sources, &local, raw).unwrap()
}

/// The paths detection reports as changed now.
fn changed_paths(t: &Tree) -> Vec<String> {
    let base = base_of(t);
    detect_as_the_engine_does(t, &base)
        .substantive()
        .map(|c| c.path().to_string())
        .collect()
}

#[test]
fn a_layer_that_added_a_path_edited_again_is_restored_to_the_layer_content() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes())],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("l.md", b"v2\n")],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(1, ProposalStatus::Open, &[added("l.md", b"v1\n")])],
    );
    place_copy(&t, b"v1\n");
    let report = discard_as_a_caller_would(&t, &[target("l.md", Some(b"v2\n"))]);
    assert_eq!(report.restored, vec!["l.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("l.md")), Some(b"v1\n".to_vec()));
    assert!(changed_paths(&t).is_empty(), "{:?}", changed_paths(&t));
}

#[test]
fn a_layer_that_modified_a_trunk_path_edited_again_is_restored_to_the_layer_not_the_trunk() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v3\n")],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(1, ProposalStatus::Open, &[modified("a.md", b"v2\n")])],
    );
    place_copy(&t, b"v2\n");
    let report = discard_as_a_caller_would(&t, &[target("a.md", Some(b"v3\n"))]);
    assert_eq!(report.restored, vec!["a.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("a.md")), Some(b"v2\n".to_vec()));
    assert!(changed_paths(&t).is_empty(), "{:?}", changed_paths(&t));
}

#[test]
fn a_deleted_file_a_layer_added_is_restored_from_the_layer() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes())],
        &[("MANIFEST.md", MANIFEST.as_bytes())],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(1, ProposalStatus::Open, &[added("l.md", b"v1\n")])],
    );
    place_copy(&t, b"v1\n");
    let report = discard_as_a_caller_would(&t, &[target("l.md", None)]);
    assert_eq!(report.restored, vec!["l.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("l.md")), Some(b"v1\n".to_vec()));
    assert!(changed_paths(&t).is_empty(), "{:?}", changed_paths(&t));
}

#[test]
fn a_deleted_file_a_layer_modified_is_restored_from_the_layer() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes())],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(1, ProposalStatus::Open, &[modified("a.md", b"v2\n")])],
    );
    place_copy(&t, b"v2\n");
    let report = discard_as_a_caller_would(&t, &[target("a.md", None)]);
    assert_eq!(report.restored, vec!["a.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("a.md")), Some(b"v2\n".to_vec()));
    assert!(changed_paths(&t).is_empty(), "{:?}", changed_paths(&t));
}

#[test]
fn the_top_record_wins_when_two_layers_carry_the_path() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v4\n")],
    );
    with_layers(
        &t,
        Some(true),
        vec![
            layer(1, ProposalStatus::Open, &[modified("a.md", b"v2\n")]),
            layer(2, ProposalStatus::Open, &[modified("a.md", b"v3\n")]),
        ],
    );
    place_copy(&t, b"v2\n");
    place_copy(&t, b"v3\n");
    let report = discard_as_a_caller_would(&t, &[target("a.md", Some(b"v4\n"))]);
    assert_eq!(report.restored, vec!["a.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("a.md")), Some(b"v3\n".to_vec()));
    assert!(changed_paths(&t).is_empty(), "{:?}", changed_paths(&t));
}

#[test]
fn a_merged_record_still_standing_is_restored_from_its_copy() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v3\n")],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(
            1,
            ProposalStatus::Merged,
            &[modified("a.md", b"v2\n")],
        )],
    );
    place_copy(&t, b"v2\n");
    let report = discard_as_a_caller_would(&t, &[target("a.md", Some(b"v3\n"))]);
    assert_eq!(report.restored, vec!["a.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("a.md")), Some(b"v2\n".to_vec()));
}

#[test]
fn a_declined_record_is_not_the_base() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v3\n")],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(
            1,
            ProposalStatus::Declined,
            &[modified("a.md", b"v2\n")],
        )],
    );
    place_copy(&t, b"v2\n");
    let report = discard_as_a_caller_would(&t, &[target("a.md", Some(b"v3\n"))]);
    assert_eq!(report.restored, vec!["a.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("a.md")), Some(b"v1\n".to_vec()));
}

#[test]
fn off_the_stacked_path_the_trunk_copy_is_restored() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v3\n")],
    );
    with_layers(
        &t,
        None,
        vec![layer(1, ProposalStatus::Open, &[modified("a.md", b"v2\n")])],
    );
    place_copy(&t, b"v2\n");
    let report = discard_as_a_caller_would(&t, &[target("a.md", Some(b"v3\n"))]);
    assert_eq!(report.restored, vec!["a.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("a.md")), Some(b"v1\n".to_vec()));
}

#[test]
fn a_layer_copy_that_fails_its_digest_is_not_written() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v3\n")],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(1, ProposalStatus::Open, &[modified("a.md", b"v2\n")])],
    );
    place_copy_named(&t, &hex(b"v2\n"), b"tampered\n");
    let report = discard_as_a_caller_would(&t, &[target("a.md", Some(b"v3\n"))]);
    assert!(report.restored.is_empty(), "{report:?}");
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert!(
        matches!(report.refused[0].1, DiscardRefusal::NoBaseCopy(_)),
        "{report:?}"
    );
    assert_eq!(read(&t.root.join("a.md")), Some(b"v3\n".to_vec()));
}

#[test]
fn a_crlf_layer_copy_is_restored_byte_for_byte() {
    let crlf: &[u8] = b"a\r\nb\r\n";
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes())],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("c.md", b"x\n")],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(1, ProposalStatus::Open, &[added("c.md", crlf)])],
    );
    place_copy(&t, crlf);
    let report = discard_as_a_caller_would(&t, &[target("c.md", Some(b"x\n"))]);
    assert_eq!(report.restored, vec!["c.md".to_string()], "{report:?}");
    assert_eq!(read(&t.root.join("c.md")), Some(crlf.to_vec()));
    assert!(changed_paths(&t).is_empty(), "{:?}", changed_paths(&t));
}

#[test]
fn sides_of_a_layer_path_show_the_layer_content_as_base() {
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("a.md", b"v1\n")],
        &[
            ("MANIFEST.md", MANIFEST.as_bytes()),
            ("a.md", b"v3\n"),
            ("l.md", b"l2\n"),
        ],
    );
    with_layers(
        &t,
        Some(true),
        vec![layer(
            1,
            ProposalStatus::Open,
            &[modified("a.md", b"v2\n"), added("l.md", b"l1\n")],
        )],
    );
    place_copy(&t, b"v2\n");
    place_copy(&t, b"l1\n");
    let a = sides_as_a_caller_would(&t, "a.md").unwrap();
    assert_eq!(
        (a.base, a.current),
        (Some(b"v2\n".to_vec()), Some(b"v3\n".to_vec()))
    );
    let l = sides_as_a_caller_would(&t, "l.md").unwrap();
    assert_eq!(
        (l.base, l.current),
        (Some(b"l1\n".to_vec()), Some(b"l2\n".to_vec()))
    );
}
