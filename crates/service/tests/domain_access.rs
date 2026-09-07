//! The engine's half of the visibility substrate: an engine with no resolver
//! installed filters nothing, one with a resolver answers per caller, and the
//! resolver is installed exactly once.
//!
//! The policy itself is pinned by the unit tests in `crate::scope`; what this
//! file pins is the wiring the read verbs (Task 9) and the REST surface
//! (Task 10) stand on.

use std::sync::Arc;

use crystalline_core::config::GlobalConfig;
use crystalline_index::TursoStore;
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use crystalline_service::{DomainAccess, Engine, Scope};
use tokio::sync::Mutex;

async fn engine() -> Arc<Engine> {
    let store = TursoStore::open_in_memory().await.unwrap();
    Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        GlobalConfig::default(),
        None,
        None,
    ))
}

/// An accounts store holding a private `lab` owned by `owner`, with `mem`
/// invited and `out` a stranger.
async fn auth(dir: &std::path::Path, file: &str) -> Arc<AuthStore> {
    let auth = Arc::new(AuthStore::open(&dir.join(file)).await.unwrap());
    for name in ["owner", "mem", "out"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("lab", true, "owner")
        .await
        .unwrap();
    auth.upsert_domain_member("lab", "mem", MemberLevel::Viewer, "owner")
        .await
        .unwrap();
    auth
}

fn user(account: &str) -> Scope {
    Scope::User {
        account: account.into(),
        admin: false,
    }
}

#[tokio::test]
async fn an_engine_with_no_resolver_filters_nothing() {
    let engine = engine().await;
    // The embedded stdio stack, a one-shot CLI command and every test engine:
    // no accounts database was ever opened, so there is nothing to filter by
    // and nothing to fail on.
    for scope in [Scope::Unrestricted, Scope::Anonymous, user("out")] {
        assert!(
            engine.hidden_domains(&scope).await.unwrap().is_none(),
            "{scope:?} sees everything on an engine with no resolver"
        );
    }
}

#[tokio::test]
async fn an_installed_resolver_hides_a_private_domain_from_a_stranger() {
    let dir = tempfile::tempdir().unwrap();
    let auth = auth(dir.path(), "web-auth.db").await;
    let engine = engine().await;
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));

    assert!(
        engine
            .hidden_domains(&Scope::Unrestricted)
            .await
            .unwrap()
            .is_none(),
        "the machine owner is still unfiltered"
    );
    let hidden = engine.hidden_domains(&user("out")).await.unwrap().unwrap();
    assert!(hidden.contains("lab"), "a stranger does not see it");
    assert!(
        engine
            .hidden_domains(&user("mem"))
            .await
            .unwrap()
            .unwrap()
            .is_empty(),
        "an invited member sees it"
    );
    assert!(
        engine
            .hidden_domains(&user("owner"))
            .await
            .unwrap()
            .unwrap()
            .is_empty(),
        "and so does its owner"
    );
    assert!(
        engine
            .hidden_domains(&Scope::Anonymous)
            .await
            .unwrap()
            .unwrap()
            .contains("lab"),
        "nobody in particular sees nothing private"
    );
}

#[tokio::test]
async fn the_resolver_is_installed_once_and_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let first = auth(dir.path(), "first.db").await;
    let second = Arc::new(
        AuthStore::open(&dir.path().join("second.db"))
            .await
            .unwrap(),
    );
    let engine = engine().await;
    engine.set_domain_access(Arc::new(DomainAccess::new(first)));
    engine.set_domain_access(Arc::new(DomainAccess::new(second)));
    assert!(
        engine
            .hidden_domains(&user("out"))
            .await
            .unwrap()
            .unwrap()
            .contains("lab"),
        "a second install is ignored, so nothing swaps the authority mid-flight"
    );
}
