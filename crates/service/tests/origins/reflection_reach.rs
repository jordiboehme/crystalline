//! A Reflection page on a domain connected to GitHub is backed up to its
//! repository's `crystalline/reflections` branch, which every reader of that
//! repository on GitHub reads, with a Crystalline account or without one. So
//! with the GitHub App in place a page may only cite a domain whose
//! repository is read by every reader of the page's repository: a public
//! source repository, or one whose collaborators include every collaborator
//! of the page's. A listing nobody knows right now fails closed, and a
//! widening of the page repository's readers names the page in the queue
//! and asks a person who shared the source on purpose again.
//!
//! The forge is the one `origins::github_app` serves, with its collaborator
//! listing and repository visibility routes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crystalline_remote::reflections_backup::BACKUP_BRANCH;

use crystalline_service::engine::Invalidate;
use crystalline_service::params::{ReflectParams, WriteReflectionParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, ReflectionCaller, Scope};
use serde_json::{Value, json};

use super::github_app::{Forge, app_env, connect, engram, files, manifest, spawn_forge};
use crate::support::MockProvider;

/// An engram of `vault`.
const ROTA: &str = "crystalline://vault/team/rota";

const OVERVIEW: &str = "# Overview\n\nFor people.\n\n## Team\n\nFive people.\n";

/// The MANIFEST lines that switch the Reflection on and home it here.
const REFLECTING: &str = "reflection: on\nreflection_home: https://kb.example.com\n";

/// A repository whose `prefix` holds a MANIFEST that switches the
/// Reflection on and the engram `team/rota`.
fn tree(prefix: &str) -> BTreeMap<String, Vec<u8>> {
    files(&[
        (&format!("{prefix}MANIFEST.md"), manifest(REFLECTING)),
        (
            &format!("{prefix}team/rota.md"),
            engram("Rota", "team/rota", "Five people take turns."),
        ),
    ])
}

fn repo(trees: &[&str]) -> Arc<MockProvider> {
    let mock = Arc::new(MockProvider::new());
    let mut all = BTreeMap::new();
    for prefix in trees {
        all.extend(tree(prefix));
    }
    let head = mock.add_commit(all);
    mock.set_branch("main", &head);
    mock
}

/// An engine with an App pointed at the forge, `acme/kb` connected as `kb`
/// and `acme/vault` as `vault` (both shared here), and the accounts
/// resolver with the GitHub governor installed. The only account is the
/// instance admin `root`, linked to nobody on GitHub, so the accounts half
/// of the reach rule covers every pair and what a test sees is the
/// repository half.
async fn setup(tmp: &Path, env: &[(&str, &str)]) -> (Arc<Forge>, Arc<Engine>, Arc<AuthStore>) {
    let (forge, eng, auth, _) = setup_with_page_repo(tmp, env).await;
    (forge, eng, auth)
}

/// [`setup`], with the repository `acme/kb` as the forge holds it.
async fn setup_with_page_repo(
    tmp: &Path,
    env: &[(&str, &str)],
) -> (Arc<Forge>, Arc<Engine>, Arc<AuthStore>, Arc<MockProvider>) {
    let kb = repo(&[""]);
    let (forge, url) = spawn_forge(
        &[("acme/kb", kb.clone()), ("acme/vault", repo(&[""]))],
        &[("acme/kb", 1), ("acme/vault", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = Arc::new(super::github_app::engine(tmp, &url, env).await);
    connect(&eng, "acme/kb", "kb", &tmp.join("kb")).await;
    connect(&eng, "acme/vault", "vault", &tmp.join("vault")).await;
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    auth.add_user("root", "Root", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth.clone()).with_governor(eng.knowledge_governor()),
    ));
    (forge, eng, auth, kb)
}

/// Sets `repo`'s collaborators by GitHub id, each with read.
fn collaborators(forge: &Forge, repo: &str, ids: &[u64]) {
    forge.collaborators.lock().unwrap().insert(
        repo.to_string(),
        ids.iter()
            .map(|id| json!({"id": id, "login": format!("gh-{id}"), "role_name": "read"}))
            .collect(),
    );
}

/// The machine owner's agent, which reads every domain: what it is refused
/// is the reach rule's doing alone.
fn owner() -> ReflectionCaller {
    ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false)
}

