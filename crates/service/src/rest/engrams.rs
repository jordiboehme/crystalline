//! The ways a client reaches an engram: a filtered listing of a domain, one
//! engram in full, a new one, and a full-document save of one that exists.
//!
//! The reads hand the engine's own JSON over unchanged, so this API and the MCP
//! tools answer with one payload rather than two shapes that drift. The detail
//! route adds exactly one thing on top: an `ETag`, so a client that later wants
//! to write back can say which version it read.
//!
//! The two writes are the first content mutations on this surface, so the rules
//! they are held to are written down here rather than left to be re-derived by
//! whatever route lands next:
//!
//! 1. **Editor only, in the handler.** [`super::auth::guard`] enforces viewer
//!    and nothing more, so both handlers open with [`Identity::require_editor`].
//!    That is also what refuses the anonymous viewer: an identity with no
//!    account behind it can never write, whatever `auth.anonymous` allows it to
//!    read.
//! 2. **Cross-site protection is the middleware's.** Every unsafe method from
//!    an account-bearing identity must echo its session's CSRF token; see
//!    `check_csrf`. Nothing here re-implements that check or exempts itself
//!    from it.
//! 3. **Read-only is answered before preconditions.** [`save`] refuses a
//!    read-only instance ahead of parsing `If-Match`, so an instance that
//!    refuses writes answers 403 rather than sending a client to fetch an ETag
//!    for a write that can never land. The engine refuses too, but only after
//!    the header parse would already have answered 428.

use axum::Json;
use axum::extract::State;
use axum::http::header::ETAG;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use utoipa::IntoParams;

use super::auth::Identity;
use super::{
    ApiError, ApiJson, ApiPath, ApiQuery, ConflictDetail, ProblemDetail, RestState, csv, if_match,
    precondition_failed,
};
use crate::engine::EngineError;
use crate::params::{
    DeleteParams, MoveParams, ReadParams, RetireParams, SaveParams, SearchParams, WriteParams,
};

/// The query string `GET /domains/{domain}/engrams` takes: the filter side of
/// [`SearchParams`], minus the domain the path already names and minus the
/// free-text query, which belongs to the search endpoint rather than a listing.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Filter by `type`.
    #[serde(rename = "type", default)]
    #[param(example = "decision")]
    engram_type: Option<String>,
    /// Filter by `status`.
    #[serde(default)]
    #[param(example = "stable")]
    status: Option<String>,
    /// Require all of these tags, comma separated. A URL is a string, so the
    /// repeated-parameter form a `Vec` would need is not on offer here; the
    /// list is split on the way in.
    #[serde(default)]
    #[param(example = "eng,nested")]
    tags: Option<String>,
    /// Only engrams recorded on or after this ISO date.
    #[serde(default)]
    #[param(example = "2026-01-31")]
    after: Option<String>,
    /// Only engrams filed under this domain-relative folder, the folder and
    /// everything below it. A folder rather than a string prefix: `notes`
    /// takes `notes/deep/y.md` and never `notes-misc/z.md`. Absent or empty
    /// is the whole domain.
    #[serde(default)]
    #[param(example = "notes")]
    path: Option<String>,
    /// One-based page number. Defaults to 1.
    #[serde(default)]
    #[param(example = 1)]
    page: Option<usize>,
    /// Page size. Defaults to 10.
    #[serde(default)]
    #[param(example = 10)]
    limit: Option<usize>,
}

