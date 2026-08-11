//! Domain lifecycle (admin): create in any of the three modes and unregister.
//! REST is the untrusted surface of the engine verbs MCP and the CLI already
//! use: names are validated here (an operator typing at the CLI is trusted; a
//! browser is not), and a local domain is always created under the configured
//! domains root - no folder parameter exists on this surface, by design.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use super::auth::Identity;
use super::{ApiError, ApiJson, ApiPath, ProblemDetail, RestState, refuse_read_only};
use crate::engine::EngineError;

/// What `POST /domains` takes: a mode and whatever that mode needs.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "A domain to register. `mode` picks which of the three \
                        kinds is meant, and every other field belongs to one \
                        of them: a field that does not apply to the mode \
                        asked for is refused rather than ignored.")]
pub struct CreateDomainBody {
    /// local | virtual | github
    #[schema(example = "local")]
    pub mode: String,
    /// The domain name. Required for local and virtual; optional for github
    /// (defaults to the repository name). A local domain always lands at
    /// <domains_root>/<name>; there is no folder parameter on this surface.
    #[serde(default)]
    #[schema(example = "notes")]
    pub name: Option<String>,
    /// owner/name; github mode only.
    #[serde(default)]
    #[schema(example = "acme/knowledge")]
    pub repo: Option<String>,
    /// Branch to track; github mode only, defaults to the repo default.
    #[serde(default)]
    #[schema(example = "main")]
    pub branch: Option<String>,
    /// Subfolder within the repository; github mode only.
    #[serde(default)]
    #[schema(example = "domains/eng")]
    pub path: Option<String>,
}

/// A domain name that is safe as a path segment under the domains root and
/// in crystalline:// addresses: no separators, no traversal, no drive
/// colon, not hidden, no whitespace.
fn check_domain_name(name: &str) -> Result<&str, ApiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::unprocessable("the domain name is empty"));
    }
    if trimmed.chars().any(char::is_whitespace)
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.starts_with('.')
    {
        return Err(ApiError::unprocessable(format!(
            "'{trimmed}' cannot name a domain: use letters, digits, hyphens \
             and underscores, with no separators, colons or leading dots"
        )));
    }
    Ok(trimmed)
}