/// `overview.md` in `domain` with its `Team` section built from `source`.
fn cite(domain: &str, source: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        domain: domain.into(),
        path: "overview.md".into(),
        markdown: Some(OVERVIEW.into()),
        sources: BTreeMap::from([("Team".to_string(), vec![source.to_string()])]),
        ..Default::default()
    }
}

async fn write(eng: &Engine, p: &WriteReflectionParams) -> Result<Value, String> {
    eng.write_reflection(p, &owner())
        .await
        .map_err(|e| e.to_string())
}

/// The `source_too_wide` items of `domain`'s queue.
async fn too_wide(eng: &Engine, domain: &str) -> Vec<Value> {
    let queue = eng
        .reflect(
            &ReflectParams {
                domains: vec![domain.into()],
                limit: Some(100),
                ..Default::default()
            },
            &owner(),
        )
        .await
        .unwrap();
    queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == "source_too_wide")
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_page_cannot_cite_a_domain_whose_repository_fewer_people_read() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1, 2]);
    collaborators(&forge, "acme/vault", &[1]);
    let err = write(&eng, &cite("kb", ROTA)).await.unwrap_err();
    assert!(err.contains("fewer people may read"), "{err}");
    // No login and no id of anybody is named.
    assert!(
        !err.contains("gh-2") && !err.contains("acme/vault"),
        "{err}"
    );
    assert!(!tmp.path().join("kb/reflections/overview.md").exists());
}

#[tokio::test]
async fn a_page_can_cite_a_domain_whose_repository_has_every_reader_of_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1, 2]);
    collaborators(&forge, "acme/vault", &[1, 2, 3]);
    write(&eng, &cite("kb", ROTA)).await.unwrap();
    // A public source repository is read by everybody.
    collaborators(&forge, "acme/vault", &[]);
    forge
        .public
        .lock()
        .unwrap()
        .insert("acme/vault".to_string());
    eng.invalidate_github_access(Invalidate::All);
    write(&eng, &cite("kb", ROTA)).await.unwrap();
    assert!(too_wide(&eng, "kb").await.is_empty());
}

#[tokio::test]
async fn a_public_repository_page_cannot_cite_a_private_repository_domain() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, auth) = setup(tmp.path(), &app_env()).await;
    // The page's domain is private here, so on the accounts side only the
    // instance admin reads it and the accounts half covers: what refuses
    // is the public backup branch.
    auth.set_domain_visibility("kb", true, "root")
        .await
        .unwrap();
    forge.public.lock().unwrap().insert("acme/kb".to_string());
    collaborators(&forge, "acme/kb", &[1]);
    collaborators(&forge, "acme/vault", &[1]);
    eng.invalidate_github_access(Invalidate::All);
    let err = write(&eng, &cite("kb", ROTA)).await.unwrap_err();
    assert!(err.contains("fewer people may read"), "{err}");
}

#[tokio::test]
async fn an_unknown_collaborator_listing_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1]);
    collaborators(&forge, "acme/vault", &[1]);
    write(&eng, &cite("kb", ROTA)).await.unwrap();
    // GitHub stops answering, and nothing old may serve.
    forge.collaborators_down.store(true, Ordering::SeqCst);
    eng.invalidate_github_access(Invalidate::All);
    let err = write(&eng, &cite("kb", ROTA)).await.unwrap_err();
    assert!(err.contains("fewer people may read"), "{err}");
    assert_eq!(too_wide(&eng, "kb").await.len(), 1);
    // The App removed from the source's repository closes it: the same.
    forge.collaborators_down.store(false, Ordering::SeqCst);
    forge.uninstall("acme/vault");
    eng.invalidate_github_access(Invalidate::All);
    let err = write(&eng, &cite("kb", ROTA)).await.unwrap_err();
    assert!(err.contains("fewer people may read"), "{err}");
}

#[tokio::test]
async fn a_new_collaborator_on_the_page_repository_raises_source_too_wide() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1]);
    collaborators(&forge, "acme/vault", &[1]);
    write(&eng, &cite("kb", ROTA)).await.unwrap();
    assert!(too_wide(&eng, "kb").await.is_empty());
    collaborators(&forge, "acme/kb", &[1, 2]);
    eng.invalidate_github_access(Invalidate::Repo("acme/kb".to_string()));
    let items = too_wide(&eng, "kb").await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["path"], "overview.md");
    // The repository made public: the same.
    collaborators(&forge, "acme/kb", &[1]);
    forge.public.lock().unwrap().insert("acme/kb".to_string());
    eng.invalidate_github_access(Invalidate::Repo("acme/kb".to_string()));
    assert_eq!(too_wide(&eng, "kb").await.len(), 1);
}

