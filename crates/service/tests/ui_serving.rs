//! The embedded web UI's serving rules, driven against a committed fixture
//! bundle rather than a real one.
//!
//! `fluid/dist` is a build artifact and is not in the tree, so a clone without
//! a node toolchain has nothing to embed. That is exactly the state these
//! tests must survive: the serving functions are generic over the asset source
//! (`RustEmbed` is the bound), so the derives below point at four committed
//! files and one deliberately empty folder, and every rule the daemon relies
//! on is pinned on every platform with no bundle anywhere in sight.
//!
//! The first half drives those functions directly. The second half drives the
//! production router construction
//! (`crystalline_service::daemon::http_router_with_assets`, the same one
//! `run_http` mounts, only told which embed to serve) over a live TCP listener,
//! so the dispatch order between the three citizens of that router - the
//! declared routes, the embedded bundle and the MCP transport - is pinned end
//! to end rather than per function.

#![cfg(feature = "fluid-ui")]

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router_with_assets;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::ui::{
    asset_response, exact_response, index_response, ui_available, wants_spa,
};
use rust_embed::RustEmbed;
use tokio::sync::Mutex;

/// A bundle-shaped fixture: the unhashed entry point, two hashed chunks, a
/// root-level file that is not the entry point, and two sourcemaps that the
/// exclude below must keep out of the embed.
#[derive(RustEmbed)]
#[folder = "tests/fixtures/ui-dist"]
#[exclude = "*.map"]
struct Fixture;

/// A binary compiled without a bundle: the empty staging folder the build
/// script leaves behind when `fluid/dist` is absent.
#[derive(RustEmbed)]
#[folder = "tests/fixtures/ui-empty"]
#[exclude = "*.map"]
struct EmptyAssets;

/// The value of one response header, as a string.
fn header_of(response: &Response, name: header::HeaderName) -> String {
    response
        .headers()
        .get(&name)
        .unwrap_or_else(|| panic!("the response carries a {name} header"))
        .to_str()
        .expect("the header value is ASCII")
        .to_owned()
}

/// The whole body of a response, as a string.
async fn body_of(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is fully readable");
    String::from_utf8(bytes.to_vec()).expect("the fixture bodies are UTF-8")
}

/// How many bytes a response's body carries.
async fn body_len(response: Response) -> usize {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is fully readable")
        .len()
}

/// A request carrying nothing but this Accept header.
fn accept(value: &str) -> Option<&str> {
    Some(value)
}

#[test]
fn a_bundle_is_available_and_an_empty_embed_is_not() {
    assert!(
        ui_available::<Fixture>(),
        "the fixture bundle carries an index.html"
    );
    assert!(
        !ui_available::<EmptyAssets>(),
        "an embed with no index.html is not a usable UI"
    );
}

#[tokio::test]
async fn the_index_is_served_and_never_stored() {
    let response = index_response::<Fixture>();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        header_of(&response, header::CONTENT_TYPE).contains("text/html"),
        "the entry point is HTML"
    );
    assert_eq!(
        header_of(&response, header::CACHE_CONTROL),
        "no-store",
        "index.html is the one unhashed name: a cached copy is how a browser \
         asks a new deployment for chunks that no longer exist"
    );
    assert!(
        !header_of(&response, header::ETAG).is_empty(),
        "the validator rides along even though no-store means nothing can \
         revalidate against it today: it is what nginx sends here too, and a \
         later decision to allow revalidation finds it already in place"
    );
    assert!(
        body_of(response).await.contains("fixture-index-marker"),
        "the body is the embedded document, not a placeholder"
    );
}

#[tokio::test]
async fn hashed_assets_are_immutable_and_carry_a_validator() {
    let response = asset_response::<Fixture>("assets/app-fixture01.js", None);
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        header_of(&response, header::CONTENT_TYPE).contains("javascript"),
        "the content type comes from the embed metadata"
    );
    assert_eq!(
        header_of(&response, header::CACHE_CONTROL),
        "public, max-age=31536000, immutable",
        "the same policy the nginx variant gives /assets/"
    );
    let etag = header_of(&response, header::ETAG);
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && etag.len() == 66,
        "the ETag is the quoted sha256 of the file, got {etag}"
    );
    assert!(
        body_of(response).await.contains("fixture-script-marker"),
        "the body is the embedded chunk"
    );
}

