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
use utoipa::IntoParams;

use super::{ApiError, ApiPath, ApiQuery, ProblemDetail, RestState};
use crate::params::{BrowseParams, ListDomainsParams};

/// `GET /domains` - every registered domain with its counts, its kind and its
/// routing bullets, plus the behavior rules that govern them.
///
/// `include_routing` is always on rather than a query parameter: a browser
/// client is exactly the caller that has no other way to learn what a domain is
/// for, and the bullets are a handful of lines per domain.
#[utoipa::path(
    get,
    path = "/api/v1/domains",
    tag = "domains",
    operation_id = "list_domains",
    responses(
        (
            status = 200,
            description = "The engine's own domain listing, unchanged.",
            body = Object,
            example = json!({
                "behavior": [
                    "Search before answering from memory.",
                    "Record what was learned as an engram."
                ],
                "domains": [{
                    "name": "eng",
                    "kind": "file",
                    "path": "/Users/ada/Documents/Crystalline/eng",
                    "engrams": 4,
                    "observations": 12,
                    "relations": 3,
                    "last_sync": "2026-08-05T09:14:22Z",
                    "when_to_use": ["Route here for eng questions."]
                }]
            }),
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
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
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TreeQuery {
    /// A domain-relative folder path. Defaults to the root.
    #[serde(default)]
    #[param(example = "notes")]
    path: Option<String>,
    /// How many folder levels deep to list. Defaults to 1.
    #[serde(default)]
    #[param(example = 2)]
    depth: Option<usize>,
    /// A glob filtering the engram paths listed.
    #[serde(default)]
    #[param(example = "notes/**")]
    glob: Option<String>,
}

/// `GET /domains/{domain}/tree` - one domain's engrams and subfolders under a
/// path, the navigation a file tree in the UI is built from.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/tree",
    tag = "domains",
    operation_id = "get_domain_tree",
    params(("domain" = String, Path, description = "The registered domain."), TreeQuery),
    responses(
        (
            status = 200,
            description = "The engine's own browse payload, unchanged.",
            body = Object,
            example = json!({
                "domain": "eng",
                "path": "/",
                "folders": ["notes"],
                "engrams": [{
                    "permalink": "alpha",
                    "title": "Alpha",
                    "type": "engram",
                    "status": "stable",
                    "path": "alpha.md"
                }]
            }),
        ),
        (
            status = 400,
            description = "The query string will not parse.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The glob is not a valid pattern.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/manifest",
    tag = "domains",
    operation_id = "get_domain_manifest",
    params(("domain" = String, Path, description = "The registered domain.")),
    responses(
        (
            status = 200,
            description = "The manifest source beside the domain it belongs to.",
            body = Object,
            example = json!({
                "domain": "eng",
                "markdown": "---\ntitle: eng\n---\n\n## When to Use\n\n- Route here for eng questions.\n"
            }),
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or the domain carries no MANIFEST yet.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn manifest(
    State(state): State<RestState>,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let markdown = state.engine.manifest_markdown(&domain).await?;
    Ok(Json(json!({ "domain": domain, "markdown": markdown })))
}
