//! The first-run setup endpoint, driven over a real loopback socket against a
//! deliberately account-less instance.
//!
//! Its own suite rather than rows in the write matrix (`rest_write_api.rs`),
//! for the reason that file's exemption comment records: every fixture there
//! has accounts, and this route answers 410 forever once one exists, so every
//! matrix leg would assert the same refusal. Here the store starts empty, which
//! is the only state in which the endpoint does anything at all.
//!
//! The router is served with `into_make_service_with_connect_info`, because the
//! handler's locality decision reads the socket peer address out of the request
//! extensions and fails closed without one - a harness that served the plain
//! router would exercise only the refusal path.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{AuthConfig, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, RestState, Role, router};
use serde_json::{Value, json};
use tokio::sync::Mutex;

/// A realistic token: 32 hex characters, the shape `serve` prints. Long enough
/// that "the refusal does not echo the token" is a meaningful assertion, which
/// a three-letter stand-in would not be (it would appear inside the word
/// "token" in any sensible message).
const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";

/// What a setup-test server varies. Everything else is the fixture below: no
/// domains, no accounts, plain JSON responses.
#[derive(Default)]
struct Options {
    /// The one-time token this process holds, as `serve` would have generated
    /// it for a non-loopback bind. `None` is the loopback case: the token path
    /// is closed.
    setup_token: Option<String>,
    /// `service.read_only`: content mutations are refused, accounts are not.
    read_only: bool,
    /// `auth.anonymous`: serve a request that carries no identity.
    anonymous: bool,
}

struct Fixture {
    addr: SocketAddr,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

/// An account-less instance: the engine and the auth database both fixtures
/// below are built on.
///
/// No domain is registered: nothing here reads content, and the one ordinary
/// write these tests re-assert against (`POST /users`) needs none.
async fn instance(opts: &Options) -> (Arc<Engine>, Arc<AuthStore>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = GlobalConfig {
        auth: Some(AuthConfig {
            trusted_header: None,
            anonymous: Some(opts.anonymous),
            max_users: None,
        }),
        service: Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            read_only: Some(opts.read_only),
            ..ServiceConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let store = TursoStore::open_in_memory().await.unwrap();
    // `service.read_only` is resolved by the daemon rather than by the engine's
    // constructor, so the fixture applies it the way `serve` does.
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, None).with_read_only(opts.read_only),
    );
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    (engine, auth, tmp)
}

/// Serve the REST router over an account-less instance.
async fn serve(opts: Options) -> Fixture {
    let (engine, auth, tmp) = instance(&opts).await;
    let state = RestState::new(engine, auth.clone())
        .unwrap()
        .with_setup_token(opts.setup_token);
    let app = axum::Router::new().nest("/api/v1", router(state));
    Fixture {
        addr: spawn(app),
        auth,
        _tmp: tmp,
    }
}

/// The production router (`daemon::http_router`), served the way `run_http`
/// serves it: with connect info, and with the token `serve` generated for a
/// non-loopback bind threaded in through its parameter.
///
/// [`serve`] above builds `RestState` itself, which proves the handler. This one
/// proves the WIRING: that the token reaches the handler through the production
/// constructor at all, and that the peer address the locality decision rests on
/// exists on a request the daemon's own `axum::serve` answered.
async fn serve_production(setup_token: Option<String>) -> Fixture {
    let (engine, auth, tmp) = instance(&Options::default()).await;
    let router = http_router(
        engine,
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        setup_token,
    )
    .unwrap();
    Fixture {
        addr: spawn(router),
        auth,
        _tmp: tmp,
    }
}

/// Bind an ephemeral loopback port and serve the router on a background task
/// with `into_make_service_with_connect_info`, which is what puts the peer
/// address in the extensions the handler reads.
fn spawn(app: axum::Router) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

