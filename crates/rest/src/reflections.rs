//! The Reflection: the routes Fluid's Reflection mode and review screen
//! read and write. Every verb is the engine's; this module builds the caller,
//! maps the four "not here" states to 409 and passes the engine's JSON
//! through unchanged, so the MCP tools and this surface describe one thing.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};

use super::auth::Identity;
use super::{ApiError, ApiJson, ApiPath, ApiQuery, ProblemDetail, RestState, refuse_read_only};
use crate::engine::{
    ANONYMOUS_COMMENT, ASSET_EXISTS, EngineError, MOVE_ONTO_A_FILE, REFUSED_ELSEWHERE_START,
    REFUSED_HOMELESS, REFUSED_HOSTED_START, REFUSED_OFF_START, REFUSED_PENDING,
    REFUSED_PENDING_NAMED, REFUSED_PENDING_SPELLINGS, REFUSED_PENDING_UNOPENED,
    REFUSED_PENDING_UNREADABLE, REFUSED_UNCONNECTED_TEAM, ReflectionSearch, TARGET_TAKEN,
};
use crate::files::{AttachmentView, attachment_response};
use crate::params::{DiscussParams, WriteReflectionParams};
use crate::scope::ReflectionCaller;

/// The person behind a request, as a reflection verb sees them.
pub(crate) fn caller(identity: &Identity) -> ReflectionCaller {
    let name = identity
        .user
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_else(|| "anonymous".to_string());
    ReflectionCaller::person(identity.scope(), &name)
}

/// The anonymous viewer's answer to a page write (save, move, delete).
pub(crate) const ANONYMOUS_EDIT: &str =
    "anonymous visitors read this Reflection and cannot change it; sign in to edit";

/// Refuse a reflection write by the anonymous viewer, before the engine
/// reads anything else of the request. Where it may not read the domain it
/// learns nothing: the domain answers the 404 an unregistered name answers,
/// on every route. Where it reads the Reflection (a public domain), the
/// answer is 401 with `sentence`, since signing in is what fixes it, and it
/// comes whatever the Reflection's state, the answer every other write on
/// this surface gives the anonymous tier.
pub(crate) async fn refuse_anonymous(
    state: &RestState,
    identity: &Identity,
    domain: &str,
    sentence: &str,
) -> Result<(), ApiError> {
    if identity.user.is_some() {
        return Ok(());
    }
    state
        .engine
        .reflection_status(domain, &caller(identity))
        .await
        .map_err(reflection_error)?;
    Err(ApiError::unauthorized(sentence))
}

/// Engine errors as this surface answers them: the five Reflection states a
/// verb refuses on are 409 (the request is fine, the Reflection is not where
/// it could serve it), with `home_url` when it lives elsewhere; a stale
/// checksum, a create over an existing page, a move or delete of a page
/// somebody is editing, a page move onto a taken target and a file move
/// onto a path that already holds a file are 409 (the shared mapping answers a write onto a path only the
/// previous host of a host move has 409 too, and every other `Conflict`
/// 422);
/// everything else keeps the shared mapping.
pub(crate) fn reflection_error(e: EngineError) -> ApiError {
    match &e {
        EngineError::Refused(text) if text.starts_with(REFUSED_ELSEWHERE_START) => {
            let url = text[REFUSED_ELSEWHERE_START.len()..].to_string();
            ApiError::conflict(text.clone()).home_url(url)
        }
        EngineError::Refused(text)
            if text.starts_with(REFUSED_OFF_START)
                || text.starts_with(REFUSED_HOSTED_START)
                || text.contains(REFUSED_PENDING_NAMED)
                || text.contains(REFUSED_PENDING)
                || text.contains(REFUSED_PENDING_UNREADABLE)
                || text.contains(REFUSED_PENDING_UNOPENED)
                || text.contains(REFUSED_PENDING_SPELLINGS)
                || text == REFUSED_HOMELESS
                || text.contains(REFUSED_UNCONNECTED_TEAM) =>
        {
            ApiError::conflict(text.clone())
        }
        // The anonymous visitor of a public domain reads and never comments:
        // logging in is what fixes it, so 401 like every other anonymous write.
        EngineError::Forbidden(text) if text == ANONYMOUS_COMMENT => {
            ApiError::unauthorized(text.clone())
        }
        // A create-only upload onto a path that holds a file: the request's
        // own precondition failed.
        EngineError::Conflict(text) if text.ends_with(ASSET_EXISTS) => {
            ApiError::precondition_failed_plain(text.clone())
        }
        EngineError::Conflict(text)
            if text.starts_with("stale edit")
                || text.starts_with("a reflection already lives at")
                || text.starts_with("somebody is editing")
                || text.ends_with(MOVE_ONTO_A_FILE)
                || text.ends_with(TARGET_TAKEN) =>
        {
            ApiError::conflict(text.clone())
        }
        _ => ApiError::from(e),
    }
}