#[tokio::test]
async fn a_new_collaborator_on_the_page_repository_asks_a_confirmed_source_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1, 2]);
    collaborators(&forge, "acme/vault", &[1]);
    let mut p = cite("kb", ROTA);
    let err = write(&eng, &p).await.unwrap_err();
    assert!(err.contains("fewer people may read"), "{err}");
    // The owner shares it on purpose.
    let ask = eng
        .reflection_disclosure_question(&p, &owner())
        .await
        .unwrap()
        .expect("a question");
    assert!(
        ask.question
            .contains("the people who can read the GitHub repository acme/kb"),
        "{}",
        ask.question
    );
    assert!(!ask.question.contains("is not shared through GitHub"));
    p.disclosed = Some(ask.basis.clone());
    write(&eng, &p).await.unwrap();
    assert!(too_wide(&eng, "kb").await.is_empty());
    // A later edit by the same person needs no new question.
    assert!(
        eng.reflection_disclosure_question(&cite("kb", ROTA), &owner())
            .await
            .unwrap()
            .is_none()
    );

    // One more collaborator on the page's repository who may not read the
    // source: the confirmation was for the readers of that time.
    collaborators(&forge, "acme/kb", &[1, 2, 3]);
    eng.invalidate_github_access(Invalidate::Repo("acme/kb".to_string()));
    assert_eq!(too_wide(&eng, "kb").await.len(), 1);
    let again = eng
        .reflection_disclosure_question(&cite("kb", ROTA), &owner())
        .await
        .unwrap()
        .expect("asked again");
    assert_ne!(again.basis, ask.basis);
    // The old answer does not land the write.
    let mut stale = cite("kb", ROTA);
    stale.disclosed = Some(ask.basis);
    assert!(write(&eng, &stale).await.is_err());

    // A new collaborator who reads the source as well changes nothing.
    let mut p = cite("kb", ROTA);
    p.disclosed = Some(again.basis);
    write(&eng, &p).await.unwrap();
    collaborators(&forge, "acme/vault", &[1, 4]);
    collaborators(&forge, "acme/kb", &[1, 2, 3, 4]);
    eng.invalidate_github_access(Invalidate::All);
    assert!(too_wide(&eng, "kb").await.is_empty());
}

#[tokio::test]
async fn two_domains_on_one_repository_read_alike() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = spawn_forge(
        &[("acme/mono", repo(&["a/", "b/"]))],
        &[("acme/mono", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = Arc::new(super::github_app::engine(tmp.path(), &url, &app_env()).await);
    for d in ["a", "b"] {
        eng.origin_add(
            "acme/mono",
            Some(d),
            Some(d),
            None,
            Some(tmp.path().join(d).to_str().unwrap()),
            Some(crystalline_service::params::Readers::Shared),
        )
        .await
        .unwrap();
    }
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth).with_governor(eng.knowledge_governor()),
    ));
    collaborators(&forge, "acme/mono", &[1]);
    write(&eng, &cite("a", "crystalline://b/team/rota"))
        .await
        .unwrap();
    // Not even a listing is needed: one repository has one set of readers.
    forge.collaborators_down.store(true, Ordering::SeqCst);
    eng.invalidate_github_access(Invalidate::All);
    write(&eng, &cite("a", "crystalline://b/team/rota"))
        .await
        .unwrap();
}

#[tokio::test]
async fn without_an_app_the_old_rule_holds() {
    let tmp = tempfile::tempdir().unwrap();
    // The instance token alone: no App, so GitHub governs nothing.
    let (forge, eng, _) = setup(
        tmp.path(),
        &[(
            "CRYSTALLINE_GITHUB_TOKEN",
            super::github_app::INSTANCE_TOKEN,
        )],
    )
    .await;
    collaborators(&forge, "acme/kb", &[1, 2, 3]);
    collaborators(&forge, "acme/vault", &[1]);
    forge.public.lock().unwrap().insert("acme/kb".to_string());
    assert!(!eng.github_governs("kb"));
    // Any two connected domains count as alike, as before.
    write(&eng, &cite("kb", ROTA)).await.unwrap();
    assert!(too_wide(&eng, "kb").await.is_empty());
}