#[tokio::test]
async fn a_known_validator_answers_304_with_no_body() {
    let first = asset_response::<Fixture>("assets/app-fixture01.css", None);
    let etag = header_of(&first, header::ETAG);

    let second = asset_response::<Fixture>("assets/app-fixture01.css", Some(&etag));
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        header_of(&second, header::ETAG),
        etag,
        "a 304 repeats the validator it matched"
    );
    assert_eq!(body_len(second).await, 0, "a 304 carries no body");
}

#[tokio::test]
async fn an_unknown_validator_answers_the_file_again() {
    let response = asset_response::<Fixture>("assets/app-fixture01.css", Some("\"not-this-one\""));
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a validator that does not match this file must send the file. A 304 \
         here is the worst failure this module can produce: the chunk is held \
         for a year under `immutable`, so the browser never asks again and \
         the stale copy outlives every deployment"
    );
    assert!(
        body_len(response).await > 0,
        "and it sends the bytes, not an empty 200"
    );
}

#[test]
fn the_validator_is_matched_in_every_form_a_client_sends_it() {
    let etag = header_of(
        &asset_response::<Fixture>("assets/app-fixture01.css", None),
        header::ETAG,
    );

    for (candidate, why) in [
        (
            format!("W/{etag}"),
            "the weak form: comparison for If-None-Match is weak by \
             definition, and a proxy may have added the prefix",
        ),
        (
            format!("\"an-older-build\", {etag}"),
            "the list form: a client may offer every copy it holds",
        ),
        ("*".to_owned(), "the wildcard: any representation at all"),
    ] {
        let response = asset_response::<Fixture>("assets/app-fixture01.css", Some(&candidate));
        assert_eq!(
            response.status(),
            StatusCode::NOT_MODIFIED,
            "`{candidate}` names this file's validator, so it is a 304: {why}"
        );
    }

    for candidate in ["", "   ", "\"unterminated", "\"another-file-entirely\""] {
        let response = asset_response::<Fixture>("assets/app-fixture01.css", Some(candidate));
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "`{candidate}` does not name this file's validator, so it is a 200"
        );
    }
}

#[test]
fn an_unknown_asset_is_a_plain_404() {
    let response = asset_response::<Fixture>("assets/nope.js", None);
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a missing chunk is a miss, never the app shell: a browser asking for \
         a chunk that no longer exists must be told so"
    );
}

#[test]
fn a_traversing_asset_path_is_a_404() {
    let response = asset_response::<Fixture>("assets/../index.html", None);
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a path with a dot segment is refused outright rather than resolved"
    );
}

#[tokio::test]
async fn root_level_files_answer_the_exact_match_but_the_index_never_does() {
    let response =
        exact_response::<Fixture>("fixture.svg").expect("a root-level embedded file is served");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, header::CONTENT_TYPE), "image/svg+xml");
    assert_eq!(
        header_of(&response, header::CACHE_CONTROL),
        "no-cache",
        "a root-level name is not content-hashed, so it revalidates rather \
         than sticking in a cache for as long as the browser feels like"
    );
    assert!(
        !header_of(&response, header::ETAG).is_empty(),
        "and it carries the validator that revalidation needs"
    );

    assert!(
        exact_response::<Fixture>("index.html").is_none(),
        "the entry point answers through index_response only, so its no-store \
         rule cannot be bypassed by asking for it by name"
    );
    assert!(
        exact_response::<Fixture>("missing.png").is_none(),
        "a name nothing stands behind falls through to the next rule"
    );
}

#[test]
fn the_exact_match_never_reaches_below_the_root() {
    assert!(
        exact_response::<Fixture>("assets/app-fixture01.js").is_none(),
        "a hashed chunk belongs to asset_response's immutable policy. Serving \
         one here would answer it correctly and cache it wrongly, and it \
         would make the router's rung order load bearing for cache \
         correctness rather than only for the shape of a 404"
    );
    assert!(
        exact_response::<Fixture>("/fixture.svg").is_none(),
        "these functions take embed keys, never URI paths: a leading slash is \
         refused rather than trimmed, so a caller that forgets gets nothing \
         instead of something surprising"
    );
    assert!(
        exact_response::<Fixture>("../Cargo.toml").is_none(),
        "and a dot segment is refused outright"
    );
}

