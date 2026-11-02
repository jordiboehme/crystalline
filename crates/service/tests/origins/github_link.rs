//! The person link on an instance with a GitHub App: an account links its
//! GitHub account through an App user token, `personal` shares go out on
//! that token (refreshed first when it is about to expire, once per account
//! however many shares ask), a refresh GitHub refuses keeps the link and
//! refuses the share, and an unlink revokes the token at GitHub.
//!
//! The forge, its log and the repository routes are the ones
//! `origins::github_app` serves; this module adds the user-token routes it
//! learned for these tests (`GET /user`, the refresh exchange and the
//! revoke).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use chrono::Utc;
use crystalline_core::config::{GitHubConfig, GlobalConfig, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_remote::UserTokenGrant;
use crystalline_service::engine::{EngineError, GithubUnlink, ShareActor};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EnvOverlay};
use serde_json::Value;
use tokio::sync::Mutex;

use super::github_app::{
    Forge, INSTANCE_TOKEN, PEM, bearer, connect, engram, repo_with_manifest, spawn_forge,
};

const CLIENT_ID: &str = "Iv23.test";
const CLIENT_SECRET: &str = "app-client-secret";
const ADA_ID: u64 = 1001;
const ADA_LOGIN: &str = "ada-gh";

/// The environment of an instance with an App that has a client secret,
/// plus the instance token, so a test can tell which credential a request
/// carried.
fn app_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN),
        ("CRYSTALLINE_GITHUB_APP_ID", "7"),
        ("CRYSTALLINE_GITHUB_APP_CLIENT_ID", CLIENT_ID),
        ("CRYSTALLINE_GITHUB_APP_CLIENT_SECRET", CLIENT_SECRET),
        ("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", PEM),
    ]
}

/// An engine pointed at the forge in `share_identity` mode, with the
/// accounts `ada` (with an email address) and `bob` in its accounts store.
async fn engine(tmp: &Path, api_url: &str, share_identity: &str) -> Engine {
    engine_with(
        tmp,
        api_url,
        share_identity,
        &app_env(),
        Some("ada@example.com"),
    )
    .await
}

async fn engine_with(
    tmp: &Path,
    api_url: &str,
    share_identity: &str,
    env: &[(&str, &str)],
    email: Option<&str>,
) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(api_url.to_string()),
            share_identity: Some(share_identity.to_string()),
            ..GitHubConfig::default()
        }),
        service: Some(ServiceConfig {
            public_url: Some("https://kb.example.com".into()),
            ..Default::default()
        }),
        ..GlobalConfig::default()
    };
    let overlay = EnvOverlay::from_vars(
        env.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(tmp.join("config.yaml")),
    )
    .with_origins_dir(tmp.join("origins"))
    .with_state_dir(tmp.to_path_buf())
    .with_token_store_dir(tmp.join("tokens"))
    .with_env_overlay(overlay);
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    auth.add_user("ada", "Ada Lovelace", email, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.add_user("bob", "Bob", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(DomainAccess::new(auth)));
    eng
}

