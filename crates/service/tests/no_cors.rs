//! The no-CORS invariant, checked against the source rather than left to prose.
//!
//! Two things on the REST surface are safe only because no other origin can
//! read an answer from it:
//!
//! - `GET /auth/me` hands the caller their CSRF token in the response body. A
//!   cross-origin page can send that request, and behind a trusted-header proxy
//!   it would even be answered for the victim's identity; what it cannot do is
//!   read what came back.
//! - The trusted-header mode's token is scoped to the identity rather than to a
//!   cookie, so every device of one SSO user shares it. That makes reading the
//!   probe's body worth more, not less.
//!
//! A permissive CORS layer would turn both into an account takeover, and it is
//! the kind of thing that gets added in a hurry to make a dev server work. The
//! doc comments on `check_csrf` and `MeResponse::csrf` say so, and this is what
//! makes ignoring them fail a build instead of passing review.
//!
//! Scoped to `src/`, and this file lives in `tests/`, so the strings it searches
//! for cannot match themselves.

use std::path::{Path, PathBuf};

/// What must not appear in the served source: the layer itself, the crate that
/// provides it, and a hand-written header that would do the same job. Each is
/// paired with what to do about it instead of a bare failure.
const FORBIDDEN: &[(&str, &str)] = &[
    (
        "CorsLayer",
        "a CORS layer would let another origin read /auth/me's answer, which \
         carries the caller's CSRF token",
    ),
    (
        "tower_http::cors",
        "the CORS module is the layer by another name",
    ),
    (
        "tower-http",
        "tower-http is not a dependency of this crate, and its cors feature is \
         the usual way a CORS layer arrives",
    ),
    (
        "Access-Control-Allow",
        "a hand-written CORS header is the same hole as the layer",
    ),
    (
        "access-control-allow",
        "a hand-written CORS header is the same hole as the layer",
    ),
];

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            found.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
    found
}

/// No CORS anywhere in the crate that builds and serves the HTTP surface.
///
/// The whole tree rather than a list of files, so a layer added to a module
/// nobody thought to name here fails too.
#[test]
fn no_cors_layer_reaches_the_http_surface() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = rust_sources(&src);
    assert!(
        sources.len() > 10,
        "the scan found only {} files under {}, so it is not looking where it \
         thinks it is",
        sources.len(),
        src.display()
    );
    for file in sources {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("reading {}: {e}", file.display()));
        for (needle, why) in FORBIDDEN {
            for (n, line) in text.lines().enumerate() {
                assert!(
                    !line.contains(needle),
                    "{}:{} mentions `{needle}`: {why}. No CORS layer exists on \
                     this surface and one must not be added without revisiting \
                     the CSRF rule in rest/auth.rs - see `check_csrf`.",
                    file.display(),
                    n + 1
                );
            }
        }
    }
}

/// The dependency itself, which is the cheapest place to catch this: a
/// `CorsLayer` cannot be written without something providing it.
#[test]
fn the_crate_does_not_depend_on_a_cors_provider() {
    let manifest =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("the crate manifest is readable");
    for needle in ["tower-http", "tower_http", "cors"] {
        assert!(
            !manifest.contains(needle),
            "crates/service/Cargo.toml mentions `{needle}`. A CORS layer would \
             let another origin read the CSRF token /auth/me hands back; see \
             `check_csrf` in rest/auth.rs before adding one."
        );
    }
}