#[test]
fn an_absolute_path_never_leaves_the_embed() {
    assert_eq!(
        asset_response::<Fixture>("/etc/hosts", None).status(),
        StatusCode::NOT_FOUND,
        "an absolute path is refused before the lookup. In a release binary \
         the embed is keyed and this is a plain miss, but a debug build \
         resolves keys on disk, and joining an absolute path throws the \
         staging folder away"
    );
    assert_eq!(
        asset_response::<Fixture>("/assets/app-fixture01.js", None).status(),
        StatusCode::NOT_FOUND,
        "the leading slash is refused even when the rest of the key is real"
    );
}

#[test]
fn sourcemaps_never_enter_the_embed() {
    assert!(
        Fixture::get("root-fixture.map").is_none(),
        "the exclude keeps a root-level sourcemap out"
    );
    assert!(
        Fixture::get("assets/app-fixture01.js.map").is_none(),
        "and a nested one too, which is the half that is not obvious: the \
         pattern is `*.map` and it only covers this because the glob treats a \
         slash as an ordinary character. A build that starts emitting \
         sourcemaps would otherwise roughly double the binary in silence"
    );
    let embedded: Vec<String> = Fixture::iter().map(|name| name.to_string()).collect();
    assert!(
        !embedded.iter().any(|name| name.ends_with(".map")),
        "nothing named like a sourcemap is in the embed at all, got {embedded:?}"
    );
    assert!(
        embedded
            .iter()
            .any(|name| name == "assets/app-fixture01.js"),
        "and the exclude did not take the chunk with it, got {embedded:?}"
    );
}

#[tokio::test]
async fn an_embed_with_no_bundle_says_so_out_loud() {
    let response = index_response::<EmptyAssets>();
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "not 200, or a health check would call a UI-less binary healthy, and \
         not 404, because the path is right and the build is what is missing"
    );
    assert!(header_of(&response, header::CONTENT_TYPE).contains("text/html"));
    assert_eq!(
        header_of(&response, header::CACHE_CONTROL),
        "no-store",
        "a cached not-built page would outlive the build that fixes it"
    );
    assert!(
        body_of(response).await.contains("pnpm --dir fluid build"),
        "the page names the command that fixes it"
    );
}

#[test]
fn only_a_browser_navigation_reaches_the_app_shell() {
    assert!(
        wants_spa(
            &Method::GET,
            accept("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        ),
        "the header every browser address bar sends"
    );
    assert!(
        wants_spa(&Method::HEAD, accept("text/html")),
        "a HEAD for a route is the same navigation, answered without a body"
    );
    assert!(
        !wants_spa(&Method::GET, accept("text/event-stream")),
        "the MCP streamable-HTTP listen request keeps going to MCP"
    );
    assert!(
        !wants_spa(&Method::GET, accept("*/*")),
        "curl's default Accept is not a navigation: treating it as one would \
         capture naive API scripts"
    );
    assert!(
        !wants_spa(&Method::GET, None),
        "no Accept at all is not a navigation either"
    );
    assert!(
        !wants_spa(&Method::POST, accept("text/html")),
        "an MCP call is a POST, whatever it says it accepts"
    );
    assert!(
        !wants_spa(&Method::GET, accept("text/htmlx")),
        "the media type is matched whole, not by prefix"
    );
    assert!(
        wants_spa(&Method::GET, accept("application/json, TEXT/HTML;q=0.9")),
        "a list, a parameter and an unusual case are all still a navigation"
    );
}

// ---------------------------------------------------------------------------
// The router half: the production construction over a live listener.
// ---------------------------------------------------------------------------

/// The `Accept` a browser address bar sends. Every navigation row below uses
/// it, because that header is the whole reason the SPA rung can be narrow.
const BROWSER_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// What the MCP streamable transport answers a request that does not speak its
/// language: the marker that says a response came from rmcp rather than from
/// anything this program added.
const MCP_NOT_ACCEPTABLE: &str = "Client must accept text/event-stream";

/// What a served router varies: the two new config keys plus the two modes the
/// plan promises keep serving the UI.
#[derive(Default)]
struct Options {
    /// `service.read_only`: mutations refused, reads served.
    read_only: bool,
    /// `auth.anonymous`: an identityless request is answered at viewer level.
    anonymous: bool,
    /// `service.ui`, absent meaning on.
    ui: Option<bool>,
    /// `service.api`, absent meaning on.
    api: Option<bool>,
}

/// A served router plus the pieces a test reaches behind it. The temp
/// directory owns the domain and the auth database, so it must outlive every
/// request.
struct Server {
    addr: std::net::SocketAddr,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

/// Serve the production router over a fixture engine, embedding `E`.
///
/// The engine is the one every service integration test builds: a real
/// temp-directory domain synced into an in-memory store, response format
/// pinned to plain JSON.
async fn serve<E: RustEmbed + 'static>(opts: Options) -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        auth: Some(AuthConfig {
            trusted_header: None,
            anonymous: Some(opts.anonymous),
            max_users: None,
        }),
        ..GlobalConfig::default()
    };
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
    )
    .unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        read_only: Some(opts.read_only),
        ui: opts.ui,
        api: opts.api,
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    // `service.read_only` is resolved by the daemon rather than by the engine's
    // constructor, so the fixture applies it the way `serve` does.
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_read_only(opts.read_only),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    let router =
        http_router_with_assets::<E>(engine, Arc::new(AtomicUsize::new(0)), &[], auth.clone())
            .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        addr,
        auth,
        _tmp: tmp,
    }
}

