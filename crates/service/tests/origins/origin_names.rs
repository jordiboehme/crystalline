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

/// Ruling 2026-09-26 (option A): a team domain never gets `domain_name`
/// written into its MANIFEST automatically, whatever `name_origin` is. An
/// explicit name at connect lands in the config only; the MANIFEST that
/// declares none stays exactly as the team has it, with no pending local
/// change and no proposal.
#[tokio::test]
async fn an_explicit_name_at_connect_leaves_the_manifest_untouched() {
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
    assert_eq!(text, String::from_utf8(bare_manifest()).unwrap(), "{text}");
    assert_eq!(
        crystalline_core::manifest::domain_name_of_source(&text).as_deref(),
        None,
        "the owner adds the name upstream by hand: {text}"
    );
    assert_eq!(
        change_paths(&r.eng, "platform").await,
        Vec::<String>::new(),
        "no pending local change"
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

/// Ruling 2026-09-26 (option A): a team domain never gets `domain_name`
/// written into its MANIFEST automatically, whatever its name's origin. So
/// an explicit name at connect leaves the MANIFEST exactly as the team has
/// it - no pending local change - and review mode is available immediately,
/// with nothing to discard first.
#[tokio::test]
async fn review_mode_right_after_a_named_connect_starts_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    let r = rig(tmp.path(), mock, &[]).await;
    r.eng
        .origin_add(REPO, Some("platform"), None, None, None)
        .await
        .unwrap();

    let changes = r
        .eng
        .local_changes("platform", &ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(
        changes["changes"].as_array().map(Vec::len),
        Some(0),
        "an explicit name leaves no pending change: {changes}"
    );

    r.eng
        .set_review_mode(
            "platform",
            Some(crystalline_core::config::ReviewMode::Overlay),
            crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .expect("nothing pending to block review mode");
}

/// Ruling 2026-09-26 (option A) resolved what used to be a pinned conflict
/// here: with no automatic write-back, nothing of ours sits over the
/// MANIFEST's frontmatter after a named connect, so a later upstream edit to
/// it pulls in clean instead of conflicting.
#[tokio::test]
async fn an_upstream_manifest_edit_after_a_named_connect_pulls_cleanly() {
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
    assert_eq!(
        pulled["domains"][0]["conflicts"].as_array().map(Vec::len),
        Some(0),
        "{pulled}"
    );
    let manifest =
        std::fs::read_to_string(r.domains_root.join("platform").join("MANIFEST.md")).unwrap();
    assert!(manifest.contains("sharing: direct"), "{manifest}");
    assert!(!manifest.contains("domain_name"), "{manifest}");
}

/// The same repository and branch already connected here under an explicit
/// name that equals the declared one is this checkout: a nameless connect
/// answers the connected state instead of stepping to `eng-2` and
/// downloading the repository a second time.
#[tokio::test]
async fn a_nameless_connect_finds_the_repo_connected_under_its_declared_name_explicitly() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", declaring_manifest())]);
    let r = rig(tmp.path(), mock.clone(), &[]).await;

    let named = r
        .eng
        .origin_add(REPO, Some("eng"), None, None, None)
        .await
        .unwrap();
    assert_eq!(named["domain"], "eng", "{named}");
    assert_eq!(named["name_origin"], "explicit", "{named}");

    let nameless = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(nameless["domain"], "eng", "{nameless}");
    assert_eq!(nameless["already_connected"], true, "{nameless}");
    assert_eq!(
        mock.tarball_calls(),
        1,
        "one download, not a second checkout"
    );
    let names: Vec<String> = on_disk(&r.config_path).domains.keys().cloned().collect();
    assert_eq!(names, vec!["eng".to_string()]);
}

/// A nameless connect that adopts a registration keeps the `name_origin` it
/// already records: a name somebody chose on purpose stays `explicit`, so it
/// never becomes eligible for an automatic rename - and the repository's
/// default is still not written into the team's MANIFEST.
#[tokio::test]
async fn a_nameless_adoption_keeps_an_explicit_name_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    let local_root = tmp.path().join("kb");
    let local = local_domain(&local_root, &bare_manifest())
        .with_name_origin(crystalline_core::config::NameOrigin::Explicit);
    let r = rig(tmp.path(), mock, &[("eng-knowledge", local)]).await;

    let result = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();

    assert_eq!(result["domain"], "eng-knowledge", "{result}");
    assert_eq!(result["adopted"], true, "{result}");
    assert_eq!(result["name_origin"], "explicit", "{result}");
    let entry = on_disk(&r.config_path).domains["eng-knowledge"].clone();
    assert_eq!(
        entry.name_origin,
        Some(crystalline_core::config::NameOrigin::Explicit)
    );
    assert!(entry.origin.is_some());
    assert_eq!(
        std::fs::read(local_root.join("MANIFEST.md")).unwrap(),
        bare_manifest(),
        "a nameless connect writes no name back, whatever the entry records"
    );
}

/// A local refusal of the repository's own name is reported as itself when
/// the forge cannot be asked, instead of the network error that would send
/// the person to the wrong fix.
#[tokio::test]
async fn a_local_refusal_of_the_repo_name_is_reported_when_the_forge_cannot_be_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    mock.fail_default_branch();
    let r = rig(
        tmp.path(),
        mock.clone(),
        &[("eng-knowledge", DomainEntry::virtual_domain())],
    )
    .await;

    let err = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::engine::EngineError::Conflict(_)),
        "{err}"
    );
    assert!(err.to_string().contains("virtual domain"), "{err}");
    assert!(mock.read_file_calls().is_empty());
}

