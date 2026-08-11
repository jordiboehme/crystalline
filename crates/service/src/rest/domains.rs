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
use axum::http::header::ETAG;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::IntoParams;

use super::auth::Identity;
use super::{
    ApiError, ApiJson, ApiPath, ApiQuery, ConflictDetail, ProblemDetail, RestState, if_match,
    precondition_failed,
};
use crate::engine::EngineError;
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
///
/// One level at a time and bounded: see [`crate::engine::TREE_LEVEL_CAP`] for
/// what a level that does not fit answers with.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/tree",
    tag = "domains",
    operation_id = "get_domain_tree",
    summary = "One level of a domain's folders and engrams.",
    description = "The navigation a file tree is built from, one level at a \
                   time and bounded: a level holding more engrams than the \
                   tree shows is cut, `total` says how many it really holds \
                   and `truncated` says the rows were cut, so a client can \
                   send its reader to the paged listing instead.\n\n`folders` \
                   is never cut, so a truncated level still names every folder \
                   a reader can descend into. A `glob` narrows the engrams \
                   this level returned, so on a truncated level it selects \
                   within the cut rather than across the whole folder.",
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
                }],
                "truncated": false,
                "total": 1
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
///
/// The response carries an `ETag` over the markdown, the same strong
/// validator [`save_manifest`] compares an `If-Match` against, so a client
/// that means to edit the manifest can go straight from this read to that
/// write without a second round trip.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/manifest",
    tag = "domains",
    operation_id = "get_domain_manifest",
    summary = "The domain's MANIFEST markdown as written.",
    description = "The source, not a reduction of it, so a client can render \
                   or edit it directly.\n\nThe response carries an `ETag` \
                   over the markdown, the same strong validator a later \
                   `PUT` compares an `If-Match` against.",
    params(("domain" = String, Path, description = "The registered domain.")),
    responses(
        (
            status = 200,
            description = "The manifest source beside the domain it belongs to.",
            body = Object,
            headers(("etag" = String, description = "The quoted checksum of \
                     the manifest as read, the token a later `PUT` carries \
                     in `If-Match`.")),
            example = json!({
                "domain": "eng",
                "markdown": "---\ntitle: eng\n---\n\n## When to Use\n\n- Route here for eng questions.\n",
                "checksum": "3f8a1c05e2"
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
) -> Result<Response, ApiError> {
    let markdown = state.engine.manifest_markdown(&domain).await?;
    manifest_response(&domain, markdown, StatusCode::OK)
}

/// What `PUT /domains/{domain}/manifest` takes: the complete MANIFEST source.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "The full MANIFEST markdown as the editor holds it, \
                        written verbatim: nothing here rebuilds the \
                        frontmatter or stamps provenance.")]
pub struct SaveManifestBody {
    /// The full MANIFEST markdown as the editor holds it.
    #[schema(
        example = "---\ntitle: eng\n---\n\n## When to Use\n\n- Route here for eng questions.\n"
    )]
    markdown: String,
}

/// `PUT /domains/{domain}/manifest` - save a domain's MANIFEST markdown
/// verbatim, guarded by the `If-Match` token of the version being replaced.
///
/// Admin, not editor: the spec's domain-management section (slice 3, section
/// 5) places MANIFEST editing among the admin-only domain screens, alongside
/// creating and unregistering a domain - a MANIFEST is what routes an agent
/// into a domain rather than a document inside it, and the Fluid UI gates its
/// own Edit affordance on `canAdminister` to match.
///
/// The same three answers `engrams::save` is held to, because the guard is
/// the same contract: 428 with no `If-Match`, 412 with a stale one (carrying
/// the version the server holds now, so a client can merge), 200 with the new
/// version and its `ETag` once it lands.
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/manifest",
    tag = "domains",
    operation_id = "save_domain_manifest",
    summary = "Save a domain's MANIFEST markdown, guarded by If-Match.",
    description = "The text lands verbatim, frontmatter included, guarded the \
                   same way an engram save is: 428 with no `If-Match`, 412 \
                   when the token is stale (carrying the version the server \
                   holds now), 200 once it lands. A read-only instance \
                   answers 403 ahead of the precondition check, so it is \
                   never 428.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "If-Match" = String,
            Header,
            description = "The quoted `ETag` of the version being replaced, \
                           from the manifest read.",
            example = "\"3f8a1c05e2\"",
        ),
    ),
    request_body = SaveManifestBody,
    responses(
        (
            status = 200,
            description = "The manifest as saved, mirroring the GET shape.",
            body = Object,
            headers(("etag" = String, description = "The quoted checksum of \
                     the manifest as saved, the token the next save \
                     carries.")),
            example = json!({
                "domain": "eng",
                "markdown": "---\ntitle: eng\n---\n\n## When to Use\n\n- Route here for eng questions.\n",
                "checksum": "3f8a1c05e2"
            }),
        ),
        (
            status = 400,
            description = "`If-Match` carries more than one entity tag: this \
                           surface expects exactly one strong checksum, not a \
                           comma-separated list.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one: an identity with \
                           no account behind it never writes.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account. A read-only instance answers this ahead of \
                           the precondition check, so it is never 428.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or the domain carries no MANIFEST \
                           yet.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 412,
            description = "The `If-Match` token is stale. The body carries \
                           the version the server holds now, so a client can \
                           merge.",
            body = ConflictDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 413,
            description = "The document is over the 10 MiB limit this API \
                           accepts.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The document carries no frontmatter block, so it \
                           is not a MANIFEST.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 428,
            description = "No `If-Match` arrived. The token comes from the \
                           manifest read.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn save_manifest(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<SaveManifestBody>,
) -> Result<Response, ApiError> {
    // Admin, not editor: see this function's doc comment for why domain
    // management, MANIFEST editing included, is held to the stronger role.
    identity.require_admin()?;
    // Before the If-Match parse, not after: the same reasoning as
    // `engrams::save`'s own read-only check, repeated here rather than
    // shared, since the handlers are not yet worth abstracting over.
    if state.engine.read_only() {
        return Err(ApiError::forbidden(
            "this instance is read-only; content mutations are disabled",
        ));
    }
    let token = if_match(&headers)?;
    match state
        .engine
        .save_manifest(&domain, &body.markdown, &token)
        .await
    {
        Ok(_) => manifest_response(&domain, body.markdown, StatusCode::OK),
        // The same stale-edit translation `engrams::save` makes, repeated
        // rather than shared for the same reason.
        Err(EngineError::Conflict(message)) if message.starts_with(STALE_EDIT) => {
            let current = state.engine.manifest_markdown(&domain).await?;
            let checksum = manifest_checksum(&current);
            Ok(precondition_failed(message, &checksum, current))
        }
        Err(e) => Err(e.into()),
    }
}

/// The prefix every refused compare-and-swap opens with, wherever the
/// comparison happened. See `engine::stale_edit_message`, the same seam
/// `engrams::STALE_EDIT` classifies on, spelled again here rather than
/// shared across the two modules.
const STALE_EDIT: &str = "stale edit";

/// The manifest response both the GET and the PUT answer with: the domain,
/// the markdown, its checksum, and the same checksum again as a quoted `ETag`
/// header - one shape for a manifest on this surface, so a client that has
/// just saved one holds what the GET route would have given it.
fn manifest_response(
    domain: &str,
    markdown: String,
    status: StatusCode,
) -> Result<Response, ApiError> {
    let checksum = manifest_checksum(&markdown);
    let etag = HeaderValue::from_str(&format!("\"{checksum}\""))
        .map_err(|_| ApiError::internal("the manifest's checksum is not a usable ETag"))?;
    let mut resp = (
        status,
        Json(json!({ "domain": domain, "markdown": markdown, "checksum": checksum })),
    )
        .into_response();
    resp.headers_mut().insert(ETAG, etag);
    Ok(resp)
}

/// The manifest's strong validator: sha256 of the markdown, the same token the
/// engine's save compares. Computed here because the manifest read is a plain
/// string, not an engine read payload that already carries a checksum.
fn manifest_checksum(markdown: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(markdown.as_bytes());
    crystalline_index::hex_lower(&hasher.finalize())
}