/// The default fixture: the bundle embedded, every key at its default.
async fn serve_fixture() -> Server {
    serve::<Fixture>(Options::default()).await
}

/// A client with proxy discovery disabled: the target is loopback, where a
/// system proxy must never be consulted anyway, and reqwest's platform proxy
/// lookup can block for a minute on a machine with a managed network
/// configuration.
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// GET a path with an explicit `Accept`, the header the whole dispatch turns
/// on.
async fn get_accepting(addr: std::net::SocketAddr, path: &str, accept: &str) -> reqwest::Response {
    client()
        .get(format!("http://{addr}{path}"))
        .header(header::ACCEPT, accept)
        .send()
        .await
        .unwrap()
}

/// One response header as a string, or the empty string when the response
/// carries none.
fn head(response: &reqwest::Response, name: header::HeaderName) -> String {
    response
        .headers()
        .get(&name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Log in as `name`, returning the session cookie value and the CSRF token.
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let response = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({"name": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "login must succeed");
    let cookie = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| value.split(';').next())
        .and_then(|value| value.strip_prefix("fluid_session="))
        .expect("login sets the session cookie")
        .to_owned();
    let body: serde_json::Value = response.json().await.unwrap();
    let csrf = body["csrf"]
        .as_str()
        .expect("login returns a csrf token")
        .to_owned();
    (cookie, csrf)
}

#[tokio::test]
async fn the_health_probe_answers_ahead_of_every_ui_rule() {
    let server = serve_fixture().await;

    let response = get_accepting(server.addr, "/health", BROWSER_ACCEPT).await;
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["status"], "ok",
        "the probe is a declared route, so it answers its own JSON even to a \
         browser Accept: a monitor must never be handed the app shell and told \
         the daemon is fine"
    );
}

#[tokio::test]
async fn the_api_nest_is_unreachable_from_the_ui_dispatch() {
    let server = serve_fixture().await;

    let response = get_accepting(server.addr, "/api/v1/domains", BROWSER_ACCEPT).await;
    assert_eq!(
        response.status(),
        401,
        "the API is nested ahead of the fallback and answers for itself, \
         closed by default: a data route the browser Accept did not turn into \
         the app shell"
    );

    let probe = get_accepting(server.addr, "/api/v1/auth/me", BROWSER_ACCEPT).await;
    assert_eq!(
        probe.status(),
        200,
        "and the identity probe is answered without one, which is how the UI \
         learns it has to log in"
    );

    let unknown = get_accepting(server.addr, "/api/v1/nonsense", BROWSER_ACCEPT).await;
    assert_eq!(
        unknown.status(),
        401,
        "the nest carries its own guard and its own fallback, so an unknown API \
         path is answered by the API - here by the closed-by-default rule the \
         guard applies ahead of routing - and never by the app shell a client \
         would then try to parse as JSON"
    );
    assert!(
        head(&unknown, header::CONTENT_TYPE).contains("json"),
        "and it is a JSON problem document, got {}",
        head(&unknown, header::CONTENT_TYPE)
    );
    assert!(
        !unknown
            .text()
            .await
            .unwrap()
            .contains("fixture-index-marker"),
        "nothing under the nest is served out of the embed"
    );
}

