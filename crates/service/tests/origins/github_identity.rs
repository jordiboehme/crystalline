//! A connected domain records GitHub's numeric ids of its repository and of
//! the repository's owner, and every lookup of the GitHub governor compares
//! them with what GitHub answers (Ruling R33). GitHub keeps a repository's id
//! through a transfer and redirects the old name, so without the ids a
//! transferred repository would open its domain to the new owner's people,
//! after a restart or with no webhook at all. A transfer (another owner id)
//! or a reused name (another repository id) closes the domain on every
//! lookup until an admin reconnects it; a rename within the owner (the same
//! ids) stays open and the origin follows it.
//!
//! The fixture is the webhook suite's: the JSON API is served, but these
//! tests send no webhook at all.

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::GlobalConfig;
use crystalline_index::TursoStore;
use crystalline_remote::state::{OriginState, RepositoryIds};
use crystalline_service::engine::Invalidate;
use crystalline_service::{DomainAccess, Engine, EnvOverlay};
use tokio::sync::Mutex;

use super::github_app::{connect, owner_id};
use super::github_webhook::{Delivery, Fx, SECRET, ask, deliver, origin_repo, setup};

/// The ids recorded for `domain`.
fn recorded(fx: &Fx, domain: &str) -> Option<RepositoryIds> {
    OriginState::load(&fx.tmp.path().join("origins").join(domain))
        .unwrap()
        .unwrap()
        .repository_ids
}

/// The engine started again on the same configuration file and state, the
/// way a restarted daemon is: nothing the old one learned in memory.
async fn restart(fx: &Fx) -> Arc<Engine> {
    restart_engine(fx, false).await
}

/// [`restart`], read-only when `read_only`.
pub(crate) async fn restart_engine(fx: &Fx, read_only: bool) -> Arc<Engine> {
    let path = fx.tmp.path().join("config.yaml");
    let cfg: GlobalConfig = crystalline_core::config::load_yaml(&path).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let overlay = EnvOverlay::from_vars(
        fx.env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let tmp = fx.tmp.path();
    let eng = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(path))
            .with_origins_dir(tmp.join("origins"))
            .with_state_dir(tmp.to_path_buf())
            .with_token_store_dir(tmp.join("tokens"))
            .with_env_overlay(overlay)
            .with_read_only(read_only),
    );
    eng.github_app().unwrap();
    eng.set_domain_access(Arc::new(
        DomainAccess::new(fx.auth.clone()).with_governor(eng.knowledge_governor()),
    ));
    eng
}

pub(crate) const MOVED: &str =
    "the repository moved to newco/kb on GitHub; disconnect the domain and connect it again";

pub(crate) fn closed_reason(eng: &Engine, domain: &str) -> Option<String> {
    eng.github_access_closed()
        .into_iter()
        .find(|c| c.domain == domain)
        .map(|c| c.reason)
}

/// Waits until `check` holds, for things done in the background.
pub(crate) async fn eventually(check: impl Fn() -> bool) -> bool {
    for _ in 0..100 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    check()
}

#[tokio::test]
async fn a_connect_records_the_repository_and_owner_ids() {
    let fx = setup(Some(SECRET)).await;
    assert_eq!(
        recorded(&fx, "kb"),
        Some(RepositoryIds {
            repository: fx.forge.id_of("acme/kb").unwrap(),
            owner: owner_id("acme"),
        })
    );
}

#[tokio::test]
async fn a_transferred_repository_is_closed_after_a_restart_without_a_webhook() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.forge.transfer("acme/kb", "newco/kb");
    let eng = restart(&fx).await;
    assert!(eng.github_people("kb").await.is_none());
    assert_eq!(closed_reason(&eng, "kb").as_deref(), Some(MOVED));
    assert_eq!(origin_repo(&eng, "kb"), "acme/kb");
    // And again on the next start.
    let eng = restart(&fx).await;
    assert!(eng.github_people("kb").await.is_none());
    assert_eq!(closed_reason(&eng, "kb").as_deref(), Some(MOVED));
}

#[tokio::test]
async fn a_reused_name_is_closed() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.forge.reuse("acme/kb");
    fx.eng
        .invalidate_github_access(Invalidate::Repo("acme/kb".to_string()));
    assert!(fx.eng.github_people("kb").await.is_none());
    assert_eq!(
        closed_reason(&fx.eng, "kb").as_deref(),
        Some(
            "the name acme/kb now belongs to another repository on GitHub; disconnect the domain and connect it again"
        )
    );
}

