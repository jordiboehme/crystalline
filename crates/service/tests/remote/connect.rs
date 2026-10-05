//! Adding a source: the browser flow against a live server with a fake
//! browser that follows the loopback redirect, token paste (asked for, or
//! because the server has no browser sign-in), the names a second source
//! gets, disconnecting one source, and a server that cannot be reached at
//! each of those steps.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crystalline_remote::{
    Announcement, Connection, CredentialKind, LocalDomain, OriginIdentity, Revocation, SignInError,
    connect_with_browser, connect_with_browser_within, connect_with_token,
    connect_with_token_within, disconnect, disconnect_within, load_sources, update_sources,
};
use serde_json::json;
use tokio::net::TcpListener;

use crate::fixture::{Options, RemoteServer};

/// A short limit for the servers that never answer.
const LIMIT: Duration = Duration::from_millis(300);

fn fake_browser(
    server: Arc<RemoteServer>,
    account: &'static str,
    decision: &'static str,
) -> impl FnOnce(&str) + Send {
    move |authorize_url: &str| {
        let url = authorize_url.to_string();
        tokio::spawn(async move {
            let location = server.consent_with(account, &url, decision).await;
            let _ = reqwest::get(location).await;
        });
    }
}

/// The paste door of a server that has OAuth: never opened.
fn no_paste(_note: String) -> Option<String> {
    panic!("a server with browser sign-in never asks for a token")
}

/// Build an HTTP client once before a clock starts: the first one in a
/// process loads the platform's certificate roots, which takes a moment of
/// its own and is no network wait.
fn warm_up() {
    let _ = crystalline_remote::http_client();
}

/// A listener that accepts every connection and never answers, the way a
/// server looks behind a VPN that dropped its packets.
async fn blackhole() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            held.push(socket);
        }
    });
    (url, accepted)
}

#[test]
fn the_token_prefix_is_the_account_stores() {
    assert_eq!(
        crystalline_remote::sign_in::MCP_TOKEN_PREFIX,
        crystalline_service::rest::MCP_TOKEN_PREFIX
    );
}

#[tokio::test]
async fn signing_in_through_the_browser_saves_an_oauth_source() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    let connected = connect_with_browser(
        &format!("{}/some/path/", server.origin()),
        Some("acme"),
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap();
    let source = &connected.source;
    assert_eq!(source.url, server.origin(), "the origin, no path, no slash");
    assert_eq!(source.name, "acme");
    assert_eq!(source.kind, CredentialKind::Oauth);
    assert_eq!(source.account, "keeper");
    assert!(
        source
            .token_endpoint
            .as_deref()
            .is_some_and(|e| e.ends_with("/api/v1/oauth/token"))
    );
    assert!(
        source
            .revocation_endpoint
            .as_deref()
            .is_some_and(|e| e.ends_with("/api/v1/oauth/revoke"))
    );
    assert_eq!(
        load_sources(dir.path()).unwrap().sources,
        vec![source.clone()]
    );
    assert!(
        source.host_dir(dir.path()).join("credential.json").exists(),
        "under the test seam the credential is in the file, never the keychain"
    );
    assert!(
        source.host_dir(dir.path()).join("routing.json").exists(),
        "the first cache"
    );
    assert_eq!(
        server.auth.list_oauth_grants("keeper").await.unwrap().len(),
        1
    );
    let names: Vec<&str> = source.mounts.iter().map(|m| m.local.as_str()).collect();
    assert_eq!(
        names,
        vec!["lab", "open", "platform", "team"],
        "every name is kept"
    );
    let connection = Connection::open(source.clone(), dir.path()).unwrap();
    let data = connection
        .ctl_data(json!({ "v": 1, "cmd": "status" }))
        .await
        .unwrap();
    assert_eq!(data["account"], "keeper");
}

#[tokio::test]
async fn a_sign_in_the_person_denies_is_reported_and_saves_nothing() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser(
        &server.origin(),
        None,
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "deny"),
        no_paste,
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, SignInError::Denied(_)), "{failure}");
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
}