/// The forge with the repositories `acme/kb` and `acme/kb2`, both covered by
/// installation 1, and `ada`'s user token `access` known to it.
async fn forge_with_ada(access: &str) -> (Arc<Forge>, String) {
    let (forge, url) = spawn_forge(
        &[
            ("acme/kb", repo_with_manifest("")),
            ("acme/kb2", repo_with_manifest("")),
        ],
        &[("acme/kb", 1), ("acme/kb2", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    forge
        .users
        .lock()
        .unwrap()
        .insert(access.to_string(), (ADA_ID, ADA_LOGIN.to_string()));
    (forge, url)
}

/// Connects `repo` as domain `name` and leaves one new engram to share.
async fn connect_with_change(eng: &Engine, tmp: &Path, repo: &str, name: &str) {
    let root = tmp.join(name);
    connect(eng, repo, name, &root).await;
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/new.md"),
        engram("New", "new", "brand new content"),
    )
    .unwrap();
}

fn grant(
    access: &str,
    refresh: Option<&str>,
    expires_in: Option<chrono::Duration>,
) -> UserTokenGrant {
    UserTokenGrant {
        access_token: access.to_string(),
        refresh_token: refresh.map(str::to_string),
        expires_at: expires_in.map(|d| Utc::now() + d),
        refresh_expires_at: refresh.map(|_| Utc::now() + chrono::Duration::days(180)),
    }
}

async fn share_as_ada(eng: &Engine, domain: &str) -> Result<Value, EngineError> {
    eng.origin_share(
        domain,
        None,
        None,
        None,
        None,
        ShareActor::Account("ada".to_string()),
    )
    .await
}

/// The `Authorization` of every write the forge saw on `repo`.
fn write_auths(forge: &Forge, repo: &str) -> Vec<Option<String>> {
    forge
        .hits()
        .into_iter()
        .filter(|h| h.is_repo_call() && h.repo().as_deref() == Some(repo) && h.method != "GET")
        .map(|h| h.auth)
        .collect()
}

/// Makes every refresh wait at the forge until the returned semaphore gets
/// permits.
fn hold_refreshes(forge: &Forge) -> Arc<tokio::sync::Semaphore> {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *forge.refresh_gate.lock().unwrap() = Some(gate.clone());
    gate
}

/// Waits until the forge has seen `n` refresh requests.
async fn wait_for_refreshes(forge: &Forge, n: usize) {
    for _ in 0..1000 {
        if refreshes(forge).len() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the forge never saw {n} refreshes");
}

fn refreshes(forge: &Forge) -> Vec<String> {
    forge.refreshes.lock().unwrap().clone()
}

/// The refusal a `personal` share without an App user token gets: it names
/// the App's way to link, never the OAuth connect.
fn is_personal_token_missing(e: &EngineError) -> bool {
    let text = e.to_string();
    text.starts_with("This instance shares with personal GitHub identities through its GitHub App")
        && text.contains("crystalline connect github --personal --as <account>")
}

#[tokio::test]
async fn a_personal_share_with_the_app_refreshes_an_expiring_token_first() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_old").await;
    forge.refreshable.lock().unwrap().insert(
        "ghr_old".to_string(),
        (
            "ghu_new".to_string(),
            "ghr_new".to_string(),
            (ADA_ID, ADA_LOGIN.to_string()),
        ),
    );
    let eng = engine(tmp.path(), &url, "personal").await;
    let view = eng
        .link_github(
            "ada",
            grant(
                "ghu_old",
                Some("ghr_old"),
                Some(chrono::Duration::minutes(2)),
            ),
        )
        .await
        .unwrap();
    assert!(view.linked);
    assert_eq!(view.login.as_deref(), Some(ADA_LOGIN));
    assert_eq!(view.github_id, Some(ADA_ID));
    assert!(view.token_live);
    let json = serde_json::to_string(&view).unwrap();
    assert!(
        !json.contains("ghu_") && !json.contains("ghr_"),
        "the view carries no token: {json}"
    );
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    let result = share_as_ada(&eng, "kb").await.unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(
        refreshes(&forge),
        vec!["ghr_old".to_string()],
        "one refresh"
    );
    let auths = write_auths(&forge, "acme/kb");
    assert!(!auths.is_empty());
    assert!(
        auths.iter().all(|a| *a == bearer("ghu_new")),
        "every write carried the refreshed token: {auths:?}"
    );
}

#[tokio::test]
async fn a_revoked_refresh_keeps_the_link_and_refuses_the_share() {
    let tmp = tempfile::tempdir().unwrap();
    // The forge knows no refresh token: the exchange answers
    // `bad_refresh_token`, the way GitHub answers a revoked authorization.
    let (forge, url) = forge_with_ada("ghu_old").await;
    let eng = engine(tmp.path(), &url, "personal").await;
    eng.link_github(
        "ada",
        grant(
            "ghu_old",
            Some("ghr_old"),
            Some(chrono::Duration::minutes(2)),
        ),
    )
    .await
    .unwrap();
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    let err = share_as_ada(&eng, "kb").await.unwrap_err();
    assert!(is_personal_token_missing(&err), "{err}");
    assert_eq!(refreshes(&forge).len(), 1);
    assert!(
        write_auths(&forge, "acme/kb").is_empty(),
        "nothing was written"
    );

    let status = eng.github_link_status("ada").await.unwrap();
    assert!(status.linked, "the link stays");
    assert_eq!(status.github_id, Some(ADA_ID));
    assert!(!status.token_live, "the token is marked gone");

    // The token was deleted: the next share is refused without asking
    // GitHub again.
    let err = share_as_ada(&eng, "kb").await.unwrap_err();
    assert!(is_personal_token_missing(&err), "{err}");
    assert_eq!(refreshes(&forge).len(), 1, "no second refresh");
}

#[tokio::test]
async fn a_ghes_token_without_expiry_is_used_as_is() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_forever").await;
    let eng = engine(tmp.path(), &url, "personal").await;
    eng.link_github("ada", grant("ghu_forever", None, None))
        .await
        .unwrap();
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    let result = share_as_ada(&eng, "kb").await.unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert!(refreshes(&forge).is_empty(), "never refreshed");
    let auths = write_auths(&forge, "acme/kb");
    assert!(!auths.is_empty());
    assert!(
        auths.iter().all(|a| *a == bearer("ghu_forever")),
        "{auths:?}"
    );
}

#[tokio::test]
async fn two_concurrent_shares_refresh_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_old").await;
    // Spent on use: a second refresh with the same token would be answered
    // `bad_refresh_token` and refuse that share.
    forge.refreshable.lock().unwrap().insert(
        "ghr_old".to_string(),
        (
            "ghu_new".to_string(),
            "ghr_new".to_string(),
            (ADA_ID, ADA_LOGIN.to_string()),
        ),
    );
    let eng = engine(tmp.path(), &url, "personal").await;
    eng.link_github(
        "ada",
        grant(
            "ghu_old",
            Some("ghr_old"),
            Some(chrono::Duration::minutes(2)),
        ),
    )
    .await
    .unwrap();
    // Two domains: shares of one domain wait for each other on its lock.
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;
    connect_with_change(&eng, tmp.path(), "acme/kb2", "kb2").await;

    // The first refresh is held open at the forge while the second share
    // runs, so the two overlap for certain: without the per-account lock
    // the second share presents the spent refresh token too and is refused.
    let gate = hold_refreshes(&forge);
    let release = async {
        wait_for_refreshes(&forge, 1).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        gate.add_permits(16);
    };
    let (one, two, ()) = tokio::join!(share_as_ada(&eng, "kb"), share_as_ada(&eng, "kb2"), release);
    assert_eq!(one.unwrap()["outcome"], "proposed");
    assert_eq!(two.unwrap()["outcome"], "proposed");
    assert_eq!(
        refreshes(&forge),
        vec!["ghr_old".to_string()],
        "one refresh"
    );
    for repo in ["acme/kb", "acme/kb2"] {
        let auths = write_auths(&forge, repo);
        assert!(!auths.is_empty());
        assert!(auths.iter().all(|a| *a == bearer("ghu_new")), "{auths:?}");
    }
}

