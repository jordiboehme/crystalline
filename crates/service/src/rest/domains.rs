//! What a client sees before it sees any engram: which domains this instance
//! serves, what each one holds, and what each one is for.
//!
//! The listing and the tree pass the engine's own JSON through unchanged, so
//! the MCP tools and this API answer with one payload rather than two shapes
//! that drift. The manifest endpoint is the one wrapper here, because a
//! markdown document is not JSON: it is handed over as a string beside the
//! domain it belongs to.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiError, ApiPath, ApiQuery, RestState};
use crate::params::{BrowseParams, ListDomainsParams};

/// `GET /domains` - every registered domain with its counts, its kind and its
/// routing bullets, plus the behavior rules that govern them.
///
/// `include_routing` is always on rather than a query parameter: a browser
/// client is exactly the caller that has no other way to learn what a domain is
/// for, and the bullets are a handful of lines per domain.
pub async fn list(State(state): State<RestState>) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .list_domains(&ListDomainsParams {
            include_routing: true,
        })
        .await?;
    Ok(Json(value))
}

/// The query string `GET /domains/{domain}/tree` takes, mirroring
/// [`BrowseParams`] minus the domain the path already names.
#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    /// A domain-relative folder path. Defaults to the root.
    #[serde(default)]
    path: Option<String>,
    /// How many folder levels deep to list. Defaults to 1.
    #[serde(default)]
    depth: Option<usize>,
    /// A glob filtering the engram paths listed.
    #[serde(default)]
    glob: Option<String>,
}

/// `GET /domains/{domain}/tree` - one domain's engrams and subfolders under a
/// path, the navigation a file tree in the UI is built from.
pub async fn tree(
    State(state): State<RestState>,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(query): ApiQuery<TreeQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .browse_domain(&BrowseParams {
            domain,
            path: query.path,
            depth: query.depth,
            glob: query.glob,
        })
        .await?;
    Ok(Json(value))
}

/// `GET /domains/{domain}/manifest` - the domain's MANIFEST markdown as
/// written, so a client can render or edit the source rather than a reduction
/// of it.
pub async fn manifest(
    State(state): State<RestState>,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let markdown = state.engine.manifest_markdown(&domain).await?;
    Ok(Json(json!({ "domain": domain, "markdown": markdown })))
}
