//! The two ways a client reaches an engram: a filtered listing of a domain,
//! and one engram in full.
//!
//! Both hand the engine's own JSON over unchanged, so this API and the MCP
//! tools answer with one payload rather than two shapes that drift. The detail
//! route adds exactly one thing on top: an `ETag`, so a client that later wants
//! to write back can say which version it read.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::ETAG;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;

use super::{ApiError, ApiPath, ApiQuery, RestState, csv};
use crate::params::{ReadParams, SearchParams};

/// The query string `GET /domains/{domain}/engrams` takes: the filter side of
/// [`SearchParams`], minus the domain the path already names and minus the
/// free-text query, which belongs to the search endpoint rather than a listing.
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Filter by `type`.
    #[serde(rename = "type", default)]
    engram_type: Option<String>,
    /// Filter by `status`.
    #[serde(default)]
    status: Option<String>,
    /// Require all of these tags, comma separated. A URL is a string, so the
    /// repeated-parameter form a `Vec` would need is not on offer here; the
    /// list is split on the way in.
    #[serde(default)]
    tags: Option<String>,
    /// Only engrams recorded on or after this ISO date.
    #[serde(default)]
    after: Option<String>,
    /// One-based page number. Defaults to 1.
    #[serde(default)]
    page: Option<usize>,
    /// Page size. Defaults to 10.
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /domains/{domain}/engrams` - one domain's engrams, filtered by
/// frontmatter and paged, which is what a browsing client lists from.
///
/// This is `search_engrams` with no query text: a filter-only search, so the
/// answer carries the engine's own page envelope (`total`, `page`, `limit`,
/// `count`, `hits`) and a client pages it the way it pages a search. Listing by
/// folder is not here: the tree endpoint owns the navigation view, this one
/// owns the frontmatter view, and neither reimplements the other.
///
/// A domain nobody registered is a 404, like the tree and manifest routes
/// beside it: a path segment names a resource, and a resource that does not
/// exist is missing, while query filters may legitimately filter to nothing. So
/// an unknown domain answers 404 and a registered domain that holds nothing
/// answers an empty page, two states a client can tell apart. The check is
/// [`crate::engine::Engine::require_domain`], the same resolution the tree
/// route's own verb opens with; `search_engrams` is left alone, because an
/// unmatched name in its `domains` filter really is just a narrower filter, and
/// that verb is shared with the MCP tool.
pub async fn list(
    State(state): State<RestState>,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    state.engine.require_domain(&domain)?;
    let value = state
        .engine
        .search_engrams(&SearchParams {
            // No text: the filters alone select, and the engine takes that as
            // the filter-only mode rather than as an empty search.
            query: None,
            domains: vec![domain],
            engram_type: query.engram_type,
            tags: csv(query.tags.as_deref()),
            status: query.status,
            after: query.after,
            page: query.page,
            limit: query.limit,
            ..SearchParams::default()
        })
        .await?;
    Ok(Json(value))
}

/// `GET /domains/{domain}/engrams/{*permalink}` - one engram in full: its
/// frontmatter, its markdown as written and the references the engine resolves
/// around it.
///
/// The permalink is a wildcard segment because a permalink is a path: an engram
/// two folders down carries `notes/deep/gamma`, and a single segment would only
/// ever reach the ones at a domain's root.
///
/// The response carries an `ETag` over the markdown, so a client knows which
/// version it is holding. See [`etag`].
pub async fn detail(
    State(state): State<RestState>,
    ApiPath((domain, permalink)): ApiPath<(String, String)>,
) -> Result<Response, ApiError> {
    let value = state
        .engine
        .read_engram(&ReadParams {
            identifier: permalink,
            domain: Some(domain),
        })
        .await?;
    let etag = etag(&value)?;
    let mut resp = Json(value).into_response();
    resp.headers_mut().insert(ETAG, etag);
    Ok(resp)
}

/// The strong validator for the engram this read returned: the checksum the
/// engine computed over the markdown it is handing back, quoted the way RFC
/// 9110 requires of a strong one.
///
/// The engine's `checksum` is reused rather than the content hashed a second
/// time here, and not to save the hash: it is the same value `edit_engram`
/// compares an `expected_checksum` against, so the tag a client reads out of a
/// response is exactly the token a later `If-Match` can be turned into, with
/// one definition of what version an engram is at rather than two that agree
/// until one of them moves.
///
/// The error branch is unreachable under the current engine, which always emits
/// `checksum` from a read; it guards against a future contract break, loudly,
/// rather than letting an engram be served without a version.
fn etag(value: &Value) -> Result<HeaderValue, ApiError> {
    let checksum = value["checksum"].as_str().ok_or_else(|| {
        ApiError::internal("the engram read carried no checksum to version it by")
    })?;
    HeaderValue::from_str(&format!("\"{checksum}\""))
        .map_err(|_| ApiError::internal("the engram's checksum is not a usable ETag"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tag is a quoted strong validator, which is what an `If-Match`
    /// comparison later depends on: unquoted or `W/`-prefixed would not match.
    #[test]
    fn the_etag_is_the_quoted_checksum() {
        let value = serde_json::json!({ "checksum": "abc123", "content": "hi" });
        assert_eq!(etag(&value).unwrap(), "\"abc123\"");
    }

    /// A response with no checksum is an engine contract this layer cannot
    /// paper over, so it says so loudly instead of dropping the header and
    /// leaving a client to think the engram has no version.
    #[test]
    fn a_read_without_a_checksum_is_an_internal_error() {
        let err = etag(&serde_json::json!({ "content": "hi" })).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }
}