#[tokio::test]
async fn a_share_cancelled_during_the_refresh_still_saves_the_new_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_old").await;
    forge.refreshable.lock().unwrap().insert(
        "ghr_old".to_string(),
        (
            "ghu_new".to_string(),
            "ghr_new".to_string(),
            (ADA_ID, ADA_LOGIN.to_string()),
        ),
    );
    let eng = engine(tmp.path(), &url, "personal").await;
    eng.link_github(
        "ada",
        grant(
            "ghu_old",
            Some("ghr_old"),
            Some(chrono::Duration::minutes(2)),
        ),
    )
    .await
    .unwrap();
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    // GitHub has rotated the pair and not answered yet when the share is
    // dropped, the way a handler is dropped when its client goes away.
    let gate = hold_refreshes(&forge);
    {
        let share = share_as_ada(&eng, "kb");
        tokio::pin!(share);
        tokio::select! {
            result = &mut share => panic!("the share finished before the refresh: {result:?}"),
            () = wait_for_refreshes(&forge, 1) => {}
        }
    }
    gate.add_permits(16);

    let result = share_as_ada(&eng, "kb").await.unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    assert_eq!(
        refreshes(&forge),
        vec!["ghr_old".to_string()],
        "the renewed pair was saved, so no second refresh"
    );
    let auths = write_auths(&forge, "acme/kb");
    assert!(!auths.is_empty());
    assert!(auths.iter().all(|a| *a == bearer("ghu_new")), "{auths:?}");
}