#[tokio::test]
async fn a_rename_within_the_owner_stays_open_and_repoints() {
    let fx = setup(Some(SECRET)).await;
    fx.forge.rename("acme/kb", "acme/handbook");
    let eng = restart(&fx).await;
    let people = eng.github_people("kb").await;
    assert!(people.is_some_and(|p| !p.is_empty()));
    assert!(closed_reason(&eng, "kb").is_none());
    assert!(eventually(|| origin_repo(&eng, "kb") == "acme/handbook").await);
    let saved = std::fs::read_to_string(fx.tmp.path().join("config.yaml")).unwrap();
    assert!(saved.contains("acme/handbook"), "{saved}");
}

#[tokio::test]
async fn a_domain_connected_before_records_its_ids_on_its_first_lookup() {
    let fx = setup(Some(SECRET)).await;
    // As a state from before the ids were recorded.
    let dir = fx.tmp.path().join("origins").join("kb");
    let mut state = OriginState::load(&dir).unwrap().unwrap();
    state.repository_ids = None;
    state.save(&dir).unwrap();
    let eng = restart(&fx).await;
    assert!(eng.github_people("kb").await.is_some());
    let want = RepositoryIds {
        repository: fx.forge.id_of("acme/kb").unwrap(),
        owner: owner_id("acme"),
    };
    assert!(eventually(|| recorded(&fx, "kb") == Some(want)).await);
    // From then on a transfer closes it.
    fx.forge.transfer("acme/kb", "newco/kb");
    let eng = restart(&fx).await;
    assert!(eng.github_people("kb").await.is_none());
    assert_eq!(closed_reason(&eng, "kb").as_deref(), Some(MOVED));
}

#[tokio::test]
async fn reconnecting_records_the_new_ids_and_opens_the_domain() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.forge.transfer("acme/kb", "newco/kb");
    fx.eng
        .invalidate_github_access(Invalidate::Repo("acme/kb".to_string()));
    assert!(fx.eng.github_people("kb").await.is_none());
    assert_eq!(closed_reason(&fx.eng, "kb").as_deref(), Some(MOVED));

    // An admin disconnects the domain and connects it where it is now.
    fx.eng.domain_remove("kb").await.unwrap();
    connect(&fx.eng, "newco/kb", "kb", &fx.tmp.path().join("kb-again")).await;
    assert_eq!(
        recorded(&fx, "kb"),
        Some(RepositoryIds {
            repository: fx.forge.id_of("newco/kb").unwrap(),
            owner: owner_id("newco"),
        })
    );
    assert!(fx.eng.github_people("kb").await.is_some());
    assert!(closed_reason(&fx.eng, "kb").is_none());
}

/// A transfer webhook for a domain with no ids recorded yet records the
/// repository's id and the old owner's, so the close outlives a restart.
#[tokio::test]
async fn a_transfer_webhook_records_the_old_owner_so_a_restart_stays_closed() {
    let fx = setup(Some(SECRET)).await;
    let dir = fx.tmp.path().join("origins").join("kb");
    let mut state = OriginState::load(&dir).unwrap().unwrap();
    state.repository_ids = None;
    state.save(&dir).unwrap();
    let id = fx.forge.id_of("acme/kb").unwrap();
    let payload = serde_json::json!({
        "action": "transferred",
        "changes": {"owner": {"from": {"organization": {"login": "acme", "id": owner_id("acme")}}}},
        "repository": {"id": id, "full_name": "newco/kb", "name": "kb", "owner": {"login": "newco"}},
        "installation": {"id": 1},
    });
    fx.forge.transfer("acme/kb", "newco/kb");
    let resp = deliver(&fx, Delivery::signed("repository", "d-transfer", &payload)).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        recorded(&fx, "kb"),
        Some(RepositoryIds {
            repository: id,
            owner: owner_id("acme"),
        })
    );
    let eng = restart(&fx).await;
    assert!(eng.github_people("kb").await.is_none());
    assert_eq!(closed_reason(&eng, "kb").as_deref(), Some(MOVED));
}