#[tokio::test]
async fn turning_the_api_off_unmounts_the_nest_and_the_ui_with_it() {
    let server = serve::<Fixture>(Options {
        api: Some(false),
        ..Options::default()
    })
    .await;

    let api = get_accepting(server.addr, "/api/v1/auth/me", BROWSER_ACCEPT).await;
    assert_eq!(
        api.status(),
        406,
        "with service.api=false nothing is nested at /api/v1, so the path \
         falls through to the MCP transport like any other"
    );
    assert!(api.text().await.unwrap().contains(MCP_NOT_ACCEPTABLE));

    let root = get_accepting(server.addr, "/", BROWSER_ACCEPT).await;
    assert_eq!(
        root.status(),
        406,
        "and the UI goes with the API: a shell whose data routes are gone \
         could only render a login error, so service.api=false turns both off"
    );

    let health = get_accepting(server.addr, "/health", BROWSER_ACCEPT).await;
    assert_eq!(
        health.status(),
        200,
        "the probe is not part of the API surface and stays served"
    );
}

#[tokio::test]
async fn the_root_serves_the_app_shell() {
    let server = serve_fixture().await;

    let response = get_accepting(server.addr, "/", BROWSER_ACCEPT).await;
    assert_eq!(response.status(), 200);
    assert!(head(&response, header::CONTENT_TYPE).contains("text/html"));
    assert_eq!(
        head(&response, header::CACHE_CONTROL),
        "no-store",
        "the entry point is the one unhashed name and names the current chunks"
    );
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("fixture-index-marker")
    );
}

#[tokio::test]
async fn a_hashed_asset_is_immutable_and_revalidates() {
    let server = serve_fixture().await;

    let response = get_accepting(server.addr, "/assets/app-fixture01.js", BROWSER_ACCEPT).await;
    assert_eq!(response.status(), 200);
    assert!(head(&response, header::CONTENT_TYPE).contains("javascript"));
    assert_eq!(
        head(&response, header::CACHE_CONTROL),
        "public, max-age=31536000, immutable",
        "the same policy the nginx variant gives /assets/"
    );
    let etag = head(&response, header::ETAG);
    assert!(!etag.is_empty(), "and the validator that a 304 needs");
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("fixture-script-marker"),
        "the route serves the chunk itself"
    );

    let revalidated = client()
        .get(format!("http://{}/assets/app-fixture01.js", server.addr))
        .header(header::IF_NONE_MATCH, &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        revalidated.status(),
        304,
        "the route forwards If-None-Match, so a browser that already holds \
         this chunk is told so instead of being sent it again"
    );
    assert!(revalidated.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_asset_is_a_404_even_for_a_browser() {
    let server = serve_fixture().await;

    let response = get_accepting(server.addr, "/assets/missing.js", BROWSER_ACCEPT).await;
    assert_eq!(
        response.status(),
        404,
        "/assets/ is a declared route, so a miss under it never reaches the \
         SPA rung: a browser asking for a chunk that no longer exists must be \
         told so rather than handed HTML it will try to run as JavaScript"
    );
    assert!(
        !response
            .text()
            .await
            .unwrap()
            .contains("fixture-index-marker")
    );
}

#[tokio::test]
async fn a_root_level_file_answers_from_the_exact_match_rung() {
    let server = serve_fixture().await;

    // `*/*` rather than a browser Accept, so this can only be the exact-match
    // rung: the SPA rung would not fire for it.
    let response = get_accepting(server.addr, "/fixture.svg", "*/*").await;
    assert_eq!(response.status(), 200);
    assert_eq!(head(&response, header::CONTENT_TYPE), "image/svg+xml");
    assert_eq!(
        head(&response, header::CACHE_CONTROL),
        "no-cache",
        "a root-level name is not content-hashed, so it revalidates"
    );
}

#[tokio::test]
async fn every_browser_navigation_gets_the_app_shell() {
    let server = serve_fixture().await;

    for path in [
        "/login",
        "/d/dom/e/a%2Fb",
        "/nonsense",
        "/settings/github",
        "/graph",
    ] {
        let response = get_accepting(server.addr, path, BROWSER_ACCEPT).await;
        assert_eq!(
            response.status(),
            200,
            "{path} is a route of the app, and the app's own router owns the \
             404 experience for the ones that are not"
        );
        assert_eq!(head(&response, header::CACHE_CONTROL), "no-store");
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("fixture-index-marker"),
            "{path} is answered with the entry point"
        );
    }
}