#[tokio::test]
async fn unlink_revokes_at_github_and_deletes_the_row() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine(tmp.path(), &url, "personal").await;
    eng.link_github("ada", grant("ghu_live", None, None))
        .await
        .unwrap();
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    assert_eq!(
        eng.unlink_github("ada").await.unwrap(),
        GithubUnlink::Unlinked
    );
    assert_eq!(
        eng.unlink_github("ada").await.unwrap(),
        GithubUnlink::NotLinked,
        "a second unlink finds nothing"
    );

    let revokes = forge.revokes.lock().unwrap().clone();
    assert_eq!(revokes.len(), 1, "{revokes:?}");
    let basic = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("{CLIENT_ID}:{CLIENT_SECRET}"),
    );
    assert_eq!(revokes[0].0, Some(format!("Basic {basic}")));
    assert_eq!(revokes[0].1, "ghu_live");
    let revoke_path = forge
        .hits()
        .into_iter()
        .find(|h| h.method == "DELETE" && h.path.starts_with("/applications/"))
        .unwrap()
        .path;
    assert_eq!(revoke_path, format!("/applications/{CLIENT_ID}/token"));

    let status = eng.github_link_status("ada").await.unwrap();
    assert!(!status.linked, "the row is gone");
    assert_eq!(status.github_id, None);
    let err = share_as_ada(&eng, "kb").await.unwrap_err();
    assert!(is_personal_token_missing(&err), "the token is gone: {err}");
}

#[tokio::test]
async fn a_failed_revoke_still_deletes_locally() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine(tmp.path(), &url, "personal").await;
    eng.link_github("ada", grant("ghu_live", None, None))
        .await
        .unwrap();
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;
    forge.revoke_fails.store(true, Ordering::SeqCst);

    let outcome = eng.unlink_github("ada").await.unwrap();
    assert_eq!(outcome, GithubUnlink::NotRevoked);
    assert_eq!(
        outcome.warning(),
        Some(
            "unlinked here; GitHub could not be reached to revoke the token, revoke it in your GitHub settings"
        )
    );

    assert_eq!(
        forge.revokes.lock().unwrap().len(),
        1,
        "the revoke was tried"
    );
    assert!(!eng.github_link_status("ada").await.unwrap().linked);
    let err = share_as_ada(&eng, "kb").await.unwrap_err();
    assert!(is_personal_token_missing(&err), "the token is gone: {err}");
}

#[tokio::test]
async fn a_pasted_personal_token_is_refused_while_an_app_is_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine(tmp.path(), &url, "personal").await;

    let err = eng
        .connect_github_identity_token("ada", "ghp_pasted")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "this instance uses a GitHub App: connect GitHub from your profile"
    );
    let err = eng
        .start_github_identity_device_flow("ada", false)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "this instance uses a GitHub App: connect GitHub from your profile"
    );
    assert!(
        forge.hits().iter().all(|h| h.path != "/user"),
        "GitHub was not asked"
    );
}

#[tokio::test]
async fn a_pasted_personal_token_still_works_without_an_app() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = forge_with_ada("ghp_pasted").await;
    let eng = engine_with(
        tmp.path(),
        &url,
        "personal",
        &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
        None,
    )
    .await;
    let identity = eng
        .connect_github_identity_token("ada", "ghp_pasted")
        .await
        .unwrap();
    assert!(identity.connected);
}

