//! The Fluid web bundle, served out of the binary itself.
//!
//! The bundle is compiled in by [`FluidAssets`] (staged by `build.rs`, see the
//! notes there) and answered from here: a handful of small pure functions the
//! daemon's HTTP router calls, each generic over the asset source so the whole
//! surface is testable against a committed fixture on a machine that has never
//! run a node toolchain.
//!
//! ## Cache semantics, mirrored from the nginx variant
//!
//! The compose deployment (`fluid/nginx.conf.template`) stays supported as the
//! scale-out variant, so a browser must not be able to tell which of the two
//! served it. That means these rules, one for one:
//!
//! | Request | Answer | Why |
//! |---|---|---|
//! | `/assets/<hashed>` | 200, `public, max-age=31536000, immutable`, `ETag` | the names are content-hashed, so a name never survives a change to what is behind it |
//! | `/assets/<hashed>` with a matching `If-None-Match` | 304, no body | the validator is the embed's own sha256 |
//! | `/assets/<gone>` | 404 | nginx says `try_files $uri =404`: a chunk that no longer exists must be told so, never answered with the shell |
//! | `index.html`, however it is reached | 200, `no-store` | the one unhashed name, and it is what names the current chunks: a stale copy is exactly how a browser asks a new deployment for chunks that are gone |
//! | another root-level file | 200, `no-cache` | unhashed too, so it revalidates rather than sticks |
//!
//! Compression is deliberately absent: nginx gzips, this does not. The bundle
//! is loaded lazily and then held immutable, so the cost is first load only,
//! and the compose variant remains the documented path where wire size rules.
//!
//! ## An unbuilt bundle answers 503
//!
//! With this feature on but no bundle staged (a dev build that never ran
//! `pnpm --dir fluid build`), the embed is empty and every navigation gets a
//! short 503 page naming that command. Not 200, because then a health check
//! or an uptime monitor would call a UI-less binary healthy; not 404, because
//! the path is right and it is the build that is missing. Asset paths still
//! answer a plain 404: nothing is going to fix those one at a time.
//!
//! The UI is served unauthenticated, exactly as nginx serves it. The shell
//! carries no knowledge; every byte of data still comes through `/api/v1`,
//! which stays closed behind its own guard.

use std::fmt::Write as _;

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::{EmbeddedFile, RustEmbed};

/// The Fluid bundle staged by `build.rs`. Empty when `fluid/dist` was absent
/// at compile time.
#[derive(RustEmbed)]
#[folder = "$OUT_DIR/fluid-dist"]
#[exclude = "*.map"]
pub struct FluidAssets;

/// The bundle's entry point, and the only name in it that is not
/// content-hashed.
const INDEX: &str = "index.html";

/// What the hashed chunks under `/assets/` may be held for.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// What an unhashed name may be held for: nothing, without asking first.
const REVALIDATE: &str = "no-cache";

/// What the entry point may be held for: nothing at all.
const NO_STORE: &str = "no-store";

/// The page a binary with no bundle answers navigations with.
const NOT_BUILT: &str = "<!doctype html>\n\
    <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
    <title>Crystalline: web UI not built</title></head>\n\
    <body><h1>No web UI in this binary</h1>\n\
    <p>This build of Crystalline carries no web bundle. Build one with\n\
    <code>pnpm --dir fluid build</code> and rebuild the binary, or use a\n\
    release binary, which always ships with the UI embedded.</p>\n\
    <p>The JSON API under <code>/api/v1</code> and the MCP endpoint are\n\
    unaffected.</p></body></html>\n";

/// Whether this binary carries a usable UI (an `index.html` is embedded).
pub fn ui_available<E: RustEmbed>() -> bool {
    E::get(INDEX).is_some()
}

/// `GET /` and the SPA fallback body: the entry point, never stored, or the
/// 503 not-built page when the embed is empty.
pub fn index_response<E: RustEmbed>() -> Response {
    embedded::<E>(INDEX, NO_STORE, None).unwrap_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [
                content_type("text/html; charset=utf-8"),
                cache_control(NO_STORE),
            ],
            Body::from(NOT_BUILT),
        )
            .into_response()
    })
}

