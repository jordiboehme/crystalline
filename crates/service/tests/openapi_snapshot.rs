//! The OpenAPI document is a committed artifact, not a runtime detail.
//!
//! `fluid/` generates its client types from `crates/service/openapi/fluid-v1.json`
//! at build time rather than from a running server, so the file in the tree is
//! the contract the UI is compiled against. These tests are what keeps it equal
//! to the document the code actually produces: the snapshot check fails on any
//! drift and says how to regenerate, and the coverage check pins the path list
//! itself so a route can neither be added without being documented nor be
//! documented without being served.

use std::collections::BTreeSet;

/// Every operation the `/api/v1` router mounts, as `METHOD path`, spelled the
/// way the OpenAPI document spells it (the engram wildcard becomes an ordinary
/// `{permalink}` template, which is the only notation OpenAPI has for it).
///
/// This list is the contract rather than a convenience: it is hand-maintained
/// against `rest::router`, and `the_document_covers_every_mounted_path` compares
/// it against the document in *both* directions, so a new route that nobody
/// annotated and an annotation for a route nobody mounted each fail here. The
/// method is part of it because a documented method the router does not serve
/// would generate a client function that compiles and then answers 405.
const MOUNTED_OPERATIONS: &[&str] = &[
    "GET /api/v1/openapi.json",
    "POST /api/v1/auth/login",
    "POST /api/v1/auth/logout",
    "GET /api/v1/auth/me",
    "GET /api/v1/domains",
    "GET /api/v1/domains/{domain}/tree",
    "GET /api/v1/domains/{domain}/manifest",
    "GET /api/v1/domains/{domain}/engrams",
    "GET /api/v1/domains/{domain}/engrams/{permalink}",
    "GET /api/v1/search",
    "GET /api/v1/vocabulary",
    "GET /api/v1/context",
    "GET /api/v1/activity",
    "GET /api/v1/graph",
    "GET /api/v1/users",
    "POST /api/v1/users",
    "PATCH /api/v1/users/{name}",
    "DELETE /api/v1/users/{name}",
];

/// Where the committed snapshot lives, as an absolute path, for the writer.
const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi/fluid-v1.json");

/// How the document is serialized, in exactly one place: the snapshot test, the
/// regenerator and the served route must all agree byte for byte or the check
/// would fail on formatting rather than on content.
fn generated() -> String {
    let mut json = serde_json::to_string_pretty(&crystalline_service::rest::openapi_document())
        .expect("the OpenAPI document serializes");
    json.push('\n');
    json
}

/// The committed document equals the generated one.
///
/// Compared as text rather than as parsed JSON on purpose: the file is read by
/// a code generator and by humans in review, so its formatting is part of what
/// is being pinned.
#[test]
fn openapi_snapshot_is_current() {
    let committed = include_str!("../openapi/fluid-v1.json");
    assert_eq!(
        generated().trim(),
        committed.trim(),
        "the OpenAPI document has drifted from crates/service/openapi/fluid-v1.json. \
         Regenerate it with: cargo nextest run -p crystalline-service --run-ignored all \
         regenerate_openapi"
    );
}

/// The document describes exactly the operations the router mounts.
///
/// Both directions matter. An operation missing from the document is a route
/// the UI cannot be generated a client for; one in the document that nothing
/// mounts is worse, because a generated client would compile and then fail at
/// runtime.
#[test]
fn the_document_covers_every_mounted_path() {
    // Read the serialized form rather than the typed tree: this is the shape a
    // client generator consumes, so it is the shape worth asserting on.
    let doc = serde_json::to_value(crystalline_service::rest::openapi_document()).unwrap();
    let documented: BTreeSet<String> = doc["paths"]
        .as_object()
        .expect("the document has a paths object")
        .iter()
        .flat_map(|(path, item)| {
            item.as_object()
                .expect("a path item is an object")
                .keys()
                .map(move |method| format!("{} {path}", method.to_uppercase()))
        })
        .collect();
    let mounted: BTreeSet<String> = MOUNTED_OPERATIONS.iter().map(|s| s.to_string()).collect();

    let missing: Vec<&String> = mounted.difference(&documented).collect();
    assert!(
        missing.is_empty(),
        "these mounted operations carry no #[utoipa::path] annotation: {missing:?}"
    );
    let phantom: Vec<&String> = documented.difference(&mounted).collect();
    assert!(
        phantom.is_empty(),
        "these operations are documented but nothing mounts them: {phantom:?}"
    );
}

/// Rewrite the committed snapshot from the code. Ignored by default: it is a
/// tool rather than a check, and a test that silently fixed its own subject
/// would make `openapi_snapshot_is_current` unable to fail in CI.
///
/// Run it with:
/// `cargo nextest run -p crystalline-service --run-ignored all regenerate_openapi`
#[test]
#[ignore = "regenerates the committed OpenAPI snapshot; run it explicitly"]
fn regenerate_openapi() {
    std::fs::write(SNAPSHOT, generated()).expect("the snapshot directory exists and is writable");
}