#[tokio::test]
async fn a_head_navigation_carries_the_headers_and_no_body() {
    let server = serve_fixture().await;

    let response = client()
        .head(format!("http://{}/login", server.addr))
        .header(header::ACCEPT, BROWSER_ACCEPT)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "a HEAD for an app route is the same navigation, so it is answered by \
         the same rung"
    );
    assert!(head(&response, header::CONTENT_TYPE).contains("text/html"));
    assert_eq!(head(&response, header::CACHE_CONTROL), "no-store");
    assert!(
        response.bytes().await.unwrap().is_empty(),
        "and the body is dropped, which is the half of a HEAD nothing else \
         pins: the shell's bytes must not ride along on a request that asked \
         for headers only"
    );
}

#[tokio::test]
async fn mcp_shaped_requests_reach_the_transport_untouched() {
    let server = serve_fixture().await;

    let initialize = client()
        .post(format!("http://{}/", server.addr))
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .body(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"ui-serving-test","version":"0.0.0"}}}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        initialize.status(),
        200,
        "the root is the path an MCP client points at by default and its calls \
         are POSTs, so the UI's own GET route there must hand every other \
         method straight to the transport rather than answer 405"
    );
    assert!(
        !head(
            &initialize,
            header::HeaderName::from_static("mcp-session-id")
        )
        .is_empty(),
        "rmcp opened a session, which nothing but rmcp does"
    );
    assert!(
        head(&initialize, header::ALLOW).is_empty(),
        "and the answer is the transport's own, undecorated. An `Allow` header \
         here would mean the root was mounted as a method router with the \
         transport as its method fallback, which axum labels with the methods \
         the route does serve - telling every MCP client at the default \
         endpoint that its POST is not among them"
    );
    // The response body is an open SSE stream; the headers are the proof and
    // reading further would wait for a session this test never uses again.
    drop(initialize);

    let posted = client()
        .post(format!("http://{}/anything", server.addr))
        .header(header::ACCEPT, BROWSER_ACCEPT)
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(
        posted.status(),
        406,
        "a POST is never a navigation, whatever it claims to accept, so it \
         lands on MCP and is refused there for not accepting event-stream"
    );

    let deleted = client()
        .delete(format!("http://{}/x", server.addr))
        .header(header::ACCEPT, BROWSER_ACCEPT)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 400, "a DELETE reaches MCP too");
    assert!(
        deleted
            .text()
            .await
            .unwrap()
            .contains("Session ID is required"),
        "and is answered in rmcp's own words"
    );

    let listening = get_accepting(server.addr, "/x", "text/event-stream").await;
    assert_eq!(
        listening.status(),
        400,
        "the streamable-HTTP listen request is a GET, and it keeps reaching \
         the transport: rmcp asks it for a session id"
    );

    for path in ["/fixture.svg", "/assets/app-fixture01.js"] {
        let response = client()
            .post(format!("http://{}{path}", server.addr))
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            405,
            "a POST at {path} reaches the transport rather than being told the \
             method is not allowed: an MCP client may be pointed at any path on \
             this endpoint, and a bundle name must not start answering one"
        );
        assert!(
            head(&response, header::CACHE_CONTROL).is_empty(),
            "and it is answered by the transport, not out of the embed"
        );
    }
}