/// What the rule guards is the backup branch: a page that a new
/// collaborator on its repository makes too wide is dropped from the
/// branch head by the next recount and backup, and held back.
#[tokio::test]
async fn a_new_collaborator_drops_the_page_from_the_backup_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _, kb) = setup_with_page_repo(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1]);
    collaborators(&forge, "acme/vault", &[1]);
    write(&eng, &cite("kb", ROTA)).await.unwrap();
    let later = || Instant::now() + Duration::from_secs(120);
    eng.reflection_backup_tick(later()).await;
    let head = || kb.commit_paths(&kb.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(head().contains(&"reflections/overview.md".to_string()));

    collaborators(&forge, "acme/kb", &[1, 2]);
    eng.invalidate_github_access(Invalidate::Repo("acme/kb".to_string()));
    assert_eq!(
        eng.schedule_too_wide_backups().await,
        vec!["kb".to_string()]
    );
    eng.reflection_backup_tick(later()).await;
    assert!(
        !head().iter().any(|p| p.contains("overview.md")),
        "{:?}",
        head()
    );
    assert_eq!(
        eng.reflection_backup_held_back("kb"),
        vec!["overview.md".to_string()]
    );
}

/// What a confirmed write is refused with while nobody knows who reads the
/// page's repository.
const REPO_UNKNOWN: &str =
    "who reads this page's GitHub repository is not known right now; try again later";

/// A person's yes is recorded for the readers of the page's repository. When
/// they are not known, nothing can be recorded, so the yes is refused and
/// nothing is written.
#[tokio::test]
async fn a_confirmed_write_is_refused_while_the_page_repository_readers_are_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1, 2]);
    collaborators(&forge, "acme/vault", &[1]);
    forge.collaborators_down.store(true, Ordering::SeqCst);
    eng.invalidate_github_access(Invalidate::All);
    let mut p = cite("kb", ROTA);
    // Nobody is asked a question whose yes would be refused.
    let err = eng
        .reflection_disclosure_question(&p, &owner())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains(REPO_UNKNOWN), "{err}");
    // A yes that arrives anyway is refused.
    p.disclosed = Some("a basis from before".to_string());
    let err = write(&eng, &p).await.unwrap_err();
    assert!(err.contains(REPO_UNKNOWN), "{err}");
    assert!(!tmp.path().join("kb/reflections/overview.md").exists());
}

/// The backup pushes bytes, so it decides from them: a page file edited on
/// disk with a too wide source before the index read it is not pushed.
#[tokio::test]
async fn a_page_edited_on_disk_before_the_index_reads_it_is_not_pushed() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _, kb) = setup_with_page_repo(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1, 2]);
    collaborators(&forge, "acme/vault", &[1]);
    write(
        &eng,
        &WriteReflectionParams {
            domain: "kb".into(),
            path: "a.md".into(),
            markdown: Some("# A\n\n## One\n\nText.\n".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    std::fs::write(
        tmp.path().join("kb/reflections/forged.md"),
        "---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: vault\n      engram: team/rota\n      checksum: abc\n---\n\n# Forged\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n",
    )
    .unwrap();
    eng.mark_reflection_dirty(
        "kb",
        crystalline_service::reflection_backup::BackupCause::Settled,
    );
    eng.reflection_backup_tick(Instant::now() + Duration::from_secs(120))
        .await;
    let head = kb.commit_paths(&kb.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(head.contains(&"reflections/a.md".to_string()), "{head:?}");
    assert!(!head.iter().any(|p| p.contains("forged")), "{head:?}");
    assert_eq!(
        eng.reflection_backup_held_back("kb"),
        vec!["forged.md".to_string()]
    );
}

/// A person the page's repository lists with a role this instance does not
/// read as read access (a custom role) still counts as a reader of the page:
/// only the source side drops them.
#[tokio::test]
async fn every_listed_person_of_the_page_repository_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    forge.collaborators.lock().unwrap().insert(
        "acme/kb".to_string(),
        vec![
            json!({"id": 1, "login": "gh-1", "role_name": "read"}),
            json!({"id": 2, "login": "gh-2", "role_name": "auditor"}),
        ],
    );
    collaborators(&forge, "acme/vault", &[1]);
    let err = write(&eng, &cite("kb", ROTA)).await.unwrap_err();
    assert!(err.contains("fewer people may read"), "{err}");
}

/// Only the page side's listing unknown (the App left the page's
/// repository): refused.
#[tokio::test]
async fn an_unknown_page_repository_listing_alone_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1]);
    collaborators(&forge, "acme/vault", &[1, 2]);
    write(&eng, &cite("kb", ROTA)).await.unwrap();
    forge.uninstall("acme/kb");
    eng.invalidate_github_access(Invalidate::All);
    let err = write(&eng, &cite("kb", ROTA)).await.unwrap_err();
    assert!(err.contains("fewer people may read"), "{err}");
}