/// `GET /assets/{*path}`: immutable, validated by an `ETag`, 304 on a match,
/// and a plain 404 on a miss.
///
/// `path` is the full embed key, `assets/` prefix included.
pub fn asset_response<E: RustEmbed>(path: &str, if_none_match: Option<&str>) -> Response {
    embedded::<E>(path, IMMUTABLE, if_none_match)
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

/// The exact-match half of the fallback: the embedded file at `path` (no
/// leading slash), or `None` when nothing stands behind that name.
///
/// `index.html` deliberately answers `None` here: it is served through
/// [`index_response`] alone, so its no-store rule cannot be bypassed by asking
/// for it by name.
pub fn exact_response<E: RustEmbed>(path: &str) -> Option<Response> {
    if path == INDEX {
        return None;
    }
    embedded::<E>(path, REVALIDATE, None)
}

/// The dispatch rule, pure over `(method, accept)`: true when this request is
/// a browser navigation the app should answer.
///
/// Deliberately narrow. A GET whose `Accept` is `*/*` (curl's default, and
/// what a naive API script sends) is NOT a navigation and falls through to the
/// MCP service, and neither is anything that is not a GET or a HEAD, whatever
/// it claims to accept. Every MCP request is a POST, a DELETE or a GET asking
/// for `text/event-stream`, so this rule cannot capture one.
pub fn wants_spa(method: &Method, accept: Option<&str>) -> bool {
    if method != Method::GET && method != Method::HEAD {
        return false;
    }
    accept.is_some_and(|accept| {
        accept.split(',').any(|entry| {
            entry
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("text/html")
        })
    })
}

/// One embedded file, with a cache policy and optional revalidation, or `None`
/// when the embed has no such name.
fn embedded<E: RustEmbed>(
    path: &str,
    cache: &'static str,
    if_none_match: Option<&str>,
) -> Option<Response> {
    // Embed lookups are keyed rather than filesystem walks, so a dot segment
    // has nothing to traverse in a release binary; a debug build resolves the
    // path on disk, though, and one line here makes the rule the same in both
    // and the review of it trivial.
    if path.is_empty() || path.contains("..") {
        return None;
    }
    let file = E::get(path)?;
    let etag = etag_of(&file);

    if if_none_match.is_some_and(|candidates| matches_etag(candidates, &etag)) {
        return Some(
            (
                StatusCode::NOT_MODIFIED,
                [cache_control(cache), etag_header(&etag)],
            )
                .into_response(),
        );
    }

    let mime = file.metadata.mimetype().to_owned();
    Some(
        (
            StatusCode::OK,
            [
                content_type(&mime),
                cache_control(cache),
                etag_header(&etag),
            ],
            Body::from(file.data.into_owned()),
        )
            .into_response(),
    )
}

/// The embed's sha256 of a file, as a quoted hex string: a strong validator
/// that needs no filesystem timestamp and is identical on every machine that
/// built the same bundle.
fn etag_of(file: &EmbeddedFile) -> String {
    let mut tag = String::with_capacity(66);
    tag.push('"');
    for byte in file.metadata.sha256_hash() {
        let _ = write!(tag, "{byte:02x}");
    }
    tag.push('"');
    tag
}

/// Whether an `If-None-Match` header names this validator. Handles the list
/// form, the `*` form and the weak prefix a proxy may have added.
fn matches_etag(if_none_match: &str, etag: &str) -> bool {
    if_none_match.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.trim_start_matches("W/") == etag
    })
}

fn content_type(value: &str) -> (HeaderName, HeaderValue) {
    (
        header::CONTENT_TYPE,
        HeaderValue::from_str(value)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    )
}

fn cache_control(value: &'static str) -> (HeaderName, HeaderValue) {
    (header::CACHE_CONTROL, HeaderValue::from_static(value))
}

fn etag_header(etag: &str) -> (HeaderName, HeaderValue) {
    (
        header::ETAG,
        HeaderValue::from_str(etag).unwrap_or_else(|_| HeaderValue::from_static("\"\"")),
    )
}
