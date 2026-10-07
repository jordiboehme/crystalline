//! The provider-free half of "see what changed and discard": resolving a
//! path against the detected delta, reading both sides, and putting one path
//! back the way the base has it. No forge, no network: everything a test
//! here needs is a temp folder, a base snapshot and the state file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crystalline_remote::changes::{LocalChange, LocalChanges, detect_local_changes};
use crystalline_remote::ops::{
    DiscardRefusal, DiscardTarget, discard_local_files, local_change_sides, resolve_local_change,
    unshared_base,
};
use crystalline_remote::state::{BaseStamp, OriginState, write_base_file};
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
        &base_of(&t),
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
        &base_of(&t),
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
        &base_of(&t),
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
fn a_path_with_no_base_copy_is_refused_without_a_provider() {
    // A stamp for a path the base tree holds no bytes for: the shape a lower
    // open stacked layer leaves behind through `tip_files_over`.
    let t = tree(
        &[("MANIFEST.md", MANIFEST.as_bytes())],
        &[("MANIFEST.md", MANIFEST.as_bytes()), ("layer.md", b"v2\n")],
    );
    let mut base = base_of(&t);
    base.insert(
        "layer.md".to_string(),
        BaseStamp {
            sha256: hex(b"v1\n"),
            size: 3,
        },
    );
    let local = detect_local_changes(&t.root, &base).unwrap();
    assert!(matches!(
        resolve_local_change(&local, "layer.md"),
        Some(LocalChange::Modified { .. })
    ));
    let sides = local_change_sides(&t.root, &t.state_dir, &local, "layer.md")
        .unwrap()
        .unwrap();
    assert_eq!(sides.base, None, "the trunk has no copy");
    assert_eq!(sides.current, Some(b"v2\n".to_vec()));
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &base,
        &local,
        &[target("layer.md", Some(b"v2\n"))],
    )
    .unwrap();
    assert_eq!(
        report.refused,
        vec![("layer.md".to_string(), DiscardRefusal::NoBaseCopy)]
    );
    assert_eq!(
        read(&t.root.join("layer.md")),
        Some(b"v2\n".to_vec()),
        "nothing written"
    );
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
        local_change_sides(&t.root, &t.state_dir, &local, "index.md")
            .unwrap()
            .is_none()
    );
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &base_of(&t),
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
        &base_of(&t),
        &local,
        &[target("nowhere.md", None)],
    )
    .unwrap();
    assert_eq!(
        report.refused,
        vec![("nowhere.md".to_string(), DiscardRefusal::UnknownPath)]
    );
    assert!(
        local_change_sides(&t.root, &t.state_dir, &local, "nowhere.md")
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
    let sides = local_change_sides(&t.root, &t.state_dir, &local, "notes/a.md")
        .unwrap()
        .unwrap();
    assert_eq!(sides.current, Some(b"new\n".to_vec()));
    let report = discard_local_files(
        &t.root,
        &t.state_dir,
        &base_of(&t),
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
    let m = local_change_sides(&t.root, &t.state_dir, &local, "m.md")
        .unwrap()
        .unwrap();
    assert_eq!(
        (m.base, m.current),
        (Some(b"old\n".to_vec()), Some(b"new\n".to_vec()))
    );
    let a = local_change_sides(&t.root, &t.state_dir, &local, "a.md")
        .unwrap()
        .unwrap();
    assert_eq!((a.base, a.current), (None, Some(b"added\n".to_vec())));
    let d = local_change_sides(&t.root, &t.state_dir, &local, "d.md")
        .unwrap()
        .unwrap();
    assert_eq!((d.base, d.current), (Some(b"gone\n".to_vec()), None));
    assert!(matches!(d.change, LocalChange::Deleted { .. }));
}
