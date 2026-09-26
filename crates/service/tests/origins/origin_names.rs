//! How a team domain gets its name when it is connected: an explicit name
//! wins, else the `domain_name` the repository's MANIFEST declares (read
//! through the forge before anything is downloaded), else the repository's
//! own name. A taken MANIFEST name steps to `<name>-2`; an explicit or
//! repository-default name keeps adopting an origin-less domain in place.
//!
//! Every test injects `crate::support::MockProvider`, whose `read_file`
//! answers from the same commit tree the download then lands on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::support::MockProvider;
use crystalline_core::config::{DomainEntry, GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::engine::ShareActor;
use tokio::sync::Mutex;

const REPO: &str = "acme/eng-knowledge";

fn manifest_body(extra: &str) -> Vec<u8> {
    format!(
        "---\ntype: manifest\ntitle: Engineering\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n{extra}---\n\n# Engineering\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n"
    )
    .into_bytes()
}

/// A MANIFEST declaring `domain_name: eng`.
fn declaring_manifest() -> Vec<u8> {
    manifest_body("domain_name: eng\n")
}

/// A MANIFEST that declares no name at all.
fn bare_manifest() -> Vec<u8> {
    manifest_body("")
}

fn engram(title: &str, permalink: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody of {title}\n"
    )
    .into_bytes()
}

/// A forge holding one commit on `main` with `files`.
fn forge(files: &[(&str, Vec<u8>)]) -> Arc<MockProvider> {
    let mock = Arc::new(MockProvider::new());
    let tree: BTreeMap<String, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (p.to_string(), c.clone()))
        .collect();
    let commit = mock.add_commit(tree);
    mock.set_branch("main", &commit);
    mock
}

/// Everything a test needs: the engine, the config file it persists to and
/// the domains root a folderless connect lands under.
struct Rig {
    eng: Engine,
    config_path: PathBuf,
    domains_root: PathBuf,
}

/// An engine with GitHub enabled, `domains_root` under `tmp`, the mock
/// provider injected and `pre` already registered.
async fn rig(tmp: &Path, mock: Arc<MockProvider>, pre: &[(&str, DomainEntry)]) -> Rig {
    let config_path = tmp.join("config.yaml");
    let domains_root = tmp.join("domains");
    let mut cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        domains_root: Some(domains_root.clone()),
        ..GlobalConfig::default()
    };
    for (name, entry) in pre {
        cfg.domains.insert(name.to_string(), entry.clone());
    }
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    )
    .with_origin_provider(mock)
    .with_origins_dir(tmp.join("origins"))
    .with_state_dir(tmp.to_path_buf());
    Rig {
        eng,
        config_path,
        domains_root,
    }
}

/// An origin-less local file domain at `root`, with a MANIFEST of its own.
fn local_domain(root: &Path, manifest: &[u8]) -> DomainEntry {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest).unwrap();
    DomainEntry::file(root.to_path_buf())
}

fn on_disk(config_path: &Path) -> GlobalConfig {
    crystalline_core::config::load_yaml(config_path).unwrap()
}

/// The paths of `domain`'s unshared changes.
async fn change_paths(eng: &Engine, domain: &str) -> Vec<String> {
    let listed = eng.local_changes(domain, &ShareActor::Owner).await.unwrap();
    listed["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_nameless_connect_takes_the_name_the_manifest_declares() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[
        ("MANIFEST.md", declaring_manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha")),
    ]);
    let r = rig(tmp.path(), mock.clone(), &[]).await;

    let result = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "eng", "{result}");
    assert_eq!(result["name_origin"], "derived", "{result}");
    assert_eq!(result["canonical_name"], "eng", "{result}");
    assert_eq!(result["shadowed"], false, "{result}");
    let root = r.domains_root.join("eng");
    assert_eq!(result["root"], root.display().to_string());
    assert_eq!(
        mock.read_file_calls(),
        vec![("main".to_string(), "MANIFEST.md".to_string())],
        "the MANIFEST is read on the branch the connect tracks"
    );

    let entry = on_disk(&r.config_path).domains["eng"].clone();
    assert_eq!(
        entry.name_origin,
        Some(crystalline_core::config::NameOrigin::Derived)
    );
    assert!(entry.origin.is_some());
    // The MANIFEST already names the domain: nothing is written back, so the
    // team's file lands and stays byte for byte as the repository holds it.
    assert_eq!(
        std::fs::read(root.join("MANIFEST.md")).unwrap(),
        declaring_manifest()
    );
    assert!(change_paths(&r.eng, "eng").await.is_empty());
}

#[tokio::test]
async fn a_nameless_connect_reads_the_manifest_under_the_subpath() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[
        ("team/eng/MANIFEST.md", declaring_manifest()),
        ("team/eng/notes/alpha.md", engram("Alpha", "alpha")),
    ]);
    let r = rig(tmp.path(), mock.clone(), &[]).await;

    let result = r
        .eng
        .origin_add(REPO, None, Some("team/eng/"), None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "eng", "{result}");
    assert_eq!(
        mock.read_file_calls(),
        vec![("main".to_string(), "team/eng/MANIFEST.md".to_string())]
    );
}

