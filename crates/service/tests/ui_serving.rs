//! The embedded web UI's serving rules, driven against a committed fixture
//! bundle rather than a real one.
//!
//! `fluid/dist` is a build artifact and is not in the tree, so a clone without
//! a node toolchain has nothing to embed. That is exactly the state these
//! tests must survive: the serving functions are generic over the asset source
//! (`RustEmbed` is the bound), so the derives below point at four committed
//! files and one deliberately empty folder, and every rule the daemon relies
//! on is pinned on every platform with no bundle anywhere in sight.

#![cfg(feature = "fluid-ui")]

use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use crystalline_service::ui::{
    asset_response, exact_response, index_response, ui_available, wants_spa,
};
use rust_embed::RustEmbed;

/// A bundle-shaped fixture: the unhashed entry point, two hashed chunks and a
/// root-level file that is not the entry point.
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
}