/// `GET /domains/{domain}/engrams` - one domain's engrams, filtered by
/// frontmatter and paged, which is what a browsing client lists from.
///
/// This is `search_engrams` with no query text: a filter-only search, so the
/// answer carries the engine's own page envelope (`total`, `page`, `limit`,
/// `count`, `hits`) and a client pages it the way it pages a search.
///
/// `path` scopes it to one folder, which is what a folder view pages from: the
/// folder is pushed into SQL beside the other filters, so `total` stays exact
/// under it and a folder holding thousands of engrams costs one page rather
/// than the folder. The tree endpoint still owns the navigation view - a level
/// of folders and their children - while this one owns the listing, paged and
/// filtered; neither reimplements the other.
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
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/engrams",
    tag = "engrams",
    operation_id = "list_engrams",
    summary = "One domain's engrams, filtered by frontmatter and paged.",
    description = "A filter-only search, so the answer carries the same page \
                   envelope a search does and a client pages it the same \
                   way.\n\n`path` scopes the listing to one folder, the folder \
                   and everything below it, and it is a folder rather than a \
                   string prefix: `notes` takes `notes/deep/y.md` and never \
                   `notes-misc/z.md`. The total stays exact under it, so a \
                   folder holding thousands of engrams costs one page rather \
                   than the folder. The tree endpoint still owns the \
                   navigation view and this one owns the listing.\n\nA \
                   domain nobody registered is a 404, while filters that match \
                   nothing are an empty page: two states a client can tell \
                   apart.",
    params(("domain" = String, Path, description = "The registered domain."), ListQuery),
    responses(
        (
            status = 200,
            description = "The engine's page envelope, unchanged.",
            body = Object,
            example = json!({
                "mode": "text",
                "total": 4,
                "page": 1,
                "limit": 10,
                "count": 1,
                "hits": [{
                    "domain": "eng",
                    "permalink": "alpha",
                    "title": "Alpha",
                    "snippet": "The first engram in this domain.",
                    "score": 1.0,
                    "engram_type": "engram",
                    "status": "stable",
                    "tags": ["eng"],
                    "kind": "engram"
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
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    state.engine.require_domain(&domain)?;
    let value = state
        .engine
        .search_engrams_under(
            &SearchParams {
                // No text: the filters alone select, and the engine takes that
                // as the filter-only mode rather than as an empty search.
                query: None,
                domains: vec![domain],
                engram_type: query.engram_type,
                tags: csv(query.tags.as_deref()),
                status: query.status,
                after: query.after,
                page: query.page,
                limit: query.limit,
                ..SearchParams::default()
            },
            query.path.as_deref(),
        )
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
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/engrams/{permalink}",
    tag = "engrams",
    operation_id = "get_engram",
    summary = "One engram in full.",
    description = "Its frontmatter, its markdown as written and the references \
                   the engine resolves around it.\n\nThe response carries an \
                   `ETag` over the markdown, so a client knows which version it \
                   is holding and can say so when it later writes back.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "permalink" = String,
            Path,
            description = "The engram permalink. A permalink is a path, so this \
                           segment may contain slashes: `notes/deep/gamma`.",
            example = "notes/deep/gamma",
        ),
    ),
    responses(
        (
            status = 200,
            description = "The engine's own read payload, unchanged.",
            body = Object,
            headers(("etag" = String, description = "The quoted checksum of the \
                     engram as read, the same token a later write compares an \
                     `expected_checksum` against.")),
            example = json!({
                "domain": "eng",
                "permalink": "alpha",
                "title": "Alpha",
                "type": "engram",
                "status": "stable",
                "path": "alpha.md",
                "url": "crystalline://eng/alpha",
                "content": "---\ntitle: Alpha\n---\n\nThe first engram.\n",
                "checksum": "3f8a1c05e2",
                "frontmatter": { "title": "Alpha", "permalink": "alpha" },
                "observations": [],
                "relations": [],
                "links": []
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
            description = "No such domain or engram.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
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

/// What `POST /domains/{domain}/engrams` takes: the create form's fields, fed
/// to the engine's write verb unchanged.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "A new engram. The engine builds the frontmatter from \
                        these fields and slugifies the title into the filename \
                        and permalink, so the body carries markdown only.")]
pub struct CreateEngramBody {
    /// The engram title. Slugified into the filename and permalink.
    #[schema(example = "Alpha")]
    title: String,
    /// The markdown body (no frontmatter: creation builds it).
    #[schema(example = "# Alpha\n\nA rule about alpha.\n")]
    content: String,
    /// A domain-relative subfolder. Defaults to the root.
    #[serde(default)]
    #[schema(example = "notes")]
    folder: Option<String>,
    /// The engram `type`. Defaults to `engram`. Free form; recommended values
    /// are guidance.
    #[serde(rename = "type", default)]
    #[schema(example = "decision")]
    engram_type: Option<String>,
    /// Lifecycle `status`. Defaults to `stable`. Free form.
    #[serde(default)]
    status: Option<String>,
    /// Tags, lowercase-with-hyphens.
    #[serde(default)]
    tags: Vec<String>,
    /// Extra frontmatter keys (valid_from, valid_to, salience, ...), passed to
    /// the engine's metadata contract unchanged.
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

/// What `PUT /domains/{domain}/engrams/{permalink}` takes: the complete file
/// text, frontmatter included, written verbatim.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "The complete file text, frontmatter included. It is \
                        written verbatim: nothing here rebuilds the \
                        frontmatter or stamps provenance, so what a client \
                        reads back is what its author typed.")]
pub struct SaveEngramBody {
    /// The full markdown text as the editor holds it.
    #[schema(example = "---\ntitle: Alpha\npermalink: alpha\n---\n\nA sharper rule.\n")]
    content: String,
}

/// What `POST /domains/{domain}/retire` takes: guided retirement of one
/// engram. The permalink rides in the body rather than the path, because a
/// permalink is a path of its own and the engram route's wildcard cannot be
/// followed by an action segment - `/engrams/{*permalink}/retire` would eat
/// `retire` as part of the permalink instead of routing to this handler.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "Guided retirement of one engram: a `status` from a \
                        fixed set, an optional close-out date, and, for \
                        `superseded`, the successor that wires the \
                        reciprocal relation pair.")]
pub struct RetireBody {
    /// The engram to retire, by permalink.
    #[schema(example = "notes/beta")]
    permalink: String,
    /// The retirement status: deprecated, superseded or archived.
    #[schema(example = "superseded")]
    status: String,
    /// The successor's permalink, wiring superseded_by / supersedes. Required
    /// for superseded, refused otherwise.
    #[serde(default)]
    successor: Option<String>,
    /// The date validity ends, plain ISO (YYYY-MM-DD). Absent means unknown.
    #[serde(default)]
    valid_to: Option<String>,
}

/// What `POST /domains/{domain}/move` takes: the engram to move, by
/// permalink, and where it goes. The permalink rides in the body for the same
/// reason [`RetireBody`]'s does.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "Move an engram to a new path, or into another \
                        registered domain. Inbound bare links are rewritten \
                        to the domain-prefixed form on a cross-domain move.")]
pub struct MoveBody {
    /// The engram to move, by permalink.
    #[schema(example = "notes/beta")]
    permalink: String,
    /// The new domain-relative path, with or without `.md`.
    #[schema(example = "guides/beta")]
    destination: String,
    /// Move into another registered domain. Inbound bare links are rewritten
    /// to the domain-prefixed form.
    #[serde(default)]
    destination_domain: Option<String>,
}

/// `POST /domains/{domain}/engrams` - create an engram from a title and a
/// markdown body, answering 201 with the detail read of what landed.
///
/// The engine builds the frontmatter and slugifies the title into the filename
/// and permalink, exactly as the MCP write tool does, and the account behind
/// the request is named in the engram's `generated.by` as `human:<name>` - so a
/// domain records who taught it what, whether that was a person through this
/// API or an agent through MCP.
///
/// The answer is the detail read rather than the write verb's own receipt, so
/// a client that has just created an engram holds the same payload the detail
/// route serves, `ETag` included, and can go straight to editing it without a
/// second round trip.
///
/// A permalink already taken is the engine's `Conflict`, answered 409: this
/// route never overwrites, so a client that means to replace something saves it
/// through the PUT with the token it read.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/engrams",
    tag = "engrams",
    operation_id = "create_engram",
    summary = "Create an engram from a title and a markdown body.",
    description = "The engine builds the frontmatter and slugifies the title \
                   into the filename and permalink, and the account behind the \
                   request is named in the engram's `generated.by`.\n\nThe \
                   answer is the detail read of what landed, `ETag` included, \
                   so a client can go straight to editing it. This route never \
                   overwrites: a permalink already taken is a 409, and \
                   replacing an engram is what the PUT is for.",
    params(("domain" = String, Path, description = "The registered domain.")),
    request_body = CreateEngramBody,
    responses(
        (
            status = 201,
            description = "The engine's own read payload for the new engram.",
            body = Object,
            headers(("etag" = String, description = "The quoted checksum of the \
                     engram as written, the token a later save carries in \
                     `If-Match`.")),
        ),
        (
            status = 400,
            description = "The body is not JSON.",
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
            description = "The caller is not an editor, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled account.",
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
            status = 409,
            description = "That permalink is already taken in this domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 413,
            description = "The body is over the 10 MiB limit this API accepts.",
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
            description = "The body is JSON but not an engram, the title does \
                           not slugify to a permalink, the metadata breaks the \
                           frontmatter contract, or the target is one of the \
                           reserved OKF names (`index.md`, `log.md`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn create(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<CreateEngramBody>,
) -> Result<Response, ApiError> {
    let caller = identity.require_editor()?;
    // The provenance the engram records. `human:` rather than a bare name so
    // `generated.by` says what kind of author this was: an MCP client writes
    // its own `clientname/version` there, and the two must not be mistaken for
    // one another when a domain is read back years later.
    let actor = format!("human:{}", caller.name());
    // A reserved target (`index.md`, `log.md`) is refused inside
    // `write_engram_as`, which owns the title-to-filename slugification and so
    // is the only place that knows what would actually be written. Repeating
    // the check here would mean repeating that derivation, which is the kind of
    // second copy that drifts.
    let written = state
        .engine
        .write_engram_as(
            &WriteParams {
                domain: domain.clone(),
                title: body.title,
                content: body.content,
                folder: body.folder,
                engram_type: body.engram_type,
                tags: body.tags,
                status: body.status,
                metadata: body.metadata,
                // Never from this route: replacing an engram goes through the
                // PUT, which demands the token of the version being replaced.
                overwrite: false,
            },
            Some(&actor),
        )
        .await
        // The engine reports a taken permalink as a conflict, which this
        // surface answers 409 rather than the 422 its generic classification
        // gives: nothing about the request can be corrected to make it
        // succeed - the engram is already there - and a client branches on
        // that difference to offer opening the existing one instead. The match
        // is on the variant rather than on the message: a collision is the only
        // way this verb conflicts, whichever layer noticed it.
        .map_err(|e| match e {
            EngineError::Conflict(message) => ApiError::conflict(message),
            other => other.into(),
        })?;
    let permalink = written["permalink"]
        .as_str()
        .ok_or_else(|| ApiError::internal("the write did not report a permalink to read back"))?
        .to_string();
    detail_response(&state, &domain, &permalink, StatusCode::CREATED).await
}

/// `PUT /domains/{domain}/engrams/{*permalink}` - save an engram's complete
/// markdown text, guarded by the `If-Match` token of the version being
/// replaced.
///
/// The text lands verbatim: nothing rebuilds the frontmatter and nothing stamps
/// provenance, so a client that saves what it read writes back byte-identical
/// bytes. That is the editor's fidelity contract, and it is why this is a PUT
/// of the whole document rather than a patch of its parts.
///
/// The three answers a client has to handle:
///
/// - **428** when no `If-Match` arrived. The token comes from the detail read.
/// - **412** when the token no longer matches, carrying the version the server
///   holds now (`current_etag`, `current_content`) so a client can show a merge
///   view instead of asking its author to retype the edit.
/// - **200** with the detail read of what landed and its new `ETag`.
///
/// One consequence of writing the text verbatim is worth knowing: an author who
/// edits the `permalink` in the frontmatter moves the engram's address, since
/// the index takes the permalink from the file. Rewriting the caller's
/// frontmatter to keep the URL and the document in step is deliberately not
/// done - an editor that saved something other than what its author typed would
/// be the worse surprise - so the save follows the engram instead: the engine's
/// receipt names the permalink the engram answers to *after* the write, and the
/// detail read in the answer is taken at that address. A rename therefore
/// answers 200 with the engram at its new permalink, and the `ETag` in that
/// answer is the token for the next save of it.
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/engrams/{permalink}",
    tag = "engrams",
    operation_id = "save_engram",
    summary = "Save an engram's complete markdown text.",
    description = "The text lands verbatim, frontmatter included: nothing \
                   rebuilds it and nothing stamps provenance, so a client that \
                   saves what it read writes back byte-identical bytes.\n\nThe \
                   write is guarded by `If-Match`, whose token is the `ETag` of \
                   the detail read it is based on: a save that arrives without \
                   one is answered 428, and one whose token is stale is \
                   answered 412 carrying the version the server holds now, so a \
                   client can merge rather than lose the edit.\n\nEditing the \
                   `permalink` in the frontmatter moves the engram's address, \
                   since the index takes the permalink from the file. Such a \
                   save is answered 200 with the engram read at its new \
                   address, so a client can follow the move rather than lose \
                   track of what it just wrote.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "permalink" = String,
            Path,
            description = "The engram permalink. A permalink is a path, so this \
                           segment may contain slashes: `notes/deep/gamma`.",
            example = "notes/deep/gamma",
        ),
        (
            "If-Match" = String,
            Header,
            description = "The quoted `ETag` of the version being replaced, \
                           from the detail read.",
            example = "\"3f8a1c05e2\"",
        ),
    ),
    request_body = SaveEngramBody,
    responses(
        (
            status = 200,
            description = "The engine's own read payload for the saved engram.",
            body = Object,
            headers(("etag" = String, description = "The quoted checksum of the \
                     engram as saved, the token the next save carries.")),
        ),
        (
            status = 400,
            description = "The body is not JSON, or `If-Match` carries more \
                           than one entity tag: this surface expects exactly \
                           one strong checksum, not a comma-separated list.",
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
            description = "The caller is not an editor, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account. A read-only instance answers this ahead of \
                           the precondition check, so it is never 428.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or engram, or the engram is indexed \
                           but its file is not on this machine.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 412,
            description = "The `If-Match` token is stale. The body carries the \
                           version the server holds now, so a client can merge.",
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
            description = "The document is not an engram (unparseable, or no \
                           frontmatter block), the `If-Match` is a wildcard or \
                           a weak validator, or the target is one of the \
                           reserved OKF names (`index.md`, `log.md`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 428,
            description = "No `If-Match` arrived. The token comes from the \
                           detail read.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn save(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
    ApiPath((domain, permalink)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<SaveEngramBody>,
) -> Result<Response, ApiError> {
    identity.require_editor()?;
    // Before the If-Match parse, not after: an instance that refuses writes
    // refuses them whatever headers arrive, so this answers 403 rather than
    // sending a client off to fetch a token for a write that can never land.
    // The engine refuses too, but only once `if_match` has already answered.
    if state.engine.read_only() {
        return Err(ApiError::forbidden(
            "this instance is read-only; content mutations are disabled",
        ));
    }
    // The reserved OKF names are generated or reserved rather than authored, so
    // they are refused here as well as inside the engine: this catches the URL
    // shape without a database round trip, and the engine catches whatever
    // resolves to a reserved path by any other route.
    if crystalline_core::is_reserved_path(&format!("{permalink}.md")) {
        return Err(ApiError::unprocessable(format!(
            "'{permalink}' is a reserved OKF name: index.md is generated from \
             its folder and log.md is reserved beside it, so neither is an \
             engram this API writes"
        )));
    }
    let token = if_match(&headers)?;
    match state
        .engine
        .save_engram(&SaveParams {
            domain: domain.clone(),
            identifier: permalink.clone(),
            content: body.content,
            expected_checksum: token,
        })
        .await
    {
        // Read back from the receipt rather than reusing the URL's permalink:
        // the text landed verbatim, so an author who edited the `permalink`
        // line has moved the address, and the detail read has to follow it or
        // answer 404 for a write that succeeded.
        Ok(receipt) => {
            let moved = receipt["permalink"]
                .as_str()
                .ok_or_else(|| {
                    ApiError::internal("the save did not report a permalink to read back")
                })?
                .to_string();
            detail_response(&state, &domain, &moved, StatusCode::OK).await
        }
        // The one conflict this route translates rather than propagates. Keyed
        // on the prefix `stale_edit_message` owns, which is the seam both
        // storage kinds speak: a file domain compares in the engine and a
        // virtual one in the database's compare-and-swap, and each reports the
        // same sentence. Any other conflict is somebody else's rule and keeps
        // its own status.
        Err(EngineError::Conflict(message)) if message.starts_with(STALE_EDIT) => {
            let current = state
                .engine
                .read_engram(&ReadParams {
                    identifier: permalink,
                    domain: Some(domain),
                })
                .await?;
            let checksum = current["checksum"].as_str().ok_or_else(|| {
                ApiError::internal("the engram read carried no checksum to version it by")
            })?;
            let content = current["content"].as_str().unwrap_or_default().to_string();
            Ok(precondition_failed(message, checksum, content))
        }
        Err(e) => Err(e.into()),
    }
}

/// `POST /domains/{domain}/retire` - guided retirement of one engram: set a
/// retirement `status`, optionally close out `valid_to`, and, for
/// `superseded`, wire the supersede pair as body relations.
///
/// No `If-Match` here: this is a guided edit rather than a full-document
/// replace, the same reason the MCP verb behind it takes none, so nothing on
/// this route depends on a version the caller read first.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/retire",
    tag = "engrams",
    operation_id = "retire_engram",
    summary = "Guided retirement of one engram.",
    description = "Sets a retirement `status` (deprecated, superseded or \
                   archived), optionally closes out `valid_to`, and for \
                   `superseded` wires the reciprocal `superseded_by` / \
                   `supersedes` relation pair so verify's T005 and the \
                   evolve sweep see a live pair rather than a dangling one.\n\n\
                   The permalink rides in the body rather than the path: a \
                   permalink is a path of its own and the engram route's \
                   wildcard cannot be followed by an action segment.",
    params(("domain" = String, Path, description = "The registered domain.")),
    request_body = RetireBody,
    responses(
        (
            status = 200,
            description = "The retirement receipt: domain, permalink, the \
                           status now set and the resolved successor \
                           permalink, if any.",
            body = Object,
            example = json!({
                "domain": "eng",
                "permalink": "alpha",
                "status": "superseded",
                "successor": "beta"
            }),
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
            description = "The caller is not an editor, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, engram, or - when status is \
                           superseded - successor.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "A conflict the engine's retirement rule raises, \
                           answered 409 like the collision this API's other \
                           writes raise. Reserved for classification parity \
                           with create and move; guided retirement raises no \
                           conflict from any input this route accepts today.",
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
            description = "The status is not deprecated, superseded or \
                           archived; a successor is missing for superseded or \
                           given for another status; the successor resolves \
                           to the same engram being retired; or valid_to is \
                           not a plain ISO date.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn retire(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<RetireBody>,
) -> Result<Json<Value>, ApiError> {
    let caller = identity.require_editor()?;
    let value = state
        .engine
        .retire_engram_as(
            &RetireParams {
                domain,
                identifier: body.permalink,
                status: body.status,
                successor: body.successor,
                valid_to: body.valid_to,
            },
            Some(&format!("human:{}", caller.name())),
        )
        .await
        // Classified 409 like `create`'s collision, for parity across this
        // API's write endpoints even though this verb raises no conflict
        // today; see the 409 response doc above.
        .map_err(|e| match e {
            EngineError::Conflict(message) => ApiError::conflict(message),
            other => other.into(),
        })?;
    Ok(Json(value))
}

/// `POST /domains/{domain}/move` - move an engram to a new path, or into
/// another registered domain.
///
/// Named `move_action` because `move` is a keyword; the operation id served
/// to clients stays `move_engram`, matching the engine verb behind it.
///
/// No `If-Match` here, for the same reason [`retire`] takes none: this
/// carries content between two truths rather than replacing a version the
/// caller read.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/move",
    tag = "engrams",
    operation_id = "move_engram",
    summary = "Move an engram to a new path, or into another domain.",
    description = "A same-domain move is a rename; a cross-domain move reads \
                   the source content and re-indexes it into the \
                   destination's source, rewriting inbound bare links from \
                   other domains to the domain-prefixed form.\n\nThe \
                   permalink rides in the body for the same reason \
                   `RetireBody`'s does: the engram route's wildcard cannot be \
                   followed by an action segment.",
    params(("domain" = String, Path, description = "The engram's current domain.")),
    request_body = MoveBody,
    responses(
        (
            status = 200,
            description = "The move receipt: where the engram came from, \
                           where it landed, whether the move crossed domains \
                           and how many inbound links were rewritten.",
            body = Object,
            example = json!({
                "from": { "domain": "eng", "permalink": "beta", "path": "beta.md" },
                "to": { "domain": "eng", "path": "guides/beta.md" },
                "cross_domain": false,
                "links_rewritten": 0
            }),
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
            description = "The caller is not an editor, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or engram.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The destination already exists in the target \
                           domain.",
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
            description = "The destination path is empty, or resolves to one \
                           of the reserved OKF names (`index.md`, `log.md`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn move_action(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<MoveBody>,
) -> Result<Json<Value>, ApiError> {
    identity.require_editor()?;
    let value = state
        .engine
        .move_engram(&MoveParams {
            identifier: body.permalink,
            domain,
            destination: body.destination,
            destination_domain: body.destination_domain,
            update_links: None,
        })
        .await
        // The one collision this verb can hit: a destination already taken.
        // Answered 409 rather than the generic 422 caller-error class, same
        // reasoning as `create`'s own translation.
        .map_err(|e| match e {
            EngineError::Conflict(message) => ApiError::conflict(message),
            other => other.into(),
        })?;
    Ok(Json(value))
}

/// What `POST /validate` takes: the document a save would write, checked
/// without writing.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "The document a save would write, checked without \
                        writing. Lives beside the engram routes because what \
                        it validates is engram markdown, not because it is \
                        scoped to a domain the way the other write routes are.")]
pub struct ValidateBody {
    /// The full markdown text, frontmatter included.
    #[schema(example = "---\ntitle: Alpha\n---\n\nA rule about alpha.\n")]
    content: String,
    /// The domain the document belongs (or will belong) to. Names the scan
    /// root in findings; defaults to "draft".
    #[serde(default)]
    #[schema(example = "eng")]
    domain: Option<String>,
    /// The domain-relative path the document sits (or will sit) at. Defaults
    /// to "draft.md".
    #[serde(default)]
    #[schema(example = "alpha.md")]
    path: Option<String>,
}

/// One finding, and the envelope: the same fields the verify report carries.
///
/// Shared rather than validation-local: the archive preview reports the same
/// findings per entry, and a second shape for the same facts would drift.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ValidateFinding {
    /// The rule id, for example E002 or T005.
    #[schema(example = "T005")]
    rule: String,
    /// error, warning or info. Hard errors are what a client blocks a save on.
    #[schema(example = "warning")]
    severity: String,
    /// What is wrong, in the rule's own words.
    message: String,
    /// The one-based source line, when the finding points at one.
    line: Option<usize>,
    /// A suggested fix, when the rule has one.
    fix: Option<String>,
}

/// What `POST /validate` answers: every finding the format and temporal rule
/// families raise over the document, and how many of them are hard errors.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ValidateResponse {
    /// Every finding, format and temporal families, default severities.
    findings: Vec<ValidateFinding>,
    /// How many findings are hard errors.
    errors: usize,
}

/// `POST /validate` - the dry-run that gives an editor pre-save feedback:
/// every issue `crystalline verify`'s format (`E`) and temporal (`T`) rule
/// families would raise over the document, without writing anything or even
/// requiring the engram to exist yet.
///
/// Link, manifest, schema and quality rules need a whole domain for context
/// and are not run here; see [`crystalline_core::verify::check_document`],
/// which this handler calls unchanged. `T006` (missing write provenance) is
/// dropped by that function for the same reason it is dropped from every
/// other caller of it: a fresh, unsaved document has not been through the
/// write pipeline that stamps provenance, so flagging its absence here would
/// nag an editor about a field the save is about to add.
///
/// Refused like every other write on this surface - editor role, read-only
/// answered first - even though nothing here is ever written: a dry run that
/// bypassed those gates would let a viewer or a read-only instance run the
/// rule engine over arbitrary content, which is scope this route does not
/// mean to open. There is no engine verb to carry the read-only check, so the
/// handler makes it itself.
#[utoipa::path(
    post,
    path = "/api/v1/validate",
    tag = "engrams",
    operation_id = "validate_document",
    summary = "Pre-save validation: the findings a save would raise, without writing.",
    description = "Runs verify's format (`E`) and temporal (`T`) rule \
                   families over the document text, the same families \
                   `crystalline verify` runs for a single document. Link, \
                   manifest, schema and quality rules need a whole domain for \
                   context and are not run here.\n\n`T006` (missing write \
                   provenance) is dropped, so a fresh unsaved document is not \
                   nagged about a field the save is about to stamp.\n\nRefused \
                   like every other write on this surface - editor role, \
                   read-only answered first - even though nothing is ever \
                   written.",
    request_body = ValidateBody,
    responses(
        (
            status = 200,
            description = "Every finding, and how many are hard errors.",
            body = ValidateResponse,
            example = json!({
                "findings": [{
                    "rule": "T005",
                    "severity": "warning",
                    "message": "status is `superseded` but no `superseded_by` relation is present",
                    "line": null,
                    "fix": "add `- superseded_by [[Target]]`"
                }],
                "errors": 0
            }),
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
            description = "The caller is not an editor, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn validate(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<ValidateBody>,
) -> Result<Json<ValidateResponse>, ApiError> {
    identity.require_editor()?;
    // No engine verb runs here, so nothing else answers read-only; the
    // handler makes the check itself, ahead of running the rule engine, for
    // the same reason `save` and `remove` answer it before their own
    // preconditions: an instance that refuses writes refuses them whatever
    // the request otherwise looks like.
    if state.engine.read_only() {
        return Err(ApiError::forbidden(
            "this instance is read-only; content mutations are disabled",
        ));
    }
    let domain = body.domain.as_deref().unwrap_or("draft");
    let rel = body.path.as_deref().unwrap_or("draft.md");
    let issues =
        crystalline_core::verify::check_document(domain, std::path::Path::new(rel), &body.content);
    let findings = findings_of(issues);
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    Ok(Json(ValidateResponse { findings, errors }))
}

/// Verify issues as this surface reports them.
///
/// One mapping for two callers - this module's `/validate` and the archive
/// preview's per-entry findings - so a document checked before a save and the
/// same document checked inside an archive are described identically.
pub(super) fn findings_of(issues: Vec<crystalline_core::verify::Issue>) -> Vec<ValidateFinding> {
    issues
        .into_iter()
        .map(|i| ValidateFinding {
            rule: i.rule.to_string(),
            severity: match i.severity {
                crystalline_core::verify::Severity::Error => "error",
                crystalline_core::verify::Severity::Warning => "warning",
                crystalline_core::verify::Severity::Info => "info",
            }
            .to_string(),
            message: i.message,
            line: i.line,
            fix: i.fix,
        })
        .collect()
}

impl ValidateFinding {
    /// Whether this is a hard error - the severity that withholds a document
    /// from a write. A method rather than a comparison at every call site, so
    /// nothing has to know how the severity is spelled on the wire.
    pub(super) fn is_error(&self) -> bool {
        self.severity == "error"
    }

    /// The finding as one line, for a report that has room for a reason but
    /// not for a findings list beside it.
    pub(super) fn summary(&self) -> String {
        format!("{}: {}", self.rule, self.message)
    }
}

/// `DELETE /domains/{domain}/engrams/{*permalink}` - hard delete an engram,
/// guarded by the `If-Match` token of the version being removed.
///
/// The same three answers [`save`] is held to, because the guard is the same
/// contract: 428 with no `If-Match`, 412 with a stale one (carrying the
/// version the server holds now, so a client can decide whether losing it is
/// really what it meant), 204 once it lands.
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}/engrams/{permalink}",
    tag = "engrams",
    operation_id = "delete_engram",
    summary = "Hard delete an engram, guarded by If-Match.",
    description = "A file domain removes the file from disk; a virtual \
                   domain drops the database rows. Guarded the same way \
                   `save` is: 428 with no `If-Match`, 412 when the token is \
                   stale (carrying the version the server holds now), 204 \
                   once it lands. A read-only instance answers 403 ahead of \
                   the precondition check, so it is never 428.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "permalink" = String,
            Path,
            description = "The engram permalink. A permalink is a path, so this \
                           segment may contain slashes: `notes/deep/gamma`.",
            example = "notes/deep/gamma",
        ),
        (
            "If-Match" = String,
            Header,
            description = "The quoted `ETag` of the version being deleted, \
                           from the detail read.",
            example = "\"3f8a1c05e2\"",
        ),
    ),
    responses(
        (
            status = 204,
            description = "Deleted. No body.",
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
            description = "The caller is not an editor, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account. A read-only instance answers this ahead of \
                           the precondition check, so it is never 428.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or engram.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 412,
            description = "The `If-Match` token is stale. The body carries the \
                           version the server holds now, so a client can \
                           decide whether losing it is really what it meant.",
            body = ConflictDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 428,
            description = "No `If-Match` arrived. The token comes from the \
                           detail read.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
    ApiPath((domain, permalink)): ApiPath<(String, String)>,
) -> Result<Response, ApiError> {
    identity.require_editor()?;
    // Before the If-Match parse, not after: the same reasoning as `save`'s
    // own read-only check, repeated here rather than shared, since the two
    // handlers are not yet worth abstracting over.
    if state.engine.read_only() {
        return Err(ApiError::forbidden(
            "this instance is read-only; content mutations are disabled",
        ));
    }
    let token = if_match(&headers)?;
    match state
        .engine
        .delete_engram(&DeleteParams {
            identifier: permalink.clone(),
            domain: domain.clone(),
            expected_checksum: Some(token),
        })
        .await
    {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        // The same stale-edit translation `save` makes, repeated rather than
        // shared for the same reason.
        Err(EngineError::Conflict(message)) if message.starts_with(STALE_EDIT) => {
            let current = state
                .engine
                .read_engram(&ReadParams {
                    identifier: permalink,
                    domain: Some(domain),
                })
                .await?;
            let checksum = current["checksum"].as_str().ok_or_else(|| {
                ApiError::internal("the engram read carried no checksum to version it by")
            })?;
            let content = current["content"].as_str().unwrap_or_default().to_string();
            Ok(precondition_failed(message, checksum, content))
        }
        Err(e) => Err(e.into()),
    }
}

/// The prefix every refused compare-and-swap opens with, wherever the
/// comparison happened. See `engine::stale_edit_message`, which is its only
/// source; the phrase is the seam this layer classifies on, so it is spelled
/// once here rather than at each use.
const STALE_EDIT: &str = "stale edit";

/// The detail read of an engram, answered with `status` and its `ETag`.
///
/// Both writes answer with this rather than with the engine's write receipt, so
/// there is exactly one payload shape for an engram on this surface and a
/// client that has just written one holds what the detail route would have
/// given it.
async fn detail_response(
    state: &RestState,
    domain: &str,
    permalink: &str,
    status: StatusCode,
) -> Result<Response, ApiError> {
    let value = state
        .engine
        .read_engram(&ReadParams {
            identifier: permalink.to_string(),
            domain: Some(domain.to_string()),
        })
        .await?;
    let etag = etag(&value)?;
    let mut resp = (status, Json(value)).into_response();
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
