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
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::fixture::{Options, Proxy, RemoteServer};

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

/// The port the authorization URL asks the browser to come back to.
fn callback_port(authorize_url: &str) -> u16 {
    let raw = authorize_url
        .split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("redirect_uri="))
        .unwrap();
    let redirect = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .unwrap()
        .to_string();
    reqwest::Url::parse(&redirect).unwrap().port().unwrap()
}

/// [`fake_browser`] that drops `iss` from the answer on its way back.
fn browser_without_iss(server: Arc<RemoteServer>) -> impl FnOnce(&str) + Send {
    move |authorize_url: &str| {
        let url = authorize_url.to_string();
        tokio::spawn(async move {
            let location = server.consent_with("keeper", &url, "allow").await;
            let (base, query) = location.split_once('?').unwrap();
            let kept: Vec<&str> = query
                .split('&')
                .filter(|pair| !pair.starts_with("iss="))
                .collect();
            let _ = reqwest::get(format!("{base}?{}", kept.join("&"))).await;
        });
    }
}

/// A stand-in Crystalline whose `/health` answers and whose two OAuth
/// documents are what `documents` makes of its origin: the
/// protected-resource status and body, and the authorization server
/// metadata.
async fn stub(documents: impl FnOnce(&str) -> (u16, Value, Value)) -> String {
    use axum::routing::get;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (status, resource, meta) = documents(&origin);
    let router = axum::Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(json!({ "status": "ok" })) }),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(move || {
                let resource = resource.clone();
                async move {
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        axum::Json(resource),
                    )
                }
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(move || {
                let meta = meta.clone();
                async move { axum::Json(meta) }
            }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    origin
}

/// Metadata in the shape this server publishes, for `issuer`.
fn metadata(issuer: &str) -> Value {
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/api/v1/oauth/authorize"),
        "token_endpoint": format!("{issuer}/api/v1/oauth/token"),
        "registration_endpoint": format!("{issuer}/api/v1/oauth/register"),
        "revocation_endpoint": format!("{issuer}/api/v1/oauth/revoke"),
    })
}

/// A front for `backend` that passes every GET through and answers the
/// control protocol with a 500: the documents name the backend (they follow
/// the Host they are asked with), so the browser flow runs against it, and
/// only the account check after the code exchange fails.
async fn front_with_a_failing_ctl(backend: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = axum::Router::new()
        .route(
            "/api/v1/ctl",
            axum::routing::post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .fallback(move |uri: axum::http::Uri| {
            let backend = backend.clone();
            async move {
                let answer = reqwest::get(format!("{backend}{uri}")).await.unwrap();
                let status = axum::http::StatusCode::from_u16(answer.status().as_u16()).unwrap();
                (
                    status,
                    [("content-type", "application/json")],
                    answer.text().await.unwrap(),
                )
            }
        });
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    origin
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
        &format!("{}/", server.origin()),
        Some("acme"),
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap();
    let source = &connected.source;
    assert_eq!(
        source.url,
        server.origin(),
        "the base address, no trailing slash"
    );
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
    assert_eq!(failure, SignInError::Denied("access_denied".to_string()));
    assert_eq!(
        failure.to_string(),
        "the sign-in was not allowed (access_denied); nothing was saved",
        "the code and nothing the server wrote beside it"
    );
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
    let fresh_dir = dir.path().join(crystalline_remote::server_folder(
        &crystalline_remote::server_key(&fresh.origin()),
    ));
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

/// A server that answers the revocation with an error was reached: the
/// note names its answer and does not send the person to check the network.
#[tokio::test]
async fn a_revocation_the_server_refuses_is_not_called_unreachable() {
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
    let origin = server.origin();
    update_sources(dir.path(), |file| {
        file.find_mut("acme").unwrap().revocation_endpoint =
            Some(format!("{origin}/api/v1/oauth/no-such-endpoint"));
        Ok(())
    })
    .unwrap();
    let gone = disconnect(dir.path(), "acme").await.unwrap().unwrap();
    let Revocation::Refused(status) = gone.revocation else {
        panic!(
            "a server that answered is not unreachable: {:?}",
            gone.revocation
        );
    };
    assert!(status >= 400, "{status}");
    let note = gone.note().unwrap();
    assert!(
        note.contains(&format!("answered {status}"))
            && note.contains("forgotten")
            && !note.contains("VPN"),
        "{note}"
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
        !dir.path()
            .join(crystalline_remote::server_folder(
                &crystalline_remote::server_key(&server.origin())
            ))
            .exists(),
        "no host folder"
    );
}

/// Review I1: a sign-in that fails after the code exchange ends the grant it
/// was issued, so failed tries do not pile up under Connected clients.
#[tokio::test]
async fn a_failed_account_check_after_the_exchange_leaves_no_grant() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let front = front_with_a_failing_ctl(server.origin()).await;
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser(
        &front,
        Some("acme"),
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, SignInError::Protocol(_)), "{failure}");
    assert!(
        server
            .auth
            .list_oauth_grants("keeper")
            .await
            .unwrap()
            .is_empty(),
        "the grant was ended"
    );
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
}