/// Ruling F16: a server without browser sign-in is not a dead end. The
/// person is told why and asked for a token, and the source is saved as a
/// pasted one.
#[tokio::test]
async fn a_server_without_oauth_falls_through_to_a_pasted_token() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = server.token_for("keeper").await;
    let dir = tempfile::tempdir().unwrap();
    let told = Arc::new(Mutex::new(Vec::<String>::new()));
    let heard = told.clone();
    let connected = connect_with_browser(
        &server.origin(),
        None,
        dir.path(),
        &[],
        |_url: &str| panic!("no browser is opened for a server without OAuth"),
        move |note: String| {
            heard.lock().unwrap().push(note);
            Some(format!("{token}\n"))
        },
    )
    .await
    .unwrap();
    let told = told.lock().unwrap().clone();
    assert_eq!(told.len(), 1, "asked once: {told:?}");
    assert!(
        told[0].contains(&server.origin()) && told[0].contains("does not offer browser sign-in"),
        "{}",
        told[0]
    );
    assert!(
        told[0].contains("Agent access") && !told[0].contains("--token"),
        "it says where a token comes from and does not send the person back: {}",
        told[0]
    );
    assert_eq!(connected.source.kind, CredentialKind::Token);
    assert_eq!(connected.source.account, "keeper");
    assert_eq!(
        load_sources(dir.path()).unwrap().sources,
        vec![connected.source.clone()]
    );
}

#[tokio::test]
async fn no_token_at_the_prompt_saves_nothing() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    for answer in [None, Some("  \n".to_string())] {
        let failure = connect_with_browser(
            &server.origin(),
            None,
            dir.path(),
            &[],
            |_url: &str| panic!("no browser is opened for a server without OAuth"),
            move |_note: String| answer,
        )
        .await
        .unwrap_err();
        assert!(matches!(failure, SignInError::BadToken(_)), "{failure}");
    }
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
}

#[tokio::test]
async fn a_pasted_token_adds_a_source_named_after_its_host() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = server.token_for("keeper").await;
    let dir = tempfile::tempdir().unwrap();
    let connected = connect_with_token(
        &server.origin(),
        None,
        &format!("  {token}\n"),
        dir.path(),
        &[],
    )
    .await
    .unwrap();
    assert_eq!(connected.source.kind, CredentialKind::Token);
    assert_eq!(connected.source.account, "keeper");
    assert_eq!(
        connected.source.name, "server",
        "a loopback address has no host label"
    );
    assert!(
        connected.announcements.is_empty(),
        "nothing collided: {:?}",
        connected.announcements
    );
}

#[tokio::test]
async fn a_bad_pasted_token_is_refused_and_saves_nothing() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    for bad in ["cmt_nobody_issued_this", "ghp_a_github_token", ""] {
        let failure = connect_with_token(&server.origin(), None, bad, dir.path(), &[])
            .await
            .unwrap_err();
        assert!(
            matches!(failure, SignInError::BadToken(_)),
            "{bad}: {failure}"
        );
    }
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
}

#[tokio::test]
async fn an_address_that_is_not_crystalline_is_refused() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, axum::Router::new()).await;
    });
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_token(&url, None, "cmt_x", dir.path(), &[])
        .await
        .unwrap_err();
    assert_eq!(failure, SignInError::NotCrystalline { url });
}