#[tokio::test]
async fn link_github_is_refused_without_an_app() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine_with(
        tmp.path(),
        &url,
        "personal",
        &[("CRYSTALLINE_GITHUB_TOKEN", INSTANCE_TOKEN)],
        None,
    )
    .await;
    let err = eng
        .link_github("ada", grant("ghu_live", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(!eng.github_link_status("ada").await.unwrap().linked);
}

#[tokio::test]
async fn a_github_id_linked_to_another_account_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine(tmp.path(), &url, "personal").await;
    eng.link_github("ada", grant("ghu_live", None, None))
        .await
        .unwrap();
    let err = eng
        .link_github("bob", grant("ghu_live", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(
        err.to_string()
            .contains("already linked to another Crystalline account"),
        "{err}"
    );
    assert!(!eng.github_link_status("bob").await.unwrap().linked);
}

#[tokio::test]
async fn an_instance_share_by_a_linked_account_is_authored_with_the_noreply_address() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine(tmp.path(), &url, "instance").await;
    eng.link_github("ada", grant("ghu_live", None, None))
        .await
        .unwrap();
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    let result = share_as_ada(&eng, "kb").await.unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    let commits = forge.commits();
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert_eq!(commits[0]["author"]["name"], "Ada Lovelace");
    assert_eq!(
        commits[0]["author"]["email"],
        format!("{ADA_ID}+{ADA_LOGIN}@users.noreply.127.0.0.1"),
        "the link's noreply address wins over the account email"
    );
    assert_eq!(commits[0]["committer"]["name"], "crystalline-acme[bot]");
}

#[tokio::test]
async fn an_instance_share_by_a_linked_account_without_email_names_the_person() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine_with(tmp.path(), &url, "instance", &app_env(), None).await;
    eng.link_github("ada", grant("ghu_live", None, None))
        .await
        .unwrap();
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    let result = share_as_ada(&eng, "kb").await.unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    let commits = forge.commits();
    assert_eq!(
        commits[0]["author"]["email"],
        format!("{ADA_ID}+{ADA_LOGIN}@users.noreply.127.0.0.1"),
        "{}",
        commits[0]
    );
}

#[tokio::test]
async fn an_instance_share_by_an_unlinked_account_keeps_the_account_email() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = forge_with_ada("ghu_live").await;
    let eng = engine(tmp.path(), &url, "instance").await;
    connect_with_change(&eng, tmp.path(), "acme/kb", "kb").await;

    let result = share_as_ada(&eng, "kb").await.unwrap();
    assert_eq!(result["outcome"], "proposed", "{result}");
    let commits = forge.commits();
    assert_eq!(commits[0]["author"]["email"], "ada@example.com");
}

#[tokio::test]
async fn a_device_flow_link_lands_as_a_link() {
    use crate::support::{device_flow_start, fake_auth};
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, url) = forge_with_ada("ghu_device").await;
    let auth = fake_auth(
        Ok(device_flow_start()),
        Ok("ghu_device".to_string()),
        Err(crystalline_remote::RemoteError::NotConnected),
    );
    let eng = engine(tmp.path(), &url, "personal")
        .await
        .with_connect_auth(auth.clone());

    let started = eng.start_github_link_device_flow("ada").await.unwrap();
    assert_eq!(started["pending"]["user_code"], "ABCD-1234", "{started}");
    assert_eq!(started["linked"], false);
    auth.run_gate.notify_one();

    let mut linked = None;
    for _ in 0..200 {
        let status = eng.github_link_status("ada").await.unwrap();
        // The row is written a moment before the flow's outcome lands, so
        // a read in between sees the link with the code still pending.
        if status.linked && status.pending.is_none() {
            linked = Some(status);
            break;
        }
        assert_eq!(status.error, None);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let linked = linked.expect("the device flow linked the account");
    assert_eq!(linked.github_id, Some(ADA_ID));
    assert_eq!(linked.login.as_deref(), Some(ADA_LOGIN));
    assert!(linked.pending.is_none());
}