/// Review I1, the save: a sources file that cannot be written fails the
/// sign-in after the exchange, and the grant is ended too.
#[tokio::test]
async fn a_failed_save_after_the_exchange_leaves_no_grant() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("sources.json"),
        json!({ "v": 99, "sources": [] }).to_string(),
    )
    .unwrap();
    let failure = connect_with_browser(
        &server.origin(),
        None,
        dir.path(),
        &[],
        fake_browser(server.clone(), "keeper", "allow"),
        no_paste,
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, SignInError::Store(_)), "{failure}");
    assert!(
        server
            .auth
            .list_oauth_grants("keeper")
            .await
            .unwrap()
            .is_empty()
    );
}

/// Review M6: signing in again replaces the grant instead of adding one.
#[tokio::test]
async fn signing_in_again_ends_the_grant_it_replaces() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        connect_with_browser(
            &server.origin(),
            Some("acme"),
            dir.path(),
            &[],
            fake_browser(server.clone(), "keeper", "allow"),
            no_paste,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        server.auth.list_oauth_grants("keeper").await.unwrap().len(),
        1,
        "only the new grant is left"
    );
    let source = load_sources(dir.path()).unwrap().sources[0].clone();
    let connection = Connection::open(source, dir.path()).unwrap();
    let data = connection
        .ctl_data(json!({ "v": 1, "cmd": "status" }))
        .await
        .unwrap();
    assert_eq!(data["account"], "keeper", "and it is the one saved");
}