/// Two spellings of one repository are one repository: no listing needed.
#[tokio::test]
async fn two_spellings_of_one_repository_read_alike() {
    let tmp = tempfile::tempdir().unwrap();
    let mono = repo(&["a/", "b/"]);
    let (forge, url) = spawn_forge(
        &[("acme/kb", mono.clone()), ("Acme/KB", mono)],
        &[("acme/kb", 1), ("Acme/KB", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let eng = Arc::new(super::github_app::engine(tmp.path(), &url, &app_env()).await);
    for (spelled, d) in [("acme/kb", "a"), ("Acme/KB", "b")] {
        eng.origin_add(
            spelled,
            Some(d),
            Some(d),
            None,
            Some(tmp.path().join(d).to_str().unwrap()),
            Some(crystalline_service::params::Readers::Shared),
        )
        .await
        .unwrap();
    }
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth).with_governor(eng.knowledge_governor()),
    ));
    forge.collaborators_down.store(true, Ordering::SeqCst);
    eng.invalidate_github_access(Invalidate::All);
    write(&eng, &cite("a", "crystalline://b/team/rota"))
        .await
        .unwrap();
    write(&eng, &cite("b", "crystalline://a/team/rota"))
        .await
        .unwrap();
}

/// An unattended run reads only what every reader of the domain it works on
/// may read, the page repository's readers on GitHub included.
#[tokio::test]
async fn an_unattended_run_does_not_read_a_narrower_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _) = setup(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1, 2]);
    collaborators(&forge, "acme/vault", &[1]);
    let limit = eng
        .read_limit_for(&Scope::Unrestricted, true, None)
        .await
        .expect("unattended");
    assert!(limit.hidden.contains("vault"), "{:?}", limit.hidden);
    assert!(!limit.hidden.contains("kb"), "{:?}", limit.hidden);
    // Once the source's repository has every reader of the page's, it reads.
    collaborators(&forge, "acme/vault", &[1, 2]);
    eng.invalidate_github_access(Invalidate::All);
    let limit = eng
        .read_limit_for(&Scope::Unrestricted, true, None)
        .await
        .expect("unattended");
    assert!(limit.hidden.is_empty(), "{:?}", limit.hidden);
}