/// `POST /domains` - register a domain: a local folder under the server's
/// domains root, a database-backed virtual one, or a GitHub team domain.
///
/// The local mode is deliberately NAME-ONLY. A browser client cannot name a
/// folder on the server, so the only path this surface can produce is
/// `<domains_root>/<name>`, and traversal is closed by construction rather
/// than by sanitizing a path: the name is checked to be one plain segment
/// ([`check_domain_name`]) and the engine joins it under the configured root.
/// An operator who does need a specific folder has the `crystalline` CLI,
/// which is trusted in a way an HTTP body is not.
///
/// The github mode pre-checks readiness and refuses with a 409 pointing at
/// the settings screen when no credential is on file, rather than letting the
/// engine fail deep inside a download. It is a SYNCHRONOUS registration: the
/// repository is fetched and indexed inside this request, so a large team
/// repository holds the response open. That is accepted for this slice; a
/// progress surface over `origin_add_with_progress` is a follow-up.
#[utoipa::path(
    post,
    path = "/api/v1/domains",
    tag = "domains",
    operation_id = "create_domain",
    summary = "Register a domain: local, virtual or a GitHub team domain.",
    description = "Admin only. A local domain is name-only and always lands \
                   at `<domains_root>/<name>` on the server, so no request \
                   can place a folder anywhere else; a virtual domain lives \
                   in the database; a team domain needs a GitHub connection \
                   and downloads the repository inside this request, which \
                   can take a while for a large one.",
    request_body = CreateDomainBody,
    responses(
        (
            status = 201,
            description = "The engine's own registration report, unchanged. \
                           Its shape follows the mode: a local domain reports \
                           where it landed, a virtual one reports that it was \
                           registered, a team domain reports what was fetched.",
            body = Object,
            example = json!({
                "domain": "notes",
                "root": "/Users/ada/Documents/Crystalline/notes",
                "kind": "file",
                "manifest_created": true,
                "adopted": false
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The name is taken by another domain, or mode \
                           `github` was asked for on an instance with no \
                           GitHub connection - the detail says where to make \
                           one.",
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
            description = "An unknown mode, a name that could escape the \
                           domains root, or a field that does not belong to \
                           the mode asked for.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn create(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<CreateDomainBody>,
) -> Result<Response, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    // Serialized against a concurrent unregister of the same name: see
    // [`RestState::domain_admin`] for the engine-level race this closes.
    let _admin = state.domain_admin().await;
    let report = match body.mode.as_str() {
        "local" => {
            let name = check_domain_name(body.name.as_deref().unwrap_or_default())?;
            require_absent(&body.repo, "repo", "local")?;
            require_absent(&body.branch, "branch", "local")?;
            require_absent(&body.path, "path", "local")?;
            state.engine.domain_add_local(Some(name), None).await
        }
        "virtual" => {
            let name = check_domain_name(body.name.as_deref().unwrap_or_default())?;
            require_absent(&body.repo, "repo", "virtual")?;
            require_absent(&body.branch, "branch", "virtual")?;
            require_absent(&body.path, "path", "virtual")?;
            state.engine.domain_add_virtual(name).await
        }
        "github" => {
            let repo = body
                .repo
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .ok_or_else(|| {
                    ApiError::unprocessable("a team domain requires repo as owner/name")
                })?;
            // The TRIMMED name, and it has to be the one that travels on:
            // `origin_add` uses what it is handed verbatim as the config key
            // and as the folder segment under the domains root, so passing
            // the raw body field would register a domain named "\tnotes "
            // and create a folder to match - exactly what this validator
            // exists to prevent, and what the other two modes avoid by
            // passing on what `check_domain_name` gave back.
            let name = body.name.as_deref().map(check_domain_name).transpose()?;
            // Checked here rather than left to the engine: without a
            // connection the registration cannot work at all, and this way
            // the refusal names the screen that fixes it instead of arriving
            // as a generic remote failure.
            if !state.engine.github_ready().await {
                return Err(ApiError::conflict(
                    "GitHub is not connected on this instance: connect it under \
                     Settings > GitHub, then register the team domain",
                ));
            }
            state
                .engine
                .origin_add(
                    repo,
                    name,
                    body.path.as_deref(),
                    body.branch.as_deref(),
                    // Never from this surface: a team domain lands under the
                    // domains root like a local one, for the same reason.
                    None,
                )
                .await
        }
        other => {
            return Err(ApiError::unprocessable(format!(
                "mode must be local, virtual or github, got '{other}'"
            )));
        }
    };
    match report {
        Ok(report) => Ok((StatusCode::CREATED, Json(report)).into_response()),
        // A taken name (or an already-registered folder) is a conflict on
        // this surface, as on engram create; the generic From keeps 422 for
        // MCP's classification.
        Err(EngineError::Conflict(detail)) => Err(ApiError::conflict(detail)),
        Err(e) => Err(e.into()),
    }
}

/// A mode-mismatched field is a 422 up front, not silently ignored.
fn require_absent(field: &Option<String>, field_name: &str, mode: &str) -> Result<(), ApiError> {
    match field.as_deref().map(str::trim) {
        Some(v) if !v.is_empty() => Err(ApiError::unprocessable(format!(
            "{field_name} does not apply to a {mode} domain"
        ))),
        _ => Ok(()),
    }
}

/// `DELETE /domains/{domain}` - unregister a domain: the registration and the
/// index rows go, the files do not.
///
/// The order of the three steps below is the whole content of this handler,
/// and it is not free to rearrange (see [`crate::collab::session::CollabSessions::dispose_domain`],
/// which records the argument in full):
///
/// 1. The join fence goes up first, so no socket can open a room in this
///    domain from here on. Without it the sweep would close what is open and
///    a join arriving one instant later would open a fresh room over a domain
///    that is about to vanish.
/// 2. The rooms are swept while the domain is STILL registered, so each
///    room's final save lands in the file that stays on disk.
/// 3. Only then is the domain unregistered. Inverted, those final saves would
///    be refused outright or - inside the window between the config write and
///    the index clear - resolve as virtual and land in the DATABASE rather
///    than in the file `files_kept` promises was left alone.
///
/// The response is the engine's report plus `rooms_closed`, so a client can
/// say how many co-editing sessions it just ended.
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}",
    tag = "domains",
    operation_id = "unregister_domain",
    summary = "Unregister a domain. Files on disk are never touched.",
    description = "Admin only. The registration and the domain's index rows \
                   go; a file domain's files stay exactly where they are \
                   (re-adding the folder adopts them again), which is what \
                   `files_kept` reports. A virtual domain has no files, so \
                   `files_kept` is false and its knowledge is gone - a client \
                   must confirm that difference in words. Any open \
                   co-editing rooms in the domain are saved and closed first; \
                   `rooms_closed` counts them.",
    params(("domain" = String, Path, description = "The registered domain.")),
    responses(
        (
            status = 200,
            description = "The engine's own unregistration report, plus the \
                           number of co-editing rooms this call closed.",
            body = Object,
            example = json!({
                "domain": "eng",
                "unregistered": true,
                "files_kept": true,
                "index_cleared": true,
                "rooms_closed": 0
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
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
            description = "The domain is defined by an environment variable, \
                           which owns it: unset the variable instead.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    // Step 1 and step 2 of this handler's ordering; see the doc comment.
    let _admin = state.domain_admin().await;
    let _fence = state.fence_joins().await;
    let rooms_closed = state.collab.dispose_domain(&domain).await;
    // Step 3, still behind both guards.
    let mut report = state.engine.domain_remove(&domain).await.map_err(|e| {
        match e {
            // An env-defined domain cannot be unregistered by anyone but the
            // environment: a conflict on this surface rather than the generic
            // 422, since no version of this request would succeed.
            EngineError::Conflict(detail) => ApiError::conflict(detail),
            other => other.into(),
        }
    })?;
    if let Value::Object(map) = &mut report {
        map.insert("rooms_closed".to_string(), Value::from(rooms_closed));
    }
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_name_is_one_plain_segment() {
        assert_eq!(check_domain_name(" notes ").unwrap(), "notes");
        assert_eq!(
            check_domain_name("brand-knowledge_2").unwrap(),
            "brand-knowledge_2"
        );
        for bad in [
            "", "   ", "../up", "a/b", "a\\b", "a:b", ".hidden", "a b", "a\tb",
        ] {
            assert_eq!(
                check_domain_name(bad).unwrap_err().status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{bad:?} names no domain"
            );
        }
    }

    #[test]
    fn a_field_from_another_mode_is_refused_but_an_empty_one_is_not() {
        assert!(require_absent(&None, "repo", "local").is_ok());
        assert!(require_absent(&Some("  ".to_string()), "repo", "local").is_ok());
        let err = require_absent(&Some("acme/kb".to_string()), "repo", "local").unwrap_err();
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(err.detail.contains("repo"), "{}", err.detail);
    }
}
