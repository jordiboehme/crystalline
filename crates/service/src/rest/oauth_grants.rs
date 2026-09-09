//! One account's own OAuth grants: the clients it has connected through the
//! authorization code flow (`GET /oauth/authorize` through
//! `POST /oauth/authorizations/{id}`), and the one way to disconnect one.
//!
//! Modelled on [`super::mcp_tokens`], which this surface sits directly beside
//! on the profile card ("Connected clients" under Agent access): no account
//! name rides the path, because the session already names the account, so a
//! caller can only ever see and revoke its own. The same two settlements that
//! surface documents carry over verbatim, because a grant is exactly the same
//! kind of thing a personal MCP token is - a credential an agent presents at
//! the MCP gate, sitting in the accounts database rather than in a domain:
//!
//! - **Every account may list and revoke, viewers included.** An OAuth grant
//!   authenticates as the account that consented to it (Task 5), so a
//!   viewer's connected client is read-only by construction and needs no
//!   separate gate.
//! - **A read-only instance serves both.** `service.read_only` protects the
//!   knowledge, and a grant is account state, not knowledge - the same
//!   reasoning [`super::mcp_tokens`] documents at length. A read-only team
//!   server with `auth.oauth` on is exactly where a hosted client needs to be
//!   revocable without being able to touch a domain.
//! - **The anonymous viewer is refused, 401.** It has no account, and a grant
//!   is issued to one. Logging in is what changes that.
//!
//! Revoking deletes the row outright rather than marking it - the same
//! `AuthStore::revoke_oauth_grant` a disabled or removed account's sweep
//! already calls - so both of the grant's tokens stop resolving at the MCP
//! gate at once, on the very next request: [`crate::mcp_gate`]'s OAuth lookup
//! is a fresh query every time, there is nothing cached to invalidate.
//!
//! Nothing here ever carries a token: the store keeps only hashes, and
//! [`super::auth_store::OauthGrantInfo`] was never given a field for one.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::auth::Identity;
use super::auth_store::OauthGrantInfo;
use super::{ApiError, ApiPath, ProblemDetail, RestState};

/// What naming an id that is not one of the caller's grants is told, in one
/// place because the revoke path produces it without the store failing at
/// all - exactly [`super::mcp_tokens::TOKEN_NOT_FOUND`]'s shape, one door
/// over.
const GRANT_NOT_FOUND: &str = "no such connected client: it may already have been revoked";

/// Turn a store failure into a status.
///
/// Unlike [`super::mcp_tokens::store_error`], neither
/// `AuthStore::list_oauth_grants` nor `AuthStore::revoke_oauth_grant` can
/// report "no such account" for the caller's own name: both take it straight
/// from the session's own `User` row, which only ever fails to normalize on an
/// empty or whitespace name, and a session cannot resolve to one. What is left
/// to fail is a database error, so this maps everything to 500 rather than
/// inventing a classification with nothing to classify.
fn store_error(e: anyhow::Error) -> ApiError {
    ApiError::internal(format!("{e:#}"))
}

/// `GET /me/oauth-grants` - the caller's own OAuth grants, newest first.
///
/// A pure read, so a read-only instance serves it. Carries no token material:
/// the store keeps only hashes, so there is nothing to show beyond which
/// client is connected, since when, and until when it may keep refreshing.
#[utoipa::path(
    get,
    path = "/api/v1/me/oauth-grants",
    tag = "settings",
    operation_id = "list_my_oauth_grants",
    summary = "The caller's own OAuth grants, newest first.",
    description = "Every signed-in account has this, viewers included: a \
                   hosted client acts as the account that consented to it, so \
                   a viewer's connected client is read-only by construction. \
                   The rows carry the client's name, the host it redirects \
                   back to, when the grant was made, when it was last used \
                   and until when it may keep refreshing - never a token, \
                   which the store keeps only hashed. A grant that can no \
                   longer refresh is left out rather than shown as dead \
                   weight. Served on a read-only instance like the rest of \
                   this surface: a grant is account state rather than \
                   knowledge.",
    responses(
        (status = 200, description = "This account's connected clients.", body = Vec<OauthGrantInfo>),
        (
            status = 401,
            description = "No identity, or an anonymous one: the anonymous \
                           viewer has no account and so holds no grants.",
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
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<Vec<OauthGrantInfo>>, ApiError> {
    let user = identity.require_account()?;
    Ok(Json(
        state
            .auth
            .list_oauth_grants(&user.name)
            .await
            .map_err(store_error)?,
    ))
}

/// `DELETE /me/oauth-grants/{id}` - revoke one of the caller's own grants,
/// 204.
///
/// 404 rather than 204 for an id that names nothing of the caller's, the same
/// departure from idempotence [`super::mcp_tokens::revoke`] makes and for the
/// same reason: revoking answers a client that should no longer be connected,
/// and a caller told "done" about a row it did not actually reach would stop
/// looking. One delete removes both of the grant's tokens at once, so the
/// next request either one presents at the MCP gate is refused immediately -
/// there is no cache between the store and the gate to wait out.
#[utoipa::path(
    delete,
    path = "/api/v1/me/oauth-grants/{id}",
    tag = "settings",
    operation_id = "revoke_my_oauth_grant",
    summary = "Revoke one of the caller's own connected clients.",
    description = "Both of the grant's tokens stop resolving at once: the \
                   next request either presents at the MCP gate is refused at \
                   the door. Only the caller's own grants can be named - any \
                   other id is 404, the same answer an unknown one gets, so \
                   another account's connections cannot be probed for.",
    params(("id" = i64, Path, description = "One of the caller's own grant ids.")),
    responses(
        (status = 204, description = "The grant is gone; both its tokens are dead."),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "A cookie session did not echo its CSRF token, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No grant of the caller's carries that id.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn revoke(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    let user = identity.require_account()?;
    let revoked = state
        .auth
        .revoke_oauth_grant(&user.name, id)
        .await
        .map_err(store_error)?;
    if !revoked {
        return Err(ApiError::not_found(GRANT_NOT_FOUND));
    }
    Ok(StatusCode::NO_CONTENT)
}