/// `GET /reflection/domains` - the domains whose Reflection this caller
/// reaches, including the ones it cannot see as engrams.
#[utoipa::path(
    get,
    path = "/api/v1/reflection/domains",
    tag = "reflection",
    operation_id = "list_reflection_domains",
    summary = "The domains whose Reflection this caller reaches.",
    description = "Every registered domain whose Reflection is not off and \
                   where the caller's reflection right is at least `read`, \
                   including domains the caller cannot see as engrams \
                   (knowledge `none` with a Reflection level, and \
                   Reflection collaborators, who hold a per-domain \
                   Reflection grant): this is how \
                   Fluid finds them, since `GET /domains` leaves them out. \
                   The anonymous viewer gets the public domains, at `read`. \
                   Sorted by name.",
    responses(
        (
            status = 200,
            description = "ReflectionDomains: `{ domains: [{ domain, state, right, \
                           engrams_visible, home, home_url, host_label, \
                           open_threads }] }`. `state` is `on`, `pending`, \
                           `homeless`, `elsewhere` or `hosted_elsewhere` \
                           (another instance on this shared database holds \
                           the domain's host lock and serves it); `home` is \
                           `here`, `local`, `elsewhere` (then `home_url` is \
                           set) or `null` (`pending`, `homeless`, \
                           `hosted_elsewhere`); `host_label` names the serving \
                           instance for `hosted_elsewhere` (`null` while a \
                           rename pauses a domain nobody holds) and is `null` \
                           otherwise; `open_threads` is 0 unless `state` is \
                           `on`.",
            body = Object,
            example = json!({ "domains": [ { "domain": "eng", "state": "on",
                "right": "edit", "engrams_visible": false, "home": "here",
                "home_url": null, "host_label": null, "open_threads": 3 } ] }),
        ),
    ),
)]
pub async fn domains(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<Value>, ApiError> {
    let caller = caller(&identity);
    let mut out = Vec::new();
    let mut names: Vec<String> = state.engine.registered_domain_names().into_iter().collect();
    names.sort();
    for name in names {
        // A domain this caller may not see is left out; any other failure is
        // the store's and is answered, not hidden.
        let status = match state.engine.reflection_status(&name, &caller).await {
            Ok(status) => status,
            Err(EngineError::UnknownDomain { .. } | EngineError::NotFound(_)) => continue,
            Err(e) => return Err(reflection_error(e)),
        };
        if status["state"] == "off" || status["right"] == "none" {
            continue;
        }
        let open = if status["state"] == "on" {
            state
                .engine
                .reflection_open_threads(&name, &caller)
                .await
                .unwrap_or(0)
        } else {
            0
        };
        out.push(json!({
            "domain": name,
            "state": status["state"],
            "right": status["right"],
            "engrams_visible": status["engrams_visible"],
            "home": status["home"],
            "home_url": status["home_url"],
            "host_label": status["host_label"],
            "open_threads": open,
        }));
    }
    Ok(Json(json!({ "domains": out })))
}

/// `GET /domains/{domain}/reflection` - the Reflection's status for this
/// caller; never a 409, since this is how Fluid learns off, pending and
/// elsewhere.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection",
    tag = "reflection",
    operation_id = "get_reflection_status",
    summary = "The Reflection's status for this caller.",
    params(("domain" = String, Path, description = "The domain.")),
    responses(
        (
            status = 200,
            description = "ReflectionStatus. `state` is `off`, `pending` (then \
                           `pending_engrams` lists the engrams still under \
                           `reflections/` for a caller with `engrams_visible`, \
                           and is empty for anyone else), `homeless`, `on`, \
                           `elsewhere` (then `home_url` is set) or \
                           `hosted_elsewhere` (another instance on this shared \
                           database holds the domain's host lock and serves \
                           it; then `host_label` names that instance, or is \
                           `null` while a rename pauses a domain whose lock \
                           nobody holds, and it is `null` in every other \
                           state). `member` is true \
                           when the caller holds a membership row it may \
                           leave; `collaborator` is true when it holds a \
                           per-domain Reflection grant it may give up \
                           (`DELETE /domains/{domain}/reflection-collaborators/{account}`).",
            body = Object,
            example = json!({ "domain": "eng", "enabled": true, "state": "on",
                "home": "here", "home_url": null, "host_label": null, "right": "curate",
                "engrams_visible": true, "member": false, "collaborator": false,
                "pending_engrams": [] }),
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn status(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let mut answer = state
        .engine
        .reflection_status(&domain, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    // Whether the caller holds a membership row it may leave (Fluid G5). Read
    // here, not in the engine: it is an accounts question, and only this
    // surface offers leaving.
    let member = match &identity.user {
        Some(user) => state
            .auth
            .domain_members(&domain)
            .await
            .map_err(|e| ApiError::internal(format!("listing this domain's members: {e:#}")))?
            .iter()
            .any(|m| m.principal == user.name),
        None => false,
    };
    answer["member"] = Value::Bool(member);
    // Whether the caller holds a Reflection grant here it may give up.
    let collaborator = match &identity.user {
        Some(user) => state
            .auth
            .reflection_grant(&domain, &user.name)
            .await
            .map_err(|e| {
                ApiError::internal(format!(
                    "reading this domain's Reflection collaborators: {e:#}"
                ))
            })?
            .is_some(),
        None => false,
    };
    answer["collaborator"] = Value::Bool(collaborator);
    Ok(Json(answer))
}

/// `GET /domains/{domain}/reflection/tree`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/tree",
    tag = "reflection",
    operation_id = "get_reflection_tree",
    summary = "The Reflection's folders and reflections.",
    params(("domain" = String, Path, description = "The domain.")),
    responses(
        (
            status = 200,
            description = "ReflectionTree: folders first, then reflections, each \
                           level sorted by name in byte order. `PACT.md` is only \
                           under `pact`, never in `nodes`.",
            body = Object,
            example = json!({ "domain": "eng",
                "pact": { "path": "PACT.md", "title": "How we write about eng",
                          "exists": true, "open_threads": 0 },
                "nodes": [ { "kind": "folder", "name": "guides", "path": "guides",
                    "children": [ { "kind": "reflection", "name": "deploying.md",
                        "path": "guides/deploying.md", "title": "Deploying",
                        "open_threads": 2, "sections": 4 } ] } ] }),
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is off, pending, homeless, lives \
                           elsewhere (then the `home_url` member names where) \
                           or is served by another instance on this shared \
                           database (the text names it).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn tree(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    state
        .engine
        .read_reflection_tree(&domain, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// `GET /domains/{domain}/reflections/{*path}`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflections/{path}",
    tag = "reflection",
    operation_id = "get_reflection",
    summary = "One reflection, or PACT.md.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("path" = String, Path, description = "The reflection's path inside \
                                               reflections/; may contain slashes."),
    ),
    responses(
        (
            status = 200,
            description = "ReflectionDoc. `markdown` is the whole file for a \
                           caller with `engrams_visible`, less the sources in \
                           its `reflection:` block that name a domain the caller \
                           may not open, and the file without that block for \
                           anyone else; `sections` (and their one-based `line`) \
                           are computed over the `markdown` answered; \
                           `checksum` is always the stored file's. \
                           `sections[].sources` is absent, not empty, without \
                           `engrams_visible`. A source's `state` is `current`, \
                           `changed`, `retired`, `missing` or `unavailable` \
                           (its domain cannot be checked here); a source of \
                           another domain than the page's carries `domain`, \
                           the local name it is reached by here. \
                           `sections[].hidden_sources` counts the sources in a \
                           domain the caller may not open, which are never \
                           named; absent when there are none. `threads` holds \
                           the open threads; `live` is present while a \
                           co-editing room is open.",
            body = Object,
            example = json!({ "domain": "eng", "path": "guides/deploying.md",
                "title": "Deploying", "markdown": "# Deploying\n...",
                "checksum": "9f2c...", "sections": [ { "id": "7f3a9c2e",
                    "heading": "Night deploys", "line": 12, "changed": true,
                    "sources": [ { "permalink": "runbooks/deploy",
                        "title": "Deploy runbook", "state": "current" },
                        { "permalink": "team/on-call", "domain": "ops",
                        "title": "On call", "state": "changed" } ],
                    "hidden_sources": 1 } ],
                "threads": [], "live": { "present": ["jordi"] } }),
        ),
        (
            status = 404,
            description = "No such domain or reflection, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (off, pending, homeless, \
                           elsewhere, then with `home_url`, or served by \
                           another instance on this shared database).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn read(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, path)): ApiPath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    state
        .engine
        .read_reflection(&domain, &path, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// What `PUT /domains/{domain}/reflections/{*path}` takes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SaveReflectionBody {
    /// The whole reflection. A body without the `<!-- r:... -->` id lines
    /// keeps them: a heading that matches a stored section gets its id back.
    /// Below curate, the stored `reflection:` block stays whatever this
    /// carries.
    pub markdown: String,
    /// The checksum `GET` answered; a reflection that changed since is 409.
    #[serde(default)]
    pub expected_checksum: Option<String>,
    /// Only create: refuse with 409 when a reflection already lives at this
    /// path, and leave it untouched. Default false: create or replace.
    #[serde(default)]
    pub create: bool,
}

/// `PUT /domains/{domain}/reflections/{*path}`
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/reflections/{path}",
    tag = "reflection",
    operation_id = "save_reflection",
    summary = "Create or replace a reflection, or PACT.md.",
    description = "Needs `edit`; `PACT.md` needs `curate`. While a co-editing \
                   room is open over the reflection the write composes into \
                   the room, and the answer carries `landed: live` and \
                   `present`.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("path" = String, Path, description = "The reflection's path inside \
                                               reflections/; may contain slashes."),
    ),
    request_body = SaveReflectionBody,
    responses(
        (
            status = 200,
            description = "Written: `{ domain, path, checksum, created, \
                           ids_assigned, curated }`. `curated` says whether \
                           this write advanced the baseline. While a \
                           co-editing room is open over the page the receipt \
                           also carries `present` (who is in it) and \
                           `landed`: `live` when the room took the write in, \
                           `file` when it is on disk only, and then `room` \
                           (`conflict` or `waiting`) with `room_note`, a \
                           sentence saying what the people in the room meet.",
            body = Object,
            example = json!({ "domain": "eng", "path": "guides/deploying.md",
                "checksum": "9f2c...", "created": false,
                "ids_assigned": ["7f3a9c2e"], "curated": false }),
        ),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: it reads \
                           the Reflection and never changes it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below edit access (curate for PACT.md), with a detail \
                           naming the right held and the right needed; the \
                           domain is off the caller's write list; or this \
                           instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "A stale `expected_checksum` (detail starting \"stale \
                           edit\"), a create over an existing page, a page \
                           only the previous host of a host move has (the \
                           detail names the remedy), or the Reflection is not \
                           here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A bad path or body.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn save(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, path)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<SaveReflectionBody>,
) -> Result<Json<Value>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    let p = WriteReflectionParams {
        domain,
        path,
        markdown: Some(body.markdown),
        expected_checksum: body.expected_checksum,
        create_only: body.create,
        ..Default::default()
    };
    state
        .engine
        .write_reflection(&p, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// `DELETE /domains/{domain}/reflections/{*path}`
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}/reflections/{path}",
    tag = "reflection",
    operation_id = "delete_reflection",
    summary = "Delete a reflection with its threads and baseline.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("path" = String, Path, description = "The reflection's path inside \
                                               reflections/; may contain slashes."),
    ),
    responses(
        (status = 204, description = "Deleted with its threads and baseline."),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: it reads \
                           the Reflection and never changes it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below edit access, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "Somebody is editing it, or the Reflection is not here.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A bad path, or `PACT.md`, which is edited and never deleted.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, path)): ApiPath<(String, String)>,
) -> Result<StatusCode, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    state
        .engine
        .delete_reflection(&domain, &path, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// What `POST /domains/{domain}/reflection/move` takes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MoveReflectionBody {
    /// The reflection's current path inside reflections/.
    pub from: String,
    /// Its new path inside reflections/.
    pub to: String,
}

/// `POST /domains/{domain}/reflection/move`
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/reflection/move",
    tag = "reflection",
    operation_id = "move_reflection",
    summary = "Move a reflection with its threads and baseline.",
    params(("domain" = String, Path, description = "The domain.")),
    request_body = MoveReflectionBody,
    responses(
        (
            status = 200,
            description = "Moved with its threads and baseline; links to it \
                           rewritten: `{ from, to, links_rewritten, threads_moved }`. \
                           `links_failed`, `[{ domain, path, reason }]`, appears only \
                           when a page that links it could not be rewritten \
                           after the move.",
            body = Object,
            example = json!({ "from": "guides/a.md", "to": "howto/a.md",
                "links_rewritten": 2, "threads_moved": true }),
        ),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: it reads \
                           the Reflection and never changes it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below edit access, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "Somebody is editing a page that links it (an \
                           editor open on the page itself does not refuse: \
                           it saves, closes and is told the new path), the \
                           new path is taken or is a page only the previous \
                           host of a host move has, or the Reflection is not \
                           here.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A bad path, or `PACT.md`, which is edited and never moved.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn move_to(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<MoveReflectionBody>,
) -> Result<Json<Value>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    state
        .engine
        .move_reflection(&domain, &body.from, &body.to, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// What `POST /domains/{domain}/reflection/disclose` takes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DiscloseReflectionBody {
    /// The reflection's path inside reflections/.
    pub path: String,
    /// The id of the section a `source_too_wide` item names.
    pub section: String,
    /// The item's `basis`: what the person's yes is bound to. When the
    /// page's readers or its repository changed since the item was read, the
    /// confirmation is refused with 409 and the person is asked again.
    pub basis: String,
}

/// `POST /domains/{domain}/reflection/disclose`
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/reflection/disclose",
    tag = "reflection",
    operation_id = "disclose_reflection_section",
    summary = "Share a section's text from a domain fewer people may read, on purpose.",
    description = "A person's answer to the `question` of a `source_too_wide` \
                   item: every source of the section in another domain that \
                   the page may not carry now is shared with the page's \
                   readers of this moment (and the repository the page is \
                   backed up to). The confirmation is kept on the server with \
                   who gave it and when, never in the page. The queue names \
                   the section again when the page's readers widen. Needs \
                   `curate` and the right to read every source's domain; only \
                   a signed-in person can confirm.",
    params(("domain" = String, Path, description = "The domain.")),
    request_body = DiscloseReflectionBody,
    responses(
        (
            status = 200,
            description = "Confirmed: `{ domain, path, section, disclosed }`, \
                           `disclosed` `{ by, at }`, or null when nothing in \
                           the section needed a confirmation.",
            body = Object,
            example = json!({ "domain": "eng", "path": "overview.md",
                "section": "7f3a9c2e",
                "disclosed": { "by": "keeper", "at": "2026-10-28T09:00:00Z" } }),
        ),
        (
            status = 401,
            description = "The anonymous viewer: nobody is behind it to confirm.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below curate access, a source in a domain this \
                           caller may not read (not named), or this instance \
                           is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, page or section with sources.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "Who may read the page, or where it is backed up, \
                           changed since the item was read: read the queue \
                           again and ask the person the new question.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A bad path, or nobody here knows who reads which \
                           domain because the accounts store could not be read.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn disclose(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<DiscloseReflectionBody>,
) -> Result<Json<Value>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    // Every caller of this route is a person: the JSON API is reached with a
    // browser session (or a proxy that signed a person in), never with an
    // agent's token, so a confirmation here is that person's.
    state
        .engine
        .confirm_reflection_disclosure(
            &domain,
            &body.path,
            &body.section,
            &body.basis,
            &caller(&identity),
        )
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// The query of `GET /domains/{domain}/reflection/threads`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ThreadsQuery {
    /// open (default), resolved or all.
    #[serde(default)]
    status: Option<String>,
    /// Only this reflection's threads.
    #[serde(default)]
    path: Option<String>,
}

/// `GET /domains/{domain}/reflection/threads`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/threads",
    tag = "reflection",
    operation_id = "list_reflection_threads",
    summary = "The Reflection's threads, oldest first.",
    params(("domain" = String, Path, description = "The domain."), ThreadsQuery),
    responses(
        (
            status = 200,
            description = "`{ threads: [ThreadView] }`, oldest first. A thread's \
                           `anchor.section` is the section's id, or its heading \
                           text for a heading with no id yet; `anchor.state` is \
                           `found`, `text_changed` or `section_gone`; \
                           `waiting_on` is `agent` when the last entry is a \
                           person's and `people` when it is an agent's; \
                           `age_secs` counts from the last entry.",
            body = Object,
            example = json!({ "threads": [ { "id": "t-3f9a2c1b",
                "path": "guides/deploying.md",
                "anchor": { "section": "7f3a9c2e", "quote": "Deploys run at night",
                            "occurrence": 1, "state": "found" },
                "status": "open", "waiting_on": "agent",
                "opened_at": "2026-09-30T10:00:00Z", "age_secs": 3600,
                "entries": [ { "at": "2026-09-30T10:00:00Z", "kind": "comment",
                    "author": { "name": "jordi", "agent": false },
                    "text": "Still at night?" } ] } ] }),
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn threads(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(q): ApiQuery<ThreadsQuery>,
) -> Result<Json<Value>, ApiError> {
    state
        .engine
        .list_reflection_threads(
            &domain,
            q.path.as_deref(),
            q.status.as_deref().unwrap_or("open"),
            &caller(&identity),
        )
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// What `POST /domains/{domain}/reflection/threads` takes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct OpenThreadBody {
    /// The reflection, its path inside reflections/.
    pub path: String,
    /// The section the thread is about: its id, or its heading text.
    pub section: String,
    /// The exact passage, when the thread is about one.
    #[serde(default)]
    pub quote: Option<String>,
    /// Which occurrence of the quote, from 1.
    #[serde(default)]
    pub occurrence: Option<u32>,
    /// The first entry.
    pub text: String,
}

/// `POST /domains/{domain}/reflection/threads`
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/reflection/threads",
    tag = "reflection",
    operation_id = "open_reflection_thread",
    summary = "Open a thread on a reflection's section.",
    description = "Reading and commenting are one level: `read` is enough. \
                   The anonymous viewer reads threads and never writes one.",
    params(("domain" = String, Path, description = "The domain.")),
    request_body = OpenThreadBody,
    responses(
        (
            status = 201,
            description = "The new thread as a ThreadView; its `anchor.section` \
                           is the section's id, or its heading text for a \
                           heading with no id yet.",
            body = Object,
            example = json!({ "id": "t-3f9a2c1b", "path": "guides/deploying.md",
                "anchor": { "section": "7f3a9c2e", "quote": "at night",
                            "occurrence": 1, "state": "found" },
                "status": "open", "waiting_on": "agent",
                "opened_at": "2026-09-30T10:00:00Z", "age_secs": 0,
                "entries": [ { "at": "2026-09-30T10:00:00Z", "kind": "comment",
                    "author": { "name": "jordi", "agent": false },
                    "text": "Still at night?" } ] }),
        ),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: a comment \
                           needs somebody to sign it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "This instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "An unknown section, a quote the section does not hold, \
                           or an empty text.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn open(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<OpenThreadBody>,
) -> Result<Response, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_COMMENT).await?;
    refuse_read_only(&state)?;
    let p = DiscussParams {
        domain,
        action: Some("open".into()),
        path: Some(body.path),
        section: Some(body.section),
        quote: body.quote,
        occurrence: body.occurrence,
        text: Some(body.text),
        ..Default::default()
    };
    let view = state
        .engine
        .discuss(&p, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

/// What the three thread actions take.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ThreadStepBody {
    /// The reflection the thread is on.
    pub path: String,
    /// answer: the text, required; reopen: optional.
    #[serde(default)]
    pub text: Option<String>,
    /// resolve: the short outcome, required.
    #[serde(default)]
    pub outcome: Option<String>,
}

/// `POST /domains/{domain}/reflection/threads/{id}/{action}` for answer,
/// resolve and reopen: one handler, the action from the path.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/reflection/threads/{id}/{action}",
    tag = "reflection",
    operation_id = "step_reflection_thread",
    summary = "Answer, resolve or reopen a thread.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("id" = String, Path, description = "The thread id (t-...)."),
        ("action" = String, Path, description = "answer, resolve or reopen; any \
                                                 other answers 404."),
    ),
    request_body = ThreadStepBody,
    responses(
        (
            status = 200,
            description = "The thread as a ThreadView after the step; its \
                           `anchor.section` is the section's id, or its heading \
                           text for a heading with no id yet.",
            body = Object,
            example = json!({ "id": "t-3f9a2c1b", "path": "guides/deploying.md",
                "anchor": { "section": "7f3a9c2e", "quote": "at night",
                            "occurrence": 1, "state": "found" },
                "status": "resolved", "waiting_on": "agent",
                "opened_at": "2026-09-30T10:00:00Z", "age_secs": 0,
                "entries": [ { "at": "2026-09-30T10:00:00Z", "kind": "comment",
                    "author": { "name": "jordi", "agent": false },
                    "text": "Still at night?" },
                    { "at": "2026-09-30T11:00:00Z", "kind": "resolve",
                    "author": { "name": "vera", "agent": false },
                    "text": "Yes." } ] }),
        ),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: an entry \
                           needs somebody to sign it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "This instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such thread, or an action other than answer, \
                           resolve or reopen.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "An answer with no text, or a resolve with no outcome \
                           (an empty one included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn step(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, id, action)): ApiPath<(String, String, String)>,
    ApiJson(body): ApiJson<ThreadStepBody>,
) -> Result<Json<Value>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_COMMENT).await?;
    refuse_read_only(&state)?;
    use crystalline_core::threads::EntryKind;
    let (kind, text) = match action.as_str() {
        "answer" => (EntryKind::Comment, body.text.unwrap_or_default()),
        "resolve" => (
            EntryKind::Resolve,
            body.outcome.or(body.text).unwrap_or_default(),
        ),
        "reopen" => (EntryKind::Reopen, body.text.unwrap_or_default()),
        other => {
            return Err(ApiError::not_found(format!(
                "no thread action '{other}'; use answer, resolve or reopen"
            )));
        }
    };
    state
        .engine
        .reflection_thread_entry(&domain, &body.path, &id, kind, &text, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// The query of the queue route.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct QueueQuery {
    /// Page size, default 10, at most 100.
    #[serde(default)]
    limit: Option<usize>,
    /// One-based page.
    #[serde(default)]
    page: Option<usize>,
}

/// `GET /domains/{domain}/reflection/queue` - read-only: never records a run.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/queue",
    tag = "reflection",
    operation_id = "get_reflect_queue",
    summary = "What the domain's Reflection needs next. Read-only.",
    description = "The same detection the MCP `reflect` tool runs, and nothing \
                   recorded: opening the review screen never counts as a run.",
    params(("domain" = String, Path, description = "The domain."), QueueQuery),
    responses(
        (
            status = 200,
            description = "ReflectQueue. `kind` is `feedback` (`what`: `new`, \
                           `changed`, `removed`), `update` (`source_changed`, \
                           `source_retired`, `source_missing`, `unreflected`, \
                           `off_structure`, `pact_changed`) or `review` \
                           (`thread`, with `thread`, the thread's id, and, \
                           when its opening comment is a conventional \
                           comment, `label` and `blocking`; a blocking thread \
                           comes first among the threads of its section). \
                           `sources`, an `unreflected` item's \
                           `folder` and `engrams`, and an `engram_attachment` \
                           item's `targets` are absent for a caller who \
                           cannot read engrams. The MCP `reflect` tool answers \
                           the same shape with `scope: { domains, kinds }` in \
                           place of `domain` and a `domain` member on each row; \
                           its `scope` may carry `unreadable`, the names of the \
                           domains whose queue could not be read, and \
                           `pending`, the names of the domains whose \
                           Reflection waits for engrams to leave \
                           `reflections/`. An update item's `what` may also \
                           be `asset_missing` (per section, with `assets`), \
                           `engram_attachment` (per section, with `targets`), \
                           `asset_unreferenced` (`path` empty, with \
                           `asset`) or `host_moved` (`path` empty, one per \
                           domain, after the domain moved to this instance \
                           on a shared database and this disk lacks files \
                           the previous host wrote: `only_elsewhere` is \
                           `{ previous_host, engrams, attachments, pages, \
                           files }`, counts only, `previous_host` null when \
                           unknown), `source_unavailable` (one per page and \
                           domain that cannot be checked here, `section` null, \
                           with `source_domain`, `count`, `sections` and \
                           `note`) or `source_too_wide` (one per section that \
                           cites a domain fewer people may read than the \
                           page's domain, with `sources`, `note`, \
                           `question`, what a person is asked before they \
                           share it on purpose through \
                           `POST /domains/{domain}/reflection/disclose`, and \
                           `basis`, what that call carries back; only \
                           sources the caller may open count). A source in `sources` may carry `domain`, \
                           the local name of another domain than the page's. \
                           Sources in a domain the caller may not open are \
                           never named. A feedback item and an update item \
                           then carry `hidden_sources`, their count, and \
                           `note`, one plain sentence whatever their state. \
                           A feedback item keeps its own `what`; for update \
                           items such sources are always on the section's \
                           `source_changed` item, aged by its page. An \
                           `off_structure` item carries `structure`, the \
                           pact's globs, and `hint` when none of them fits \
                           any page; a `pact_changed` item carries `diff` and \
                           `sections`, the ids of the changed parts of the \
                           pact.",
            body = Object,
            example = json!({ "domain": "eng", "total": 1, "page": 1, "limit": 10,
                "count": 1, "kinds": { "feedback": 0, "update": 1, "review": 0 },
                "queue": [ { "n": 1, "kind": "update", "what": "source_changed",
                    "path": "guides/deploying.md", "section": "7f3a9c2e",
                    "heading": "Night deploys", "age_secs": 3600,
                    "sources": [ { "permalink": "runbooks/deploy",
                                   "state": "changed" } ] } ],
                "guidance": "..." }),
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn queue(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(q): ApiQuery<QueueQuery>,
) -> Result<Json<Value>, ApiError> {
    state
        .engine
        .reflect_queue(&domain, q.limit, q.page, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// `GET /domains/{domain}/reflection/review`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/review",
    tag = "reflection",
    operation_id = "get_reflection_review",
    summary = "The review screen: open threads by who they wait on, and the queue.",
    params(("domain" = String, Path, description = "The domain.")),
    responses(
        (
            status = 200,
            description = "ReflectionReview: open threads only, grouped by \
                           reflection, groups and threads oldest first; \
                           `queue` is the ReflectQueue's page 1 at `limit` 50.",
            body = Object,
            example = json!({ "domain": "eng",
                "waiting_for_people": [],
                "waiting_for_the_agent": [ { "path": "guides/deploying.md",
                    "title": "Deploying", "threads": [] } ],
                "queue": { "domain": "eng", "total": 0, "page": 1, "limit": 50,
                    "count": 0, "kinds": { "feedback": 0, "update": 0, "review": 0 },
                    "queue": [], "guidance": "..." } }),
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn review(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    state
        .engine
        .reflection_review(&domain, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// `GET /domains/{domain}/reflection/summary`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/summary",
    tag = "reflection",
    operation_id = "get_reflection_summary",
    summary = "The Reflection home: what waits for review, in counts and by page.",
    description = "The cheap cut of the review the Reflection home draws: \
                   how many open threads wait for the agent and for people, \
                   `updates` (the review queue's `kinds.update`, counted \
                   without rendering a diff or looking up a source), \
                   `pages_to_update` (the distinct pages those updates are \
                   on; an update of no page, such as unreflected engrams or \
                   a loose file, is in `updates` only) and the open threads \
                   by page. A thread carries its id, its \
                   first quote (`null` for a whole section), who it waits on, \
                   when it was opened and the people who wrote in it, never \
                   its entries. Pages and threads come oldest first. The same \
                   rights as the review: a reader without engram rights is \
                   not counted any update about engrams that no page cites.",
    params(("domain" = String, Path, description = "The domain.")),
    responses(
        (
            status = 200,
            description = "ReflectionSummary.",
            body = Object,
            example = json!({ "domain": "eng",
                "waiting_for_the_agent": 1,
                "waiting_for_people": 0,
                "updates": 3,
                "pages_to_update": 2,
                "open_by_page": [ { "path": "guides/deploying.md",
                    "title": "Deploying",
                    "threads": [ { "id": "t-0a1b2c3d",
                        "quote": "at night",
                        "waiting_on": "agent",
                        "opened_at": "2026-10-14T09:12:00Z",
                        "people": [ { "name": "ann", "agent": false } ] } ] } ] }),
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn summary(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    state
        .engine
        .reflection_summary(&domain, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

// A schema type only, never built: the route reads `search_type` as a
// string, so an unknown value answers the engine's 422 (which names the
// three) instead of the query extractor's 400. No doc comment: one would
// replace the parameter's own description in the generated client.
#[expect(dead_code, reason = "a schema type only")]
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReflectionSearchType {
    Hybrid,
    Text,
    Semantic,
}

/// The query of the search route.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// The words to find. Required; a missing or empty `q` answers 422.
    #[serde(default)]
    q: Option<String>,
    /// hybrid (default), text or semantic. hybrid and semantic answer as
    /// text while the pages are not embedded yet; the answer's mode says
    /// which ran.
    // Read as a string so an unknown value reaches the engine and its 422
    // names the three; the schema lists them.
    #[serde(default)]
    #[param(inline, value_type = Option<ReflectionSearchType>)]
    search_type: Option<String>,
    /// At most this many hits, default 20, at most 100.
    #[serde(default)]
    limit: Option<usize>,
    /// One-based page, default 1.
    #[serde(default)]
    page: Option<usize>,
}

/// `GET /domains/{domain}/reflection/search`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/search",
    tag = "reflection",
    operation_id = "search_reflections",
    summary = "Search the domain's reflections by words, by meaning or both.",
    description = "A hit is one section. A reflection's `reflection:` \
                   frontmatter block is taken off before it is searched, for \
                   every caller, so no hit or snippet comes out of it.",
    params(("domain" = String, Path, description = "The domain."), SearchQuery),
    responses(
        (
            status = 200,
            description = "`{ domain, query, mode, total, page, limit, count, \
                           hits: [{ path, title, section, heading, snippet }] }`, \
                           best first. `mode` is the search that ran (hybrid, \
                           text or semantic), not a score; no hit carries one. \
                           `total` counts every hit, `count` this page's. \
                           `section` is the section id, `_top` for the text \
                           before the first heading, or `null` for a heading \
                           with no id yet.",
            body = Object,
            example = json!({ "domain": "eng", "query": "night", "mode": "hybrid",
                "total": 1, "page": 1, "limit": 20, "count": 1, "hits": [ {
                "path": "guides/deploying.md", "title": "Deploying",
                "section": "7f3a9c2e", "heading": "Night deploys",
                "snippet": "Deploys run at night." } ] }),
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A missing or empty `q`, or a `search_type` other \
                           than hybrid, text or semantic.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn search(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(q): ApiQuery<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let Some(words) = q.q.as_deref().map(str::trim).filter(|w| !w.is_empty()) else {
        return Err(ApiError::unprocessable(
            "search needs `q`, the words to find",
        ));
    };
    state
        .engine
        .search_reflection(
            &domain,
            &ReflectionSearch {
                query: words,
                search_type: q.search_type.as_deref(),
                limit: q.limit.unwrap_or(20).clamp(1, 100),
                page: q.page.unwrap_or(1),
            },
            &caller(&identity),
        )
        .await
        .map(Json)
        .map_err(reflection_error)
}

/// The path inside `reflections/` a file route's wildcard names.
fn asset_path(rel: &str) -> String {
    format!("{}{rel}", crystalline_core::ASSETS_PREFIX)
}

/// Every file of a domain's Reflection.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReflectionAssetsResponse {
    /// The rows, ordered by path; `path` is inside `reflections/`
    /// (`assets/...`).
    pub assets: Vec<AttachmentView>,
}

/// What an upload into a Reflection answers.
#[derive(Debug, Serialize, ToSchema)]
pub struct UploadedReflectionAsset {
    /// The path inside `reflections/`, `assets/...`: how a reflection in the
    /// folder root links it.
    #[schema(example = "assets/diagrams/flow.png")]
    pub path: String,
    /// The mime it is served under, from the extension.
    #[schema(example = "image/png")]
    pub mime: String,
    /// Byte length as stored.
    #[schema(example = 20481)]
    pub size: u64,
    /// Lowercase hex SHA-256 of the stored bytes, the token the read's
    /// `ETag` carries.
    #[schema(example = "9f2a1c05e2b7")]
    pub sha256: String,
    /// Whether the path held no file before.
    pub created: bool,
}

/// What `POST /domains/{domain}/reflection/move-asset` takes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MoveReflectionAssetBody {
    /// The file's path inside `reflections/`, `assets/...`.
    #[schema(example = "assets/diagrams/flow.png")]
    pub from: String,
    /// Its new path inside `reflections/`, `assets/...`.
    #[schema(example = "assets/flow.png")]
    pub to: String,
}

/// `GET /domains/{domain}/reflection/assets`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/assets",
    tag = "reflection",
    operation_id = "list_reflection_assets",
    summary = "Every file of the Reflection, metadata only.",
    description = "Needs `read` on the Reflection; knowledge rights play no \
                   part, so a person at knowledge `none` with a Reflection \
                   level lists every file.",
    params(("domain" = String, Path, description = "The domain.")),
    responses(
        (
            status = 200,
            description = "Every file under reflections/assets/, ordered by \
                           path; each `path` is inside `reflections/`.",
            body = ReflectionAssetsResponse,
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (off, pending, homeless, \
                           elsewhere, then with `home_url`, or served by \
                           another instance on this shared database).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn assets(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<ReflectionAssetsResponse>, ApiError> {
    let rows = state
        .engine
        .reflection_asset_list(&domain, &caller(&identity))
        .await
        .map_err(reflection_error)?;
    Ok(Json(ReflectionAssetsResponse {
        assets: rows.into_iter().map(AttachmentView::from).collect(),
    }))
}

/// `GET /domains/{domain}/reflection/assets/{*path}`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/reflection/assets/{path}",
    tag = "reflection",
    operation_id = "read_reflection_asset",
    summary = "One file of the Reflection, its bytes.",
    description = "Needs `read` on the Reflection; knowledge rights play no \
                   part. Served like an engram attachment, with the same \
                   headers.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("path" = String, Path, description = "The path under \
                                               reflections/assets/; may contain slashes.",
         example = "diagrams/flow.png"),
    ),
    responses(
        (
            status = 200,
            description = "The bytes, under the mime the allowlist assigns.",
            body = String,
            content_type = "application/octet-stream",
            headers(
                ("etag" = String, description = "The strong quoted SHA-256 of the bytes served."),
                ("cache-control" = String, description = "Always `no-cache`: store it, but revalidate before every use."),
                ("x-content-type-options" = String, description = "Always `nosniff`."),
                ("content-security-policy" = String, description = "Always `default-src 'none'; sandbox`."),
                ("content-disposition" = String, description = "`inline` for images, PDFs and text; `attachment; filename=\"...\"` otherwise."),
            ),
        ),
        (
            status = 304,
            description = "The `If-None-Match` token matches the stored bytes; \
                           no body, the `ETag` and the same `Cache-Control`.",
        ),
        (
            status = 404,
            description = "No such file, or no such domain for this caller (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A path rule or the extension allowlist refused the path.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn asset_read(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
    ApiPath((domain, rel)): ApiPath<(String, String)>,
) -> Result<Response, ApiError> {
    let (bytes, row) = state
        .engine
        .reflection_asset_read(&domain, &asset_path(&rel), &caller(&identity))
        .await
        .map_err(reflection_error)?;
    Ok(attachment_response(&headers, bytes, &row))
}

/// `PUT /domains/{domain}/reflection/assets/{*path}`
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/reflection/assets/{path}",
    tag = "reflection",
    operation_id = "write_reflection_asset",
    summary = "Add a file to the Reflection, or replace one.",
    description = "Needs `edit` for a new path and `curate` to replace a \
                   file, since a replace removes the old bytes. The request \
                   body is the raw file, not multipart; the extension \
                   allowlist decides the mime. The path rules, the allowlist \
                   and the 10 MiB ceiling are the engram attachments' own. \
                   With `If-None-Match: *` the upload only creates: a path \
                   that already holds a file is refused with 412, for a \
                   curator too, so two people cannot replace each other's \
                   upload by accident.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("path" = String, Path, description = "Where to store it, under \
                                               reflections/assets/; may contain slashes.",
         example = "diagrams/flow.png"),
        (
            "If-None-Match" = Option<String>,
            Header,
            description = "`*` to only create: a path that already holds a \
                           file is refused with 412 and left as it is. Any \
                           other value is 422.",
        ),
    ),
    request_body(
        description = "The raw file bytes.",
        content_type = "application/octet-stream",
        content = String,
    ),
    responses(
        (
            status = 200,
            description = "Stored. `path` is inside `reflections/`.",
            body = UploadedReflectionAsset,
        ),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: it reads \
                           the Reflection and never changes it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below edit access; a replace below curate access, \
                           with a detail naming the right held and curate \
                           (upload it under a new name instead); the domain \
                           is off the caller's write list; or this instance \
                           is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The path is a file only the previous host of a \
                           host move has, or the Reflection is not here (then \
                           possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 412,
            description = "`If-None-Match: *` and the path already holds a \
                           file; nothing was stored. Pick another name.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 413,
            description = "Over the 10 MiB ceiling.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A path rule or the extension allowlist refused it, \
                           or `If-None-Match` carried something other than `*`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn asset_write(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
    ApiPath((domain, rel)): ApiPath<(String, String)>,
    // Last: the only extractor here that consumes the body.
    body: Bytes,
) -> Result<Json<UploadedReflectionAsset>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    let create_only = super::create_only(&headers)?;
    let (path, bytes, who) = (asset_path(&rel), body.to_vec(), caller(&identity));
    let written = if create_only {
        state
            .engine
            .reflection_asset_create(&domain, &path, bytes, &who)
            .await
    } else {
        state
            .engine
            .reflection_asset_write(&domain, &path, bytes, &who)
            .await
    }
    .map_err(reflection_error)?;
    Ok(Json(UploadedReflectionAsset {
        path: written.row.path,
        mime: written.row.mime,
        size: written.row.size,
        sha256: written.row.sha256,
        created: written.created,
    }))
}

/// `DELETE /domains/{domain}/reflection/assets/{*path}`
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}/reflection/assets/{path}",
    tag = "reflection",
    operation_id = "delete_reflection_asset",
    summary = "Remove a file from the Reflection.",
    params(
        ("domain" = String, Path, description = "The domain."),
        ("path" = String, Path, description = "The path under \
                                               reflections/assets/; may contain slashes.",
         example = "diagrams/flow.png"),
    ),
    responses(
        (
            status = 204,
            description = "Removed. A reflection that still links it keeps the \
                           link; the reflect queue names it as missing.",
        ),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: it reads \
                           the Reflection and never changes it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below curate access, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such file, or no such domain for this caller (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The file is one only the previous host of a host move \
                           has, whose row a delete here would drop; or the \
                           Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A path rule refused the path.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn asset_remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, rel)): ApiPath<(String, String)>,
) -> Result<StatusCode, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    state
        .engine
        .reflection_asset_delete(&domain, &asset_path(&rel), &caller(&identity))
        .await
        .map_err(reflection_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /domains/{domain}/reflection/move-asset`
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/reflection/move-asset",
    tag = "reflection",
    operation_id = "move_reflection_asset",
    summary = "Move or rename a file of the Reflection.",
    description = "Needs `curate`. Every reflection that links the file \
                   (PACT.md included) is rewritten to the new path; the \
                   rewrite is not a person's change and owes no feedback. A \
                   move never overwrites a file, and waits while a page it \
                   would rewrite is open in a co-editing room.",
    params(("domain" = String, Path, description = "The domain.")),
    request_body = MoveReflectionAssetBody,
    responses(
        (
            status = 200,
            description = "Moved: `{ domain, from, to, links_rewritten }`. \
                           Two keys appear only when needed: `links_failed`, \
                           `[{ domain, path, reason }]` for each page whose link \
                           could not be rewritten after the file moved, and \
                           `note`, a sentence saying the file moved and its \
                           index row did not follow (the next sync brings it \
                           up to date).",
            body = Object,
            example = json!({ "domain": "eng", "from": "assets/diagrams/flow.png",
                "to": "assets/flow.png", "links_rewritten": 2 }),
        ),
        (
            status = 401,
            description = "The anonymous viewer of a domain it reads: it reads \
                           the Reflection and never changes it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Below curate access, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such file, or no such domain for this caller (the \
                           anonymous viewer included).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The new path already holds a file or is a file only \
                           the previous host of a host move has, a page that \
                           links it is open in a co-editing room, or the \
                           Reflection is not here (then possibly `home_url`).",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "A path rule refused a path, or both name the same file.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn asset_move(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<MoveReflectionAssetBody>,
) -> Result<Json<Value>, ApiError> {
    refuse_anonymous(&state, &identity, &domain, ANONYMOUS_EDIT).await?;
    refuse_read_only(&state)?;
    state
        .engine
        .move_reflection_asset(&domain, &body.from, &body.to, &caller(&identity))
        .await
        .map(Json)
        .map_err(reflection_error)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;

    /// A write onto a page or file only the previous host has is 409, on
    /// this surface, on the engram files routes and through the shared
    /// mapping every engram route uses.
    #[test]
    fn a_write_onto_a_path_only_the_previous_host_has_is_a_conflict() {
        use crate::engine::ONLY_ELSEWHERE;
        let text =
            format!("'guides/kept.md' exists only on alpha{ONLY_ELSEWHERE}eng --accept-disk");
        assert_eq!(
            reflection_error(EngineError::Conflict(text.clone())).status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            crate::files::malformed_path_is_a_bad_request(EngineError::Conflict(text.clone()))
                .status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            ApiError::from(EngineError::Conflict(text)).status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            ApiError::from(EngineError::Conflict("another conflict".into())).status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "every other conflict keeps its 422"
        );
    }

    /// The "not here" states are 409, and only the elsewhere one names
    /// the home.
    #[test]
    fn the_reflection_states_are_conflicts_and_elsewhere_names_its_home() {
        let elsewhere = reflection_error(EngineError::Refused(format!(
            "{REFUSED_ELSEWHERE_START}https://team.example.com"
        )));
        assert_eq!(elsewhere.status, StatusCode::CONFLICT);
        assert_eq!(
            elsewhere.home_url.as_deref(),
            Some("https://team.example.com")
        );
        for text in [
            format!("{REFUSED_OFF_START}'eng'; its owner switches it on in the domain policies"),
            format!("Reflection for 'eng' {REFUSED_PENDING_NAMED}: a"),
            format!("Reflection for 'eng' {REFUSED_PENDING}"),
            format!("Reflection for 'eng' {REFUSED_PENDING_UNREADABLE}: reflections/x.md"),
            format!("Reflection for 'eng' {REFUSED_PENDING_UNOPENED}: reflections/y.md"),
            format!("Reflection for 'eng' {REFUSED_PENDING_SPELLINGS}: Reflections/, reflections/"),
            REFUSED_HOMELESS.to_string(),
            format!(
                "'eng' {REFUSED_UNCONNECTED_TEAM}. A team domain's Reflection lives only at its home"
            ),
            format!("{REFUSED_HOSTED_START}alpha; move it with crystalline serve --take-over"),
        ] {
            let e = reflection_error(EngineError::Refused(text.clone()));
            assert_eq!(e.status, StatusCode::CONFLICT, "{text}");
            assert!(e.home_url.is_none(), "{text}");
        }
        // Any other refusal keeps the shared mapping.
        let other = reflection_error(EngineError::Refused("PACT.md is the domain's".into()));
        assert_eq!(other.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// The anonymous comment refusal is a 401: signing in fixes it. Every
    /// other `Forbidden` stays 403.
    #[test]
    fn an_anonymous_comment_is_told_to_sign_in() {
        let e = reflection_error(EngineError::Forbidden(ANONYMOUS_COMMENT.into()));
        assert_eq!(e.status, StatusCode::UNAUTHORIZED);
        let e = reflection_error(EngineError::Forbidden(
            "your access to the Reflection of 'eng' is read, and edit is needed".into(),
        ));
        assert_eq!(e.status, StatusCode::FORBIDDEN);
    }

    /// A stale edit, a create over a page, a busy page and a file move onto
    /// a file are 409; any other conflict keeps the shared 422.
    #[test]
    fn the_reflection_conflicts_are_409() {
        for text in [
            "stale edit: the reflection changed since it was read",
            "a reflection already lives at 'guides/deploying.md'",
            "somebody is editing 'guides/deploying.md'",
            "'assets/flow.png' already holds a file; remove it first or pick another name",
        ] {
            let e = reflection_error(EngineError::Conflict(text.into()));
            assert_eq!(e.status, StatusCode::CONFLICT, "{text}");
        }
        let other = reflection_error(EngineError::Conflict("something else".into()));
        assert_eq!(other.status, StatusCode::UNPROCESSABLE_ENTITY);
        // Only the file move's own sentence: another conflict that happens
        // to say a path holds a file keeps the shared mapping.
        let engram = reflection_error(EngineError::Conflict(
            "'a.md' already holds a file that is not a readable engram".into(),
        ));
        assert_eq!(engram.status, StatusCode::UNPROCESSABLE_ENTITY);
        // A contributor's replace carries the same words and is no conflict:
        // the right is too low, which is 403.
        let replace = reflection_error(EngineError::Forbidden(
            "'assets/flow.png' already holds a file, and replacing it removes that file".into(),
        ));
        assert_eq!(replace.status, StatusCode::FORBIDDEN);
    }
}