/// The same refusal is moot when the forge answers with a declared name of
/// its own: the repository's name is never used, so nothing refuses.
#[tokio::test]
async fn a_declared_name_makes_a_refused_repo_name_moot() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", declaring_manifest())]);
    let r = rig(
        tmp.path(),
        mock,
        &[("eng-knowledge", DomainEntry::virtual_domain())],
    )
    .await;

    let result = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(result["domain"], "eng", "{result}");
}

// --- adoption after a pull (Task 17) ------------------------------------------

/// The owner adds `domain_name: eng` upstream: the pull lands it, and once
/// the pull has finished the derived team domain is renamed to `eng` on this
/// machine, its repository name kept as an alias. The pull report is the
/// pull's own, under the name the pull ran with.
#[tokio::test]
async fn a_pulled_domain_name_renames_a_derived_team_domain_after_the_pull() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = forge(&[("MANIFEST.md", bare_manifest())]);
    let r = rig(tmp.path(), mock.clone(), &[]).await;
    let connected = r
        .eng
        .origin_add(REPO, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(connected["domain"], "eng-knowledge", "{connected}");

    let declared = mock.add_commit(
        [("MANIFEST.md".to_string(), declaring_manifest())]
            .into_iter()
            .collect(),
    );
    mock.set_branch("main", &declared);
    let pulled = r
        .eng
        .origin_update(
            Some("eng-knowledge"),
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();

    assert_eq!(pulled["domains"][0]["domain"], "eng-knowledge", "{pulled}");
    assert_eq!(
        pulled["domains"][0]["applied"],
        serde_json::json!(["MANIFEST.md"]),
        "{pulled}"
    );
    let cfg = on_disk(&r.config_path);
    assert!(!cfg.domains.contains_key("eng-knowledge"));
    let entry = &cfg.domains["eng"];
    assert_eq!(
        entry.name_origin,
        Some(crystalline_core::config::NameOrigin::Derived)
    );
    assert!(
        entry.aliases.contains(&"eng-knowledge".to_string()),
        "{entry:?}"
    );
    assert_eq!(entry.canonical_seen.as_deref(), Some("eng"));
    assert!(tmp.path().join("origins").join("eng").is_dir());
    assert!(change_paths(&r.eng, "eng").await.is_empty());
}