/// Open issue 4, the MANIFEST branch: a declared name that is already taken
/// steps to `-2` and never adopts the domain holding it.
#[tokio::test]
async fn a_taken_manifest_name_steps_and_never_adopts_the_holder() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", declaring_manifest())]);
    let local_root = tmp.path().join("my-eng");
    let local = local_domain(&local_root, &bare_manifest());
    let r = rig(tmp.path(), mock, &[("eng", local.clone())]).await;

    let result = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "eng-2", "{result}");
    assert_eq!(result["name_origin"], "derived", "{result}");
    assert_eq!(result["canonical_name"], "eng", "{result}");
    assert_eq!(result["shadowed"], true, "{result}");
    assert!(
        result["note"]
            .as_str()
            .unwrap()
            .contains("'eng' is already a domain here"),
        "{result}"
    );
    assert_eq!(result["adopted"], false, "{result}");
    assert_eq!(
        result["root"],
        r.domains_root.join("eng-2").display().to_string()
    );

    let cfg = on_disk(&r.config_path);
    assert_eq!(
        cfg.domains["eng"], local,
        "the origin-less domain holding the name is left exactly as it was"
    );
    assert!(cfg.domains["eng-2"].origin.is_some());
}

/// Open issue 4, the repository-default branch: no declared name falls back
/// to the repository's own name, which is never written into the team's
/// MANIFEST.
#[tokio::test]
async fn a_nameless_connect_without_a_declared_name_uses_the_repo_name() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    let r = rig(tmp.path(), mock, &[]).await;

    let result = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "eng-knowledge", "{result}");
    assert_eq!(result["name_origin"], "derived", "{result}");
    let root = r.domains_root.join("eng-knowledge");
    assert_eq!(
        std::fs::read(root.join("MANIFEST.md")).unwrap(),
        bare_manifest(),
        "a defaulted repository name is never written back"
    );
    assert!(change_paths(&r.eng, "eng-knowledge").await.is_empty());
}

/// Open issue 4, repository-default branch, today's behaviour: an
/// origin-less domain already holding the repository's name is adopted in
/// place, keeping what was decided about it.
#[tokio::test]
async fn the_repo_default_name_still_adopts_an_origin_less_domain_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[
        ("MANIFEST.md", bare_manifest()),
        ("notes/beta.md", engram("Beta", "beta")),
    ]);
    let local_root = tmp.path().join("kb");
    let mut local = local_domain(&local_root, &bare_manifest());
    local.aliases = vec!["old-kb".to_string()];
    let r = rig(tmp.path(), mock, &[("eng-knowledge", local)]).await;

    let result = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "eng-knowledge", "{result}");
    assert_eq!(result["adopted"], true, "{result}");
    assert_eq!(result["root"], local_root.display().to_string());
    let entry = on_disk(&r.config_path).domains["eng-knowledge"].clone();
    assert_eq!(entry.origin.as_ref().unwrap().repo, REPO);
    assert_eq!(
        entry.aliases,
        vec!["old-kb".to_string()],
        "adopting in place keeps the domain's own aliases"
    );
    assert_eq!(
        entry.name_origin,
        Some(crystalline_core::config::NameOrigin::Derived)
    );
    assert!(local_root.join("notes/beta.md").exists());
    assert_eq!(
        std::fs::read(local_root.join("MANIFEST.md")).unwrap(),
        bare_manifest(),
        "the repository default is not written back on adoption either"
    );
}

/// An explicit name is written into a MANIFEST that declares none, as an
/// ordinary pending local change the next share carries - never a proposal
/// of its own.
#[tokio::test]
async fn an_explicit_name_is_written_back_as_a_pending_local_change() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    let r = rig(tmp.path(), mock.clone(), &[]).await;

    let result = r
        .eng
        .origin_add(REPO, Some("platform"), None, None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "platform", "{result}");
    assert_eq!(result["name_origin"], "explicit", "{result}");
    assert!(
        mock.read_file_calls().is_empty(),
        "a given name needs no MANIFEST read"
    );
    let root = r.domains_root.join("platform");
    let text = std::fs::read_to_string(root.join("MANIFEST.md")).unwrap();
    assert_eq!(
        crystalline_core::manifest::domain_name_of_source(&text).as_deref(),
        Some("platform"),
        "{text}"
    );
    assert_eq!(
        change_paths(&r.eng, "platform").await,
        vec!["MANIFEST.md".to_string()]
    );
    assert!(
        !mock
            .calls()
            .iter()
            .any(|c| c.starts_with("create_proposal")),
        "no proposal: {:?}",
        mock.calls()
    );
}

/// A MANIFEST that cannot be read up front is no reason to refuse the
/// connect: the repository's name stands in.
#[tokio::test]
async fn a_failed_manifest_read_falls_back_to_the_repo_name() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", declaring_manifest())]);
    mock.fail_read_file();
    let r = rig(tmp.path(), mock, &[]).await;

    let result = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "eng-knowledge", "{result}");
    assert_eq!(result["name_origin"], "derived", "{result}");
}