/// Review I2: an endpoint over plain http off this machine is refused, by
/// the address the person typed and without the server's words.
#[tokio::test]
async fn a_token_endpoint_over_plain_http_is_refused() {
    let origin = stub(|origin| {
        let mut meta = metadata(origin);
        meta["token_endpoint"] = json!("http://kb.example/api/v1/oauth/token");
        (
            200,
            json!({ "resource": origin, "authorization_servers": [origin] }),
            meta,
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser(
        &origin,
        None,
        dir.path(),
        &[],
        |_url: &str| panic!("no browser for endpoints that are not secure"),
        no_paste,
    )
    .await
    .unwrap_err();
    assert_eq!(
        failure,
        SignInError::InsecureEndpoints {
            url: origin.clone()
        }
    );
    let said = failure.to_string();
    assert!(
        said.contains("not secure") && said.contains("--token") && !said.contains("kb.example"),
        "{said}"
    );
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
}

/// Review I2: metadata that names another issuer than the one it was
/// fetched for is refused (RFC 8414 section 3.3).
#[tokio::test]
async fn metadata_for_another_issuer_is_refused() {
    let origin = stub(|origin| {
        let mut meta = metadata(origin);
        meta["issuer"] = json!("https://elsewhere.example");
        (
            200,
            json!({ "resource": origin, "authorization_servers": [origin] }),
            meta,
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser(
        &origin,
        None,
        dir.path(),
        &[],
        |_url: &str| panic!("no browser for a mismatched issuer"),
        no_paste,
    )
    .await
    .unwrap_err();
    assert_eq!(failure, SignInError::InsecureEndpoints { url: origin });
    assert!(!failure.to_string().contains("elsewhere"), "{failure}");
}

/// Review M5: only a missing protected-resource document means no browser
/// sign-in; a server that fails to answer it is not asked for a token.
#[tokio::test]
async fn a_failing_protected_resource_document_is_not_a_server_without_oauth() {
    let origin = stub(|origin| (500, json!({}), metadata(origin))).await;
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser(
        &origin,
        None,
        dir.path(),
        &[],
        |_url: &str| panic!("no browser"),
        |_note: String| panic!("no token prompt for a server that failed"),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&failure, SignInError::Protocol(text) if text.contains("500")),
        "{failure}"
    );
}

/// Review M1: the server says every answer carries `iss`, so one without it
/// is refused.
#[tokio::test]
async fn an_answer_without_iss_is_refused_when_the_server_promises_it() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser(
        &server.origin(),
        None,
        dir.path(),
        &[],
        browser_without_iss(server.clone()),
        no_paste,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&failure, SignInError::Protocol(text) if text.contains("another server")),
        "{failure}"
    );
    assert!(load_sources(dir.path()).unwrap().sources.is_empty());
}

/// Review M2: a refusal that is not this sign-in's answer is not taken as
/// the person's no.
#[tokio::test]
async fn a_stray_refusal_is_another_sign_ins_answer() {
    let server = RemoteServer::start(Options::OAUTH).await;
    let dir = tempfile::tempdir().unwrap();
    let failure = connect_with_browser(
        &server.origin(),
        None,
        dir.path(),
        &[],
        |authorize_url: &str| {
            let port = callback_port(authorize_url);
            tokio::spawn(async move {
                let _ = reqwest::get(format!(
                    "http://127.0.0.1:{port}/callback?error=access_denied&state=not-this-one"
                ))
                .await;
            });
        },
        no_paste,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&failure, SignInError::Protocol(text) if text.contains("another sign-in")),
        "{failure}"
    );
}

/// Review M3: a connection the browser opens and sends nothing on does not
/// hold the loopback port; the real answer behind it is served.
#[tokio::test]
async fn a_silent_connection_to_the_loopback_port_does_not_hold_the_sign_in() {
    let server = Arc::new(RemoteServer::start(Options::OAUTH).await);
    let dir = tempfile::tempdir().unwrap();
    let browser = {
        let server = server.clone();
        move |authorize_url: &str| {
            let port = callback_port(authorize_url);
            // Opened first and held open, silent, the way a preconnect is.
            let silent = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let url = authorize_url.to_string();
            tokio::spawn(async move {
                let location = server.consent_with("keeper", &url, "allow").await;
                let _ = reqwest::get(location).await;
                drop(silent);
            });
        }
    };
    let connected = connect_with_browser_within(
        &server.origin(),
        Some("acme"),
        dir.path(),
        &[],
        browser,
        no_paste,
        Duration::from_secs(8),
    )
    .await
    .unwrap();
    assert_eq!(connected.source.account, "keeper");
}

#[tokio::test]
async fn signing_in_through_the_browser_under_a_prefix_saves_the_base() {
    for proxy in [Proxy::Strips, Proxy::PassesThrough] {
        let server =
            Arc::new(RemoteServer::start(Options::OAUTH.under("/crystalline", proxy)).await);
        let dir = tempfile::tempdir().unwrap();
        let connected = connect_with_browser(
            &format!("{}/", server.base()),
            None,
            dir.path(),
            &[],
            fake_browser(server.clone(), "keeper", "allow"),
            no_paste,
        )
        .await
        .unwrap();
        let source = &connected.source;
        assert_eq!(
            source.url,
            server.base(),
            "{proxy:?}: the base, no trailing slash"
        );
        assert_eq!(
            source.token_endpoint.as_deref(),
            Some(format!("{}/api/v1/oauth/token", server.base()).as_str()),
            "{proxy:?}"
        );
        assert_eq!(
            source.revocation_endpoint.as_deref(),
            Some(format!("{}/api/v1/oauth/revoke", server.base()).as_str()),
            "{proxy:?}"
        );
        assert!(
            source.host_dir(dir.path()).ends_with(format!(
                "{}~crystalline",
                server.addr.to_string().replace(':', "_")
            )),
            "{proxy:?}: {:?}",
            source.host_dir(dir.path())
        );
        assert!(source.host_dir(dir.path()).join("credential.json").exists());
        let data = Connection::open(source.clone(), dir.path())
            .unwrap()
            .ctl_data(json!({ "v": 1, "cmd": "status" }))
            .await
            .unwrap();
        assert_eq!(data["account"], "keeper", "{proxy:?}");
    }
}

#[tokio::test]
async fn a_pasted_token_under_a_prefix_adds_the_source() {
    let server = RemoteServer::start(Options::TOKENS.under("/crystalline", Proxy::Strips)).await;
    let token = server.token_for("keeper").await;
    let dir = tempfile::tempdir().unwrap();
    let connected = connect_with_token(&server.base(), None, &token, dir.path(), &[])
        .await
        .unwrap();
    assert_eq!(connected.source.url, server.base());
    assert_eq!(connected.source.account, "keeper");
}

#[tokio::test]
async fn two_paths_on_one_host_are_two_sources_and_the_second_keeps_the_first() {
    let (a, b) = RemoteServer::start_two_on_one_host(Options::TOKENS, "/team-a", "/team-b").await;
    let dir = tempfile::tempdir().unwrap();
    let token_a = a.token_for("keeper").await;
    let token_b = b.token_for("keeper").await;
    let first = connect_with_token(&a.base(), None, &token_a, dir.path(), &[])
        .await
        .unwrap();
    let second = connect_with_token(&b.base(), None, &token_b, dir.path(), &[])
        .await
        .unwrap();
    assert_eq!(first.source.url, a.base());
    assert_eq!(second.source.url, b.base());
    assert_eq!(
        (first.source.name.as_str(), second.source.name.as_str()),
        ("server", "server-2")
    );
    let saved = load_sources(dir.path()).unwrap();
    assert_eq!(
        saved.sources.len(),
        2,
        "the second connect never retires the first"
    );
    assert_ne!(first.source.key(), second.source.key());
    assert_ne!(
        crystalline_remote::server_token::server_account(&first.source.key()),
        crystalline_remote::server_token::server_account(&second.source.key()),
        "two keychain entries"
    );
    for source in [&first.source, &second.source] {
        assert!(
            source.host_dir(dir.path()).join("credential.json").exists(),
            "{}",
            source.url
        );
        let data = Connection::open(source.clone(), dir.path())
            .unwrap()
            .ctl_data(json!({ "v": 1, "cmd": "status" }))
            .await
            .unwrap();
        assert_eq!(data["account"], "keeper");
    }
}

#[tokio::test]
async fn a_source_saved_by_0_23_0_still_opens_after_the_upgrade() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = server.token_for("keeper").await;
    let dir = tempfile::tempdir().unwrap();
    // The two files exactly as 0.23.0 wrote them for a root server: the
    // record's url is the origin and the folder is the bare server key.
    let folder = dir.path().join(server.addr.to_string().replace(':', "_"));
    std::fs::create_dir_all(&folder).unwrap();
    crystalline_remote::ServerCredentialStore::file(&folder)
        .save(&crystalline_remote::ServerCredential::token(
            token,
            server.origin(),
            "keeper".into(),
            chrono::Utc::now(),
        ))
        .unwrap();
    std::fs::write(
        dir.path().join("sources.json"),
        serde_json::to_vec_pretty(&json!({ "v": 1, "sources": [{
            "url": server.origin(), "name": "server", "account": "keeper", "kind": "token",
            "connected_at": "2026-10-05T12:00:00Z", "mounts": []
        }]}))
        .unwrap(),
    )
    .unwrap();
    let file = load_sources(dir.path()).unwrap();
    let source = file.find("server").unwrap().clone();
    assert_eq!(source.host_dir(dir.path()), folder, "the 0.23.0 folder");
    let data = Connection::open(source, dir.path())
        .unwrap()
        .ctl_data(json!({ "v": 1, "cmd": "status" }))
        .await
        .unwrap();
    assert_eq!(data["account"], "keeper");
}