/// A file only a held page links stays off the branch, also when that page
/// is known only from its bytes on disk.
#[tokio::test]
async fn a_file_only_a_page_on_disk_links_is_held_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _, kb) = setup_with_page_repo(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", &[1, 2]);
    collaborators(&forge, "acme/vault", &[1]);
    write(
        &eng,
        &WriteReflectionParams {
            domain: "kb".into(),
            path: "a.md".into(),
            markdown: Some("# A\n\n## One\n\nText.\n".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let assets = tmp.path().join("kb/reflections/assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("rota.png"), b"\x89PNG secret").unwrap();
    std::fs::write(
        tmp.path().join("kb/reflections/forged.md"),
        "---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: vault\n      engram: team/rota\n      checksum: abc\n---\n\n# Forged\n\n## Team\n<!-- r:aaaa1111 -->\n\n![rota](assets/rota.png)\n",
    )
    .unwrap();
    eng.mark_reflection_dirty(
        "kb",
        crystalline_service::reflection_backup::BackupCause::Settled,
    );
    eng.reflection_backup_tick(Instant::now() + Duration::from_secs(120))
        .await;
    let head = kb.commit_paths(&kb.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(head.contains(&"reflections/a.md".to_string()), "{head:?}");
    assert!(!head.iter().any(|p| p.contains("rota.png")), "{head:?}");
}

/// `kb` with one page that goes out (`a.md`), then `files` dropped into its
/// `reflections/` folder by hand and one backup tick: the paths on the
/// branch head and what the backup says it held back.
async fn backed_up_with(files: &[(&str, &[u8])]) -> (Vec<String>, Vec<String>) {
    backed_up_as(files, &[1]).await
}

/// [`backed_up_with`] with a second collaborator on kb's repository, who may
/// not read vault's: a page citing vault is too wide.
async fn backed_up_with_wide(files: &[(&str, &[u8])]) -> (Vec<String>, Vec<String>) {
    backed_up_as(files, &[1, 2]).await
}

async fn backed_up_as(files: &[(&str, &[u8])], kb_ids: &[u64]) -> (Vec<String>, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _, kb) = setup_with_page_repo(tmp.path(), &app_env()).await;
    collaborators(&forge, "acme/kb", kb_ids);
    collaborators(&forge, "acme/vault", &[1]);
    write(
        &eng,
        &WriteReflectionParams {
            domain: "kb".into(),
            path: "a.md".into(),
            markdown: Some("# A\n\n## One\n\nText.\n".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for (rel, bytes) in files {
        let abs = tmp.path().join("kb/reflections").join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(abs, bytes).unwrap();
    }
    eng.mark_reflection_dirty(
        "kb",
        crystalline_service::reflection_backup::BackupCause::Settled,
    );
    eng.reflection_backup_tick(Instant::now() + Duration::from_secs(120))
        .await;
    let head = kb.commit_paths(&kb.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(head.contains(&"reflections/a.md".to_string()), "{head:?}");
    // A page's baseline covered by its records goes out with it.
    assert!(
        head.contains(&"reflections/.reflected/a.md".to_string()),
        "{head:?}"
    );
    (head, eng.reflection_backup_held_back("kb"))
}

/// A page whose records cannot be read may hide any source: held.
#[tokio::test]
async fn a_page_whose_records_cannot_be_read_is_held() {
    let pages: [&[u8]; 3] = [
        b"---\nreflection:\n  aaaa1111:\n    sources: 5\n---\n\n# Bad\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n",
        b"---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: vault\n      engram: team/rota\n      checksum: abc\n\n# Bad\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n",
        b"---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: vault\n      engram: team/rota\n      checksum: abc\n---\n\n# Bad \xff\xfe\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n",
    ];
    for page in pages {
        let (head, held) = backed_up_with(&[("bad.md", page)]).await;
        assert!(!head.iter().any(|p| p.contains("bad.md")), "{head:?}");
        assert_eq!(held, vec!["bad.md".to_string()]);
    }
}

/// A page without frontmatter has no records to read and goes out.
#[tokio::test]
async fn a_page_without_frontmatter_goes_out() {
    let (head, held) = backed_up_with(&[("plain.md", b"# Plain\n\n## One\n\nText.\n")]).await;
    assert!(
        head.contains(&"reflections/plain.md".to_string()),
        "{head:?}"
    );
    assert!(held.is_empty(), "{held:?}");
}

/// A held page's baseline is held with it.
#[tokio::test]
async fn a_baseline_stays_held_with_its_page() {
    let page: &[u8] = b"---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: vault\n      engram: team/rota\n      checksum: abc\n---\n\n# Forged\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n";
    let baseline: &[u8] = b"## Team\n<!-- r:aaaa1111 -->\n\nFive people from the vault.\n";
    // vault's repository has fewer readers than kb's here.
    let (head, held) =
        backed_up_with_wide(&[("forged.md", page), (".reflected/forged.md", baseline)]).await;
    assert!(!head.iter().any(|p| p.contains("forged.md")), "{head:?}");
    assert_eq!(held, vec!["forged.md".to_string()]);
}

/// A curator removed a section together with its narrow source; the
/// baseline still keeps that section's text. The page goes out, its
/// baseline does not.
#[tokio::test]
async fn a_baseline_keeping_a_removed_section_is_held_while_the_page_goes_out() {
    let page: &[u8] = b"---\nreflection:\n  bbbb2222:\n    sources:\n    - engram: team/rota\n      checksum: abc\n---\n\n# B\n\n## Kept\n<!-- r:bbbb2222 -->\n\nOurs.\n";
    let baseline: &[u8] = b"## Kept\n<!-- r:bbbb2222 -->\n\nOurs.\n\n## Removed\n<!-- r:cccc3333 -->\n\nFive people from the vault.\n";
    let (head, held) = backed_up_with(&[("b.md", page), (".reflected/b.md", baseline)]).await;
    assert!(head.contains(&"reflections/b.md".to_string()), "{head:?}");
    assert!(
        !head.contains(&"reflections/.reflected/b.md".to_string()),
        "{head:?}"
    );
    assert!(held.is_empty(), "{held:?}");
}