/// A retry of a nameless connect that already landed answers the connected
/// state: the declared name being taken by the connect itself must not step
/// it to a second copy.
#[tokio::test]
async fn a_nameless_retry_of_a_declaring_repo_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", declaring_manifest())]);
    let r = rig(tmp.path(), mock.clone(), &[]).await;

    let first = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(first["domain"], "eng", "{first}");
    let reads = mock.read_file_calls().len();

    let second = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(second["domain"], "eng", "{second}");
    assert_eq!(second["already_connected"], true, "{second}");
    assert_eq!(
        mock.read_file_calls().len(),
        reads,
        "the retry answers before it asks the forge anything"
    );

    // Even with the forge unreadable, the retry finds the connection rather
    // than connecting a second copy under the repository's name.
    mock.fail_read_file();
    let third = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(third["domain"], "eng", "{third}");
    assert_eq!(third["already_connected"], true, "{third}");

    let names: Vec<String> = on_disk(&r.config_path).domains.keys().cloned().collect();
    assert_eq!(names, vec!["eng".to_string()]);
}

/// The same, after the declared name had to step: the retry finds `eng-2`
/// rather than stepping on to `eng-3`.
#[tokio::test]
async fn a_nameless_retry_after_a_step_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", declaring_manifest())]);
    let local = local_domain(&tmp.path().join("my-eng"), &bare_manifest());
    let r = rig(tmp.path(), mock, &[("eng", local)]).await;

    let first = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(first["domain"], "eng-2", "{first}");
    let second = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(second["domain"], "eng-2", "{second}");
    assert_eq!(second["already_connected"], true, "{second}");
    let mut names: Vec<String> = on_disk(&r.config_path).domains.keys().cloned().collect();
    names.sort();
    assert_eq!(names, vec!["eng".to_string(), "eng-2".to_string()]);
}

/// The retry lookup only ever answers for a domain an earlier nameless
/// connect made: a domain somebody connected to the same repository under a
/// name of their own, on another branch, is not this connect repeated.
#[tokio::test]
async fn a_nameless_connect_is_not_answered_by_an_explicitly_named_connection() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", declaring_manifest())]);
    // The staging branch declares no name, so the explicitly named domain on
    // it does not also claim `eng` as its canonical name.
    let staging = mock.add_commit(
        [("MANIFEST.md".to_string(), bare_manifest())]
            .into_iter()
            .collect(),
    );
    mock.set_branch("staging", &staging);
    let r = rig(tmp.path(), mock, &[]).await;

    let named = r
        .eng
        .origin_add(REPO, Some("eng-staging"), None, Some("staging"), None)
        .await
        .unwrap();
    assert_eq!(named["domain"], "eng-staging", "{named}");

    let nameless = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(nameless["domain"], "eng", "{nameless}");
    assert!(nameless["already_connected"].is_null(), "{nameless}");
    let entry = on_disk(&r.config_path).domains["eng"].clone();
    assert_eq!(entry.origin.unwrap().branch(), "main");
}

/// Pinned consequence of the explicit name's write-back, awaiting a ruling:
/// the MANIFEST change it leaves is an unshared folder change, so turning on
/// review right after a named connect is refused until it is shared or
/// discarded.
#[tokio::test]
async fn review_mode_right_after_a_named_connect_waits_on_the_name_write_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    let r = rig(tmp.path(), mock, &[]).await;
    r.eng
        .origin_add(REPO, Some("platform"), None, None, None)
        .await
        .unwrap();

    let err = r
        .eng
        .set_review_mode(
            "platform",
            Some(crystalline_core::config::ReviewMode::Overlay),
            crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("MANIFEST.md"), "{err}");

    crate::support::discard_name_write_back(&r.eng, "platform", &r.domains_root.join("platform"))
        .await;
    r.eng
        .set_review_mode(
            "platform",
            Some(crystalline_core::config::ReviewMode::Overlay),
            crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("with the write-back put back, review starts");
}

/// Pinned consequence of the explicit name's write-back, awaiting a ruling:
/// the `domain_name` line sits in the MANIFEST's frontmatter where a team
/// edit to that frontmatter lands too, so a pull of such an edit before the
/// name was shared meets it as a conflict.
#[tokio::test]
async fn an_upstream_manifest_edit_before_the_name_is_shared_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    let r = rig(tmp.path(), mock.clone(), &[]).await;
    r.eng
        .origin_add(REPO, Some("platform"), None, None, None)
        .await
        .unwrap();

    let edited = mock.add_commit(
        [(
            "MANIFEST.md".to_string(),
            manifest_body("sharing: direct\n"),
        )]
        .into_iter()
        .collect(),
    );
    mock.set_branch("main", &edited);
    let pulled = r
        .eng
        .origin_update(Some("platform"), &crystalline_service::Scope::Unrestricted)
        .await
        .unwrap();
    let conflicts = pulled["domains"][0]["conflicts"].clone();
    assert_eq!(conflicts.as_array().map(Vec::len), Some(1), "{pulled}");
    assert_eq!(conflicts[0]["path"], "MANIFEST.md", "{pulled}");
}