#[tokio::test]
async fn a_default_accept_is_not_a_navigation() {
    let server = serve_fixture().await;

    let curled = get_accepting(server.addr, "/d/foo", "*/*").await;
    assert_eq!(
        curled.status(),
        406,
        "curl's default Accept reaches MCP rather than the shell. A deliberate \
         deviation from the nginx variant's try_files: treating */* as a \
         navigation would capture every naive API script"
    );
    assert!(
        curled.text().await.unwrap().contains(MCP_NOT_ACCEPTABLE),
        "and the answer names what the transport does want"
    );

    let bare = client()
        .get(format!("http://{}/d/foo", server.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bare.status(),
        406,
        "a request that asks for nothing in particular is not a navigation \
         either"
    );
}

#[tokio::test]
async fn with_the_ui_off_the_mcp_service_answers_every_ui_path() {
    let server = serve::<Fixture>(Options {
        ui: Some(false),
        ..Options::default()
    })
    .await;

    for path in ["/", "/assets/app-fixture01.js", "/fixture.svg", "/login"] {
        let response = get_accepting(server.addr, path, BROWSER_ACCEPT).await;
        assert_eq!(
            response.status(),
            406,
            "with service.ui=false {path} is the MCP transport's again, byte \
             for byte the router this program started from"
        );
        assert!(
            head(&response, header::CACHE_CONTROL).is_empty(),
            "and it carries none of the UI's cache policy"
        );
        assert!(
            !response.text().await.unwrap().contains("fixture"),
            "{path} serves nothing out of the embed"
        );
    }

    assert_eq!(
        get_accepting(server.addr, "/health", BROWSER_ACCEPT)
            .await
            .status(),
        200,
        "the probe is untouched by the UI key"
    );
    assert_eq!(
        get_accepting(server.addr, "/api/v1/domains", BROWSER_ACCEPT)
            .await
            .status(),
        401,
        "and so is the API: service.ui=false turns off the UI alone, which is \
         the nginx scale-out deployment, where nginx serves the shell and this \
         daemon answers the data"
    );
}

#[tokio::test]
async fn a_read_only_daemon_serves_the_ui_and_still_refuses_writes() {
    let server = serve::<Fixture>(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    server
        .auth
        .add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();
    let (cookie, csrf) = login(server.addr, "root", "rootpw").await;

    let shell = get_accepting(server.addr, "/", BROWSER_ACCEPT).await;
    assert_eq!(
        shell.status(),
        200,
        "the UI is GETs, so a read-only mirror serves the whole of it"
    );
    assert!(shell.text().await.unwrap().contains("fixture-index-marker"));

    let write = client()
        .put(format!(
            "http://{}/api/v1/domains/eng/manifest",
            server.addr
        ))
        .header("cookie", format!("fluid_session={cookie}"))
        .header("x-csrf-token", &csrf)
        .json(&serde_json::json!({"markdown": "# eng\n"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        write.status(),
        403,
        "and the API's own read-only rule is untouched by serving the shell in \
         front of it, for the strongest caller there is"
    );
}

#[tokio::test]
async fn anonymous_browsing_serves_the_shell_and_the_api_logged_out() {
    let anonymous = serve::<Fixture>(Options {
        anonymous: true,
        ..Options::default()
    })
    .await;
    assert_eq!(
        get_accepting(anonymous.addr, "/", BROWSER_ACCEPT)
            .await
            .status(),
        200
    );
    let listed = get_accepting(anonymous.addr, "/api/v1/domains", BROWSER_ACCEPT).await;
    assert_eq!(
        listed.status(),
        200,
        "auth.anonymous plus the embedded UI is the published-archive story: a \
         browser that never logs in gets both the shell and its data"
    );
    let probe: serde_json::Value = get_accepting(anonymous.addr, "/api/v1/auth/me", BROWSER_ACCEPT)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        probe["anonymous"], true,
        "and the probe tells the shell which mode it is browsing in"
    );

    let closed = serve_fixture().await;
    assert_eq!(
        get_accepting(closed.addr, "/api/v1/domains", BROWSER_ACCEPT)
            .await
            .status(),
        401,
        "with anonymous off the data stays closed. The identity probe stays \
         public either way (it is how a client learns it must log in), so the \
         data route is what says this"
    );
    assert_eq!(
        get_accepting(closed.addr, "/", BROWSER_ACCEPT)
            .await
            .status(),
        200,
        "while the shell is served to anyone who connects, exactly as nginx \
         serves it: it carries no knowledge, and every byte of that still \
         comes through the guarded API"
    );
}

#[tokio::test]
async fn a_binary_with_no_bundle_says_so_on_every_navigation() {
    let server = serve::<EmptyAssets>(Options::default()).await;

    let root = get_accepting(server.addr, "/", BROWSER_ACCEPT).await;
    assert_eq!(
        root.status(),
        503,
        "not 200, or an uptime monitor would call a UI-less binary healthy, \
         and not 404, because the path is right and the build is what is \
         missing"
    );
    assert!(
        root.text()
            .await
            .unwrap()
            .contains("pnpm --dir fluid build")
    );

    let navigation = get_accepting(server.addr, "/login", BROWSER_ACCEPT).await;
    assert_eq!(
        navigation.status(),
        503,
        "a navigation the SPA rung would answer says the same thing: the build \
         is missing everywhere, not only at the root"
    );

    assert_eq!(
        get_accepting(server.addr, "/assets/app-fixture01.js", BROWSER_ACCEPT)
            .await
            .status(),
        404,
        "asset paths stay a plain 404: nothing is going to fix those one at a \
         time"
    );
    assert_eq!(
        get_accepting(server.addr, "/health", BROWSER_ACCEPT)
            .await
            .status(),
        200,
        "and the rest of the surface is unaffected by an empty embed"
    );
}