/// A client with proxy discovery disabled: the target is loopback, where a
/// system proxy must never be consulted anyway, and reqwest's platform proxy
/// lookup can block for a minute on a machine with a managed network
/// configuration.
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// `POST /auth/setup` with a JSON body and nothing else: the shape the wizard
/// sends from a browser on the machine that serves the instance.
async fn setup(addr: SocketAddr, body: Value) -> reqwest::Response {
    client()
        .post(format!("http://{addr}/api/v1/auth/setup"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

/// The same POST, made to look like it arrived through a proxy: any
/// forwarded-shaped header makes the request not local however loopback the
/// peer address is.
async fn setup_forwarded(addr: SocketAddr, body: Value) -> reqwest::Response {
    client()
        .post(format!("http://{addr}/api/v1/auth/setup"))
        .header("x-forwarded-for", "203.0.113.9")
        .json(&body)
        .send()
        .await
        .unwrap()
}

/// `GET /auth/me`, optionally on a session.
async fn me(addr: SocketAddr, cookie: Option<&str>) -> Value {
    let mut req = client().get(format!("http://{addr}/api/v1/auth/me"));
    if let Some(token) = cookie {
        req = req.header("cookie", format!("fluid_session={token}"));
    }
    let resp = req.send().await.unwrap();
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

/// The `fluid_session` value out of a response's `Set-Cookie`, attributes
/// dropped.
fn session_cookie(resp: &reqwest::Response) -> Option<String> {
    set_cookie(resp)?
        .split(';')
        .next()
        .and_then(|v| v.trim().strip_prefix("fluid_session=").map(str::to_string))
}

/// The raw `Set-Cookie` header for `fluid_session`, attributes and all.
fn set_cookie(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .map(str::to_string)
}

/// A response's problem document, checking the media type on the way through:
/// every refusal on this surface is `application/problem+json`.
async fn problem(resp: reqwest::Response) -> Value {
    assert_eq!(
        resp.headers()[reqwest::header::CONTENT_TYPE],
        "application/problem+json",
        "a refusal is a problem document like every other one here"
    );
    resp.json().await.unwrap()
}

/// The probe answers `needs_setup: true` exactly while no account exists, by
/// either route in: the wizard's own POST or `crystalline users add`.
#[tokio::test]
async fn me_reports_needs_setup_until_an_account_exists() {
    let fixture = serve(Options::default()).await;
    let probe = me(fixture.addr, None).await;
    assert_eq!(probe["needs_setup"], true);
    assert_eq!(probe["user"], Value::Null, "and nobody is signed in yet");

    let resp = setup(fixture.addr, json!({"name": "root", "password": "rootpw"})).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(me(fixture.addr, None).await["needs_setup"], false);

    // The CLI path closes it just as well: the flag is about accounts, not
    // about this endpoint having been used.
    let seeded = serve(Options::default()).await;
    assert_eq!(me(seeded.addr, None).await["needs_setup"], true);
    seeded
        .auth
        .add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();
    assert_eq!(me(seeded.addr, None).await["needs_setup"], false);
}

/// The success path: an admin account, signed in, in the login response shape.
#[tokio::test]
async fn setup_creates_a_signed_in_admin() {
    let fixture = serve(Options::default()).await;
    let resp = setup(fixture.addr, json!({"name": "Root", "password": "rootpw"})).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()[reqwest::header::CACHE_CONTROL],
        "no-store",
        "the answer carries a session cookie and a CSRF token"
    );
    let raw_cookie = set_cookie(&resp).expect("setup sets the session cookie");
    assert!(
        raw_cookie.to_ascii_lowercase().contains("httponly"),
        "the session cookie is HttpOnly: {raw_cookie}"
    );
    let token = session_cookie(&resp).unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["user"]["name"], "root", "the folded login name");
    assert_eq!(body["user"]["display"], "Root", "display as typed");
    assert_eq!(body["user"]["role"], "admin");
    assert_eq!(body["user"]["disabled"], false);
    let csrf = body["csrf"].as_str().expect("a CSRF token comes back");
    assert!(!csrf.is_empty());

    // The session is live: the probe names the account it belongs to.
    let probe = me(fixture.addr, Some(&token)).await;
    assert_eq!(probe["user"]["name"], "root");
    assert_eq!(probe["needs_setup"], false);

    // And it is a session that may write: cookie plus CSRF token passes the
    // guard on an admin-only mutating route.
    let created = client()
        .post(format!("http://{}/api/v1/users", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .header("x-csrf-token", csrf)
        .json(&json!({"name": "ada", "role": "viewer", "password": "s3cret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201, "the new admin can manage accounts");
}

/// Decision 1: once any account exists the slot is gone, permanently, and the
/// refusal says what to do instead.
#[tokio::test]
async fn setup_is_gone_once_any_account_exists() {
    let fixture = serve(Options::default()).await;
    assert_eq!(
        setup(fixture.addr, json!({"name": "root", "password": "rootpw"}))
            .await
            .status(),
        200
    );
    let resp = setup(fixture.addr, json!({"name": "boss", "password": "bosspw"})).await;
    assert_eq!(resp.status(), 410);
    let doc = problem(resp).await;
    assert_eq!(doc["status"], 410);
    assert!(
        doc["detail"].as_str().unwrap().contains("log in"),
        "the refusal points at the login form: {}",
        doc["detail"]
    );
    assert_eq!(fixture.auth.user_count().await.unwrap(), 1);

    // An instance seeded only through the CLI answers the same way: the gate is
    // "any account", not "an account this endpoint created".
    let seeded = serve(Options::default()).await;
    seeded
        .auth
        .add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();
    let resp = setup(seeded.addr, json!({"name": "root", "password": "rootpw"})).await;
    assert_eq!(resp.status(), 410);
    assert_eq!(seeded.auth.user_count().await.unwrap(), 1);

    // The order the handler checks in is a security property, so it is asserted
    // rather than read off the code: an instance that HOLDS a token, probed
    // from a non-local caller carrying none, hears only "gone". Were locality
    // checked first, that prober would instead be handed a 403 whose
    // `token_required` member tells it a setup token exists to be guessed at.
    let configured = serve(Options {
        setup_token: Some(TOKEN.to_string()),
        ..Options::default()
    })
    .await;
    configured
        .auth
        .add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();
    let resp = setup_forwarded(
        configured.addr,
        json!({"name": "root", "password": "rootpw"}),
    )
    .await;
    assert_eq!(resp.status(), 410, "the account gate is checked first");
    let doc = problem(resp).await;
    assert!(
        doc.get("token_required").is_none(),
        "and a remote prober learns nothing about this instance's token: {doc}"
    );
}

/// The handler half of invariant 1: whatever arrives at once, exactly one
/// caller is answered 200 and the rest are told the slot is gone.
///
/// Single-process only, and deliberately so: all sixteen requests serialize on
/// the one `AuthStore`'s guard mutex, so this would pass even against a naive
/// check-then-insert. What it pins is the handler's mapping of a lost race onto
/// the same 410 an outright refusal gets. The cross-process claim - that the
/// store's conditional insert is what makes the slot atomic - is pinned in
/// `auth_store.rs` by `a_second_store_open_cannot_also_win_first_admin`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_of_many_concurrent_setups_wins() {
    let fixture = serve(Options::default()).await;
    let mut tasks = Vec::new();
    for i in 0..16 {
        let addr = fixture.addr;
        tasks.push(tokio::spawn(async move {
            setup(addr, json!({"name": format!("admin{i}"), "password": "pw"}))
                .await
                .status()
                .as_u16()
        }));
    }
    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.unwrap());
    }
    assert_eq!(
        statuses.iter().filter(|s| **s == 200).count(),
        1,
        "exactly one setup wins: {statuses:?}"
    );
    assert_eq!(
        statuses.iter().filter(|s| **s == 410).count(),
        15,
        "and every other one is told the slot is gone: {statuses:?}"
    );
    assert_eq!(fixture.auth.user_count().await.unwrap(), 1);
}

/// Invariant 2: the peer address decides locality, and a forwarded-shaped
/// header may only take it away.
///
/// A loopback test socket cannot mint a non-loopback peer, so the other half of
/// the rule - an actually remote peer, and a missing one - is pinned by the
/// unit test of `is_local_setup_peer` in `rest/auth.rs`.
#[tokio::test]
async fn locality_is_the_peer_address_and_headers_only_narrow_it() {
    let fixture = serve(Options::default()).await;
    // No token configured, no token sent, loopback peer: served.
    assert_eq!(
        setup(fixture.addr, json!({"name": "root", "password": "rootpw"}))
            .await
            .status(),
        200
    );

    for header in ["x-forwarded-for", "x-forwarded-proto", "forwarded"] {
        let fixture = serve(Options {
            setup_token: Some(TOKEN.to_string()),
            ..Options::default()
        })
        .await;
        let value = if header == "forwarded" {
            "for=203.0.113.9"
        } else if header == "x-forwarded-proto" {
            "https"
        } else {
            "203.0.113.9"
        };
        let resp = client()
            .post(format!("http://{}/api/v1/auth/setup", fixture.addr))
            .header(header, value)
            .json(&json!({"name": "root", "password": "rootpw"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            403,
            "{header} means a proxy relayed this, so it is not local"
        );
        let doc = problem(resp).await;
        assert_eq!(doc["token_required"], true, "and the token is the way in");
        assert_eq!(fixture.auth.user_count().await.unwrap(), 0);
    }
}

/// Invariant 3, the handler's half: a non-local caller needs the token this
/// process holds, compared without leaking it, and a process that holds none
/// says so without inventing one.
#[tokio::test]
async fn the_setup_token_is_required_and_compared_for_a_non_local_caller() {
    let fixture = serve(Options {
        setup_token: Some(TOKEN.to_string()),
        ..Options::default()
    })
    .await;

    let missing =
        setup_forwarded(fixture.addr, json!({"name": "root", "password": "rootpw"})).await;
    assert_eq!(missing.status(), 403);
    let missing = problem(missing).await;
    let wrong = setup_forwarded(
        fixture.addr,
        json!({"name": "root", "password": "rootpw", "token": "0000000000000000000000000000face"}),
    )
    .await;
    assert_eq!(wrong.status(), 403);
    let wrong = problem(wrong).await;
    assert_eq!(
        missing["detail"], wrong["detail"],
        "one message for both, so nothing says whether the token was close"
    );
    for doc in [&missing, &wrong] {
        assert_eq!(
            doc["token_required"], true,
            "the wizard keys its token field on this member, never on the prose"
        );
        assert!(
            !doc["detail"].as_str().unwrap().contains(TOKEN),
            "the refusal never echoes the token: {}",
            doc["detail"]
        );
    }
    assert_eq!(fixture.auth.user_count().await.unwrap(), 0);

    let right = setup_forwarded(
        fixture.addr,
        json!({"name": "root", "password": "rootpw", "token": TOKEN}),
    )
    .await;
    assert_eq!(right.status(), 200, "the right token is the way in");
    assert_eq!(
        right.json::<Value>().await.unwrap()["user"]["role"],
        "admin"
    );

    // A loopback bind generates no token at all. The refusal then carries no
    // `token_required` member, so the wizard never renders a dead-end field,
    // and the prose does not point at a token that does not exist.
    let tokenless = serve(Options::default()).await;
    let resp = setup_forwarded(
        tokenless.addr,
        json!({"name": "root", "password": "rootpw"}),
    )
    .await;
    assert_eq!(resp.status(), 403);
    let doc = problem(resp).await;
    assert!(
        doc.get("token_required").is_none(),
        "no token exists here, so the member must be absent: {doc}"
    );
    assert!(
        doc["detail"].as_str().unwrap().contains("machine"),
        "it says to run setup from the machine that serves this instance: {}",
        doc["detail"]
    );
    // The wrong token is no better than none when there is nothing to compare
    // against.
    let resp = setup_forwarded(
        tokenless.addr,
        json!({"name": "root", "password": "rootpw", "token": TOKEN}),
    )
    .await;
    assert_eq!(resp.status(), 403);
    assert_eq!(tokenless.auth.user_count().await.unwrap(), 0);
}

/// A blank configured token is no token: it must never match the empty string a
/// caller who sends nothing is compared as, which would hand first-run setup to
/// every remote caller on the network.
///
/// Nothing generates a blank token today, which is exactly why this is pinned:
/// the failure is silent, it is the worst one this endpoint has, and the
/// plan's own named follow-up (a `--setup-token` flag for provisioning scripts)
/// would put an operator's empty string one flag away from reaching it.
#[tokio::test]
async fn a_blank_setup_token_is_no_token_rather_than_a_skeleton_key() {
    for blank in ["", "   "] {
        let fixture = serve(Options {
            setup_token: Some(blank.to_string()),
            ..Options::default()
        })
        .await;
        // A remote caller sending nothing: refused, and told the truth - this
        // instance holds no token, so no token field may be offered.
        let resp =
            setup_forwarded(fixture.addr, json!({"name": "root", "password": "rootpw"})).await;
        assert_eq!(resp.status(), 403, "a blank token admits nobody");
        let doc = problem(resp).await;
        assert!(
            doc.get("token_required").is_none(),
            "a blank token is no token, so no token field is offered: {doc}"
        );
        // And sending the blank value back does not match it either.
        for presented in ["", "   "] {
            let resp = setup_forwarded(
                fixture.addr,
                json!({"name": "root", "password": "rootpw", "token": presented}),
            )
            .await;
            assert_eq!(resp.status(), 403, "{presented:?} is not a token either");
        }
        assert_eq!(fixture.auth.user_count().await.unwrap(), 0);
        // The local path is untouched: a blank token closes the token path, not
        // the endpoint.
        assert_eq!(
            setup(fixture.addr, json!({"name": "root", "password": "rootpw"}))
                .await
                .status(),
            200
        );
    }
}

/// Decision 6: setup is CSRF-exempt by path, exactly as login is, and nothing
/// else moved.
#[tokio::test]
async fn setup_needs_no_csrf_but_everything_else_still_does() {
    let fixture = serve(Options::default()).await;
    // No cookie, no CSRF header, no token: this is the pre-session request the
    // wizard sends, and it is served.
    let resp = setup(fixture.addr, json!({"name": "root", "password": "rootpw"})).await;
    assert_eq!(resp.status(), 200);
    let token = session_cookie(&resp).unwrap();

    // The same session on an ordinary write, without the token it now has:
    // still refused. The rest of the matrix is `rest_write_api.rs`'s job.
    let refused = client()
        .post(format!("http://{}/api/v1/users", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .json(&json!({"name": "ada", "role": "viewer", "password": "s3cret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert!(
        problem(refused).await["detail"]
            .as_str()
            .unwrap()
            .contains("x-csrf-token")
    );
}

/// Decision 21: accounts are not content, so a read-only mirror can still be
/// given its first admin - otherwise its UI is locked shut forever - while
/// every content and account mutation stays refused there.
#[tokio::test]
async fn setup_works_on_a_read_only_instance() {
    let fixture = serve(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    let probe = me(fixture.addr, None).await;
    assert_eq!(probe["read_only"], true);
    assert_eq!(probe["needs_setup"], true);

    let resp = setup(fixture.addr, json!({"name": "root", "password": "rootpw"})).await;
    assert_eq!(resp.status(), 200, "the first admin is creatable here");
    let token = session_cookie(&resp).unwrap();
    let csrf = resp.json::<Value>().await.unwrap()["csrf"]
        .as_str()
        .unwrap()
        .to_string();

    let refused = client()
        .post(format!("http://{}/api/v1/users", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .header("x-csrf-token", csrf)
        .json(&json!({"name": "ada", "role": "viewer", "password": "s3cret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        403,
        "and the carve-out stops there: writes are still refused"
    );
}

/// The endpoint fails the way the rest of the surface does: problem+json, and
/// the shared empty-password rule rather than a second spelling of it.
#[tokio::test]
async fn setup_serves_problem_json_like_the_rest() {
    let fixture = serve(Options::default()).await;
    let resp = setup(fixture.addr, json!({"name": "root", "password": ""})).await;
    assert_eq!(resp.status(), 422);
    let doc = problem(resp).await;
    assert!(
        doc["detail"].as_str().unwrap().contains("password"),
        "{}",
        doc["detail"]
    );
    assert_eq!(fixture.auth.user_count().await.unwrap(), 0);

    // Not JSON at all. This is the pre-session cross-origin defense, not a
    // formality: with no CORS layer a cross-site form can only send the three
    // simple content types, and none of them gets past here.
    let resp = client()
        .post(format!("http://{}/api/v1/auth/setup", fixture.addr))
        .header("content-type", "text/plain")
        .body("name=root&password=rootpw")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 415);
    problem(resp).await;

    // JSON, but not a setup.
    let resp = setup(fixture.addr, json!({"name": "root"})).await;
    assert_eq!(resp.status(), 422);
    assert_eq!(fixture.auth.user_count().await.unwrap(), 0);

    // A name the store cannot key on, which is the one store refusal this
    // endpoint reclassifies as the caller's mistake. Asserted at the HTTP level
    // because the mapping reads the store's own wording: without this, a
    // reworded store message would turn a typo on the first screen a new user
    // ever sees into a 500.
    for name in ["ada lovelace", "   "] {
        let resp = setup(fixture.addr, json!({"name": name, "password": "rootpw"})).await;
        assert_eq!(resp.status(), 422, "{name:?} is not a login name");
        let doc = problem(resp).await;
        assert!(
            doc["detail"].as_str().unwrap().contains("name"),
            "the store's own wording, which says what is wrong with it: {}",
            doc["detail"]
        );
    }
    assert_eq!(fixture.auth.user_count().await.unwrap(), 0);

    // The anonymous viewer changes nothing: setup is public either way, and an
    // instance browsing anonymously still needs its first admin.
    let anonymous = serve(Options {
        anonymous: true,
        ..Options::default()
    })
    .await;
    let probe = me(anonymous.addr, None).await;
    assert_eq!(probe["anonymous"], true);
    assert_eq!(probe["needs_setup"], true);
    assert_eq!(
        setup(
            anonymous.addr,
            json!({"name": "root", "password": "rootpw"})
        )
        .await
        .status(),
        200
    );
}

// The two below drive the router `serve` actually builds, rather than a state
// this file assembled: they are the wiring tests. Everything above them would
// stay green if `run_http` served a router with no peer address in it and no
// token threaded through, which is exactly the outage this endpoint would have
// shipped with.

/// End to end through the production constructor: a browser on the machine that
/// serves the daemon creates the first admin with no token at all, because the
/// peer address the server saw is loopback.
#[tokio::test]
async fn the_production_router_creates_the_first_admin_over_a_real_socket() {
    let fixture = serve_production(None).await;
    assert_eq!(me(fixture.addr, None).await["needs_setup"], true);

    let resp = setup(fixture.addr, json!({"name": "Root", "password": "rootpw"})).await;
    assert_eq!(
        resp.status(),
        200,
        "connect info is served, so the loopback peer is visible to the handler"
    );
    assert!(session_cookie(&resp).is_some(), "and it signs them in");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["user"]["name"], "root");
    assert_eq!(body["user"]["role"], "admin");
    assert_eq!(me(fixture.addr, None).await["needs_setup"], false);
    assert_eq!(fixture.auth.user_count().await.unwrap(), 1);
}

/// The token a non-loopback `serve` generates reaches the handler through
/// `http_router`'s parameter: presented by a caller this server treats as
/// remote, it opens the slot, and the same request against a router built
/// without one is refused.
#[tokio::test]
async fn the_production_router_threads_the_setup_token_to_the_handler() {
    let configured = serve_production(Some(TOKEN.to_string())).await;
    let resp = setup_forwarded(
        configured.addr,
        json!({"name": "root", "password": "rootpw", "token": TOKEN}),
    )
    .await;
    assert_eq!(resp.status(), 200, "the threaded token is the way in");
    assert_eq!(configured.auth.user_count().await.unwrap(), 1);

    // The parameter is what carries it: the identical request against a router
    // built with `None` (the loopback-bind case) is refused, and the refusal
    // offers no token field, since this daemon has no token to present.
    let tokenless = serve_production(None).await;
    let resp = setup_forwarded(
        tokenless.addr,
        json!({"name": "root", "password": "rootpw", "token": TOKEN}),
    )
    .await;
    assert_eq!(resp.status(), 403);
    let doc = problem(resp).await;
    assert!(
        doc.get("token_required").is_none(),
        "a daemon with no token offers no token field: {doc}"
    );
    assert!(
        !doc.to_string().contains(TOKEN),
        "and no refusal ever echoes a token: {doc}"
    );
    assert_eq!(tokenless.auth.user_count().await.unwrap(), 0);
}