/// The second server brings names the first already has: they get its
/// suffix, the same team domain is left out, and the first keeps all of its
/// names.
#[tokio::test]
async fn a_second_source_gets_the_suffix_and_the_same_domain_is_taken_from_the_first() {
    let first = RemoteServer::start(Options::TOKENS).await;
    let second = RemoteServer::start(Options::TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    connect_with_token(
        &first.origin(),
        Some("acme"),
        &first.token_for("keeper").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap();
    let beta = connect_with_token(
        &second.origin(),
        Some("beta"),
        &second.token_for("keeper").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap();
    assert!(
        beta.announcements.contains(&Announcement::Renamed {
            source: "beta".into(),
            remote: "open".into(),
            local: "open-beta".into(),
        }),
        "{:?}",
        beta.announcements
    );
    assert!(
        beta.announcements.contains(&Announcement::Skipped {
            source: "beta".into(),
            remote: "platform".into(),
            kept_by: "acme".into(),
        }),
        "{:?}",
        beta.announcements
    );
    let file = load_sources(dir.path()).unwrap();
    assert_eq!(
        file.names(),
        vec!["acme".to_string(), "beta".to_string()],
        "connect order"
    );
    let acme: Vec<&str> = file.sources[0]
        .mounts
        .iter()
        .map(|m| m.local.as_str())
        .collect();
    assert_eq!(
        acme,
        vec!["lab", "open", "platform", "team"],
        "the first keeps its names"
    );
}

#[tokio::test]
async fn the_same_domain_as_a_local_copy_is_announced_as_replacing_it() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let local = [LocalDomain {
        name: "platform".into(),
        aliases: vec![],
        origin: Some(OriginIdentity {
            forge: "github.com".into(),
            repository: "acme/platform".into(),
            path: String::new(),
            branch: "main".into(),
        }),
    }];
    let connected = connect_with_token(
        &server.origin(),
        Some("acme"),
        &server.token_for("keeper").await,
        dir.path(),
        &local,
    )
    .await
    .unwrap();
    assert!(
        connected
            .announcements
            .contains(&Announcement::ReplacesLocal {
                source: "acme".into(),
                local: "platform".into(),
            }),
        "{:?}",
        connected.announcements
    );
}

#[tokio::test]
async fn a_taken_source_name_is_refused_and_reconnecting_keeps_the_name() {
    let first = RemoteServer::start(Options::TOKENS).await;
    let second = RemoteServer::start(Options::TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    connect_with_token(
        &first.origin(),
        Some("acme"),
        &first.token_for("keeper").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap();
    let failure = connect_with_token(
        &second.origin(),
        Some("acme"),
        &second.token_for("keeper").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, SignInError::BadName(_)), "{failure}");
    let again = connect_with_token(
        &first.origin(),
        None,
        &first.token_for("out").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        again.source.name, "acme",
        "signing in again is not connecting anew"
    );
    assert_eq!(again.source.account, "out");
}

/// A save that fails half way leaves what was there: a new server leaves no
/// credential behind, and a server signed in to before keeps the sign-in it
/// had.
#[tokio::test]
async fn a_save_that_fails_leaves_the_credential_as_it_was() {
    let known = RemoteServer::start(Options::TOKENS).await;
    let fresh = RemoteServer::start(Options::TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let kept = connect_with_token(
        &known.origin(),
        Some("acme"),
        &known.token_for("keeper").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap()
    .source;
    let credential = kept.host_dir(dir.path()).join("credential.json");
    let before = std::fs::read(&credential).unwrap();
    // A sources file this build cannot write over: every save stops there.
    std::fs::write(
        dir.path().join("sources.json"),
        json!({ "v": 99, "sources": [] }).to_string(),
    )
    .unwrap();
    let failure = connect_with_token(
        &known.origin(),
        None,
        &known.token_for("out").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, SignInError::Store(_)), "{failure}");
    assert_eq!(
        std::fs::read(&credential).unwrap(),
        before,
        "the sign-in before is back"
    );
    let failure = connect_with_token(
        &fresh.origin(),
        None,
        &fresh.token_for("keeper").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, SignInError::Store(_)), "{failure}");
    let fresh_dir = dir
        .path()
        .join(crystalline_remote::server_key(&fresh.origin()));
    assert!(!fresh_dir.exists(), "no credential is left behind");
}

#[tokio::test]
async fn disconnect_revokes_one_source_and_leaves_the_other() {
    let oauth = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let tokens = RemoteServer::start(Options::TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let acme = connect_with_browser(
        &oauth.origin(),
        Some("acme"),
        dir.path(),
        &[],
        fake_browser(oauth.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap()
    .source;
    let beta = connect_with_token(
        &tokens.origin(),
        Some("beta"),
        &tokens.token_for("keeper").await,
        dir.path(),
        &[],
    )
    .await
    .unwrap()
    .source;
    let gone = disconnect(dir.path(), "acme").await.unwrap().unwrap();
    assert_eq!(
        (gone.name.as_str(), gone.url.as_str()),
        ("acme", oauth.origin().as_str())
    );
    assert_eq!(gone.revocation, Revocation::Revoked);
    assert_eq!(gone.note(), None, "nothing to add when it was revoked");
    assert!(
        oauth
            .auth
            .list_oauth_grants("keeper")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !acme.host_dir(dir.path()).exists(),
        "cache and credential are gone"
    );
    assert!(
        beta.host_dir(dir.path()).exists(),
        "the other source is untouched"
    );
    assert_eq!(
        load_sources(dir.path()).unwrap().names(),
        vec!["beta".to_string()]
    );
    assert!(
        disconnect(dir.path(), "acme").await.unwrap().is_none(),
        "nothing left to remove"
    );
    let by_url = disconnect(dir.path(), &tokens.origin())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        by_url.revocation,
        Revocation::NotApplicable,
        "a pasted token is only forgotten here"
    );
}

/// A credential that holds only its access token still ends the grant on the
/// server: revocation by the access token is the other half of RFC 7009.
#[tokio::test]
async fn disconnect_ends_the_grant_by_its_access_token_too() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    let source = connect_with_browser(
        &server.origin(),
        Some("acme"),
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap()
    .source;
    let path = source.host_dir(dir.path()).join("credential.json");
    let mut credential: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(credential["refresh_token"].is_string());
    credential["refresh_token"] = serde_json::Value::Null;
    std::fs::write(&path, credential.to_string()).unwrap();
    assert_eq!(
        server.auth.list_oauth_grants("keeper").await.unwrap().len(),
        1
    );

    let gone = disconnect(dir.path(), "acme").await.unwrap().unwrap();
    assert_eq!(gone.revocation, Revocation::Revoked);
    assert!(
        server
            .auth
            .list_oauth_grants("keeper")
            .await
            .unwrap()
            .is_empty(),
        "the grant is gone on the server"
    );
}

#[tokio::test]
async fn disconnect_with_the_server_down_still_forgets_locally() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    let source = connect_with_browser(
        &server.origin(),
        Some("acme"),
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap()
    .source;
    server.stop().await;
    let gone = disconnect(dir.path(), "acme").await.unwrap().unwrap();
    assert_eq!(
        gone.revocation,
        Revocation::NotReached("nothing accepts connections at that address".to_string())
    );
    let note = gone.note().unwrap();
    for words in [
        "acme",
        server.origin().as_str(),
        "VPN or the network",
        "forgotten",
        "revoked",
        "Connected clients",
    ] {
        assert!(note.contains(words), "{words}: {note}");
    }
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
    assert!(
        !source.host_dir(dir.path()).exists(),
        "credential and cache are gone"
    );
}

/// A revocation endpoint that accepts the connection and never answers ends
/// at the limit, and the source is forgotten all the same.
#[tokio::test]
async fn disconnect_from_a_server_that_never_answers_ends_at_the_limit() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let (hole, accepted) = blackhole().await;
    let dir = tempfile::tempdir().unwrap();
    let source = connect_with_browser(
        &server.origin(),
        Some("acme"),
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap()
    .source;
    update_sources(dir.path(), |file| {
        file.find_mut("acme").unwrap().revocation_endpoint =
            Some(format!("{hole}/api/v1/oauth/revoke"));
        Ok(())
    })
    .unwrap();
    let started = Instant::now();
    let gone = disconnect_within(dir.path(), "acme", LIMIT)
        .await
        .unwrap()
        .unwrap();
    assert!(
        started.elapsed() < LIMIT + Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 1, "it did ask");
    assert_eq!(
        gone.revocation,
        Revocation::NotReached("it did not answer within 0.3 s".to_string())
    );
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
    assert!(!source.host_dir(dir.path()).exists());
}

#[tokio::test]
async fn a_server_that_never_answers_ends_the_sign_in_at_the_limit() {
    warm_up();
    let (url, accepted) = blackhole().await;
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let failure = connect_with_token_within(&url, Some("acme"), "cmt_x", dir.path(), &[], LIMIT)
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < LIMIT + Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(
        failure,
        SignInError::Unreachable {
            url: url.clone(),
            detail: "it did not answer within 0.3 s".to_string(),
        }
    );
    let said = failure.to_string();
    for words in [
        url.as_str(),
        "cannot be reached right now",
        "VPN or the network",
        "nothing was changed",
    ] {
        assert!(said.contains(words), "{words}: {said}");
    }
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "not even a folder is left"
    );
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_unreachable_quickly() {
    warm_up();
    let url = "https://crystalline-remote-test.invalid";
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let failure = connect_with_token(url, None, "cmt_x", dir.path(), &[])
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        failure,
        SignInError::Unreachable {
            url: url.to_string(),
            detail: "the name crystalline-remote-test.invalid does not resolve".to_string(),
        }
    );
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
}

#[tokio::test]
async fn a_browser_that_never_comes_back_saves_nothing() {
    let server = RemoteServer::start(Options::OAUTH).await;
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser_within(
        &server.origin(),
        Some("acme"),
        dir.path(),
        &[],
        |_url: &str| {},
        no_paste,
        LIMIT,
    )
    .await
    .unwrap_err();
    assert_eq!(failure, SignInError::TimedOut);
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
    assert!(
        server
            .auth
            .list_oauth_grants("keeper")
            .await
            .unwrap()
            .is_empty()
    );
}
