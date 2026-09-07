//! One account's own single sign-on identities: which providers it can arrive
//! from, and the one way to give one of them up.
//!
//! Self-service, like [`super::mcp_tokens`] and unlike [`super::users_api`]:
//! no account name rides the path, because the session already names the
//! account, so a caller can only ever see and remove its own links. Making a
//! link is not here at all - it is the sign-on itself, started from
//! `GET /auth/oidc/login?link=true` and finished at the callback, because an
//! identity is only linkable once the provider has proved the person holds it.
//! What this surface adds is the other two halves: reading back what is
//! linked, and unlinking.
//!
//! Two rules are this surface's own rather than the guard's:
//!
//! - **Every account may read and unlink its own, viewers included.** How
//!   somebody signs in is not a privilege, and an account that could not undo
//!   its own link would have to ask an administrator to undo something it did
//!   itself.
//! - **A read-only instance serves both.** `service.read_only` protects the
//!   knowledge, and an identity link is account state, in the accounts
//!   database, beside the password that logs the same person in. Exactly the
//!   settlement [`super::mcp_tokens`] documents.
//!
//! The one refusal worth stating here is the store's, forwarded verbatim: an
//! account with no password whose last identity this is cannot unlink it,
//! because it would leave an account nobody can reach. That is one rule, in
//! `AuthStore::unlink_identity`, and this route, the profile card and
//! `crystalline users unlink` are three callers of it rather than three
//! spellings of it.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::auth::{Caller, Identity};
use super::auth_store::{IdentityLink, RefusalKind, StoreRefusal, User};
use super::{ApiError, ApiPath, ProblemDetail, RestState};

/// What `GET /me/identity-links` answers with.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "The single sign-on identities this account holds, and \
                        whether it has a password to fall back on. The two \
                        together are what the profile card needs: an account \
                        with no password and one identity cannot unlink it, \
                        because that link is its only way in.")]
pub struct IdentityLinksResponse {
    /// Every identity this account holds, by issuer.
    pub links: Vec<IdentityLink>,
    /// Whether this account can also sign in with a password. False for an
    /// account a first sign-on provisioned, which has none until
    /// `crystalline users passwd` gives it one.
    #[schema(example = true)]
    pub has_password: bool,
}

/// The account behind the request, which is the only one this surface acts
/// for.
///
/// [`Identity::require_viewer`] is the whole role check - every account may
/// hold identities - but it hands back a [`Caller`], and the anonymous variant
/// of that is not an account: there is nobody whose links could be listed. That
/// case is 401 rather than 403, because logging in is what fixes it.
fn require_own_account(identity: &Identity) -> Result<User, ApiError> {
    match identity.require_viewer()? {
        Caller::Account(user) => Ok(user),
        Caller::Anonymous => Err(ApiError::unauthorized(
            "this request is served as the anonymous viewer, which has no \
             account and so holds no single sign-on identities: log in first",
        )),
    }
}

/// Turn a store failure into a status.
///
/// The one refusal that is the caller's doing is the last-way-in guard, and it
/// is a conflict with the state of the account rather than a bad request: the
/// same call succeeds once the account has a password. Its words are the
/// store's own and are forwarded verbatim, because they name the command that
/// resolves it and they name only the caller's own account.
fn store_error(e: anyhow::Error) -> ApiError {
    match StoreRefusal::kind_of(&e) {
        Some(RefusalKind::LastCredential) => ApiError::conflict(format!("{e:#}")),
        _ => ApiError::internal(format!("{e:#}")),
    }
}

/// `GET /me/identity-links` - the identities the caller's account holds.
#[utoipa::path(
    get,
    path = "/api/v1/me/identity-links",
    tag = "settings",
    operation_id = "list_my_identity_links",
    summary = "The single sign-on identities the caller's account holds.",
    description = "Every signed-in account has this surface, viewers \
                   included. A row carries the issuer, the provider's stable \
                   subject, when the link was made and who made it - `jit` \
                   for a link a first sign-on created along with its account, \
                   `cli` for one an administrator made, otherwise the account \
                   that linked it to itself. `has_password` says whether the \
                   account has a second way in, which is what decides whether \
                   the last link may be given up. Served on a read-only \
                   instance: an identity link is account state rather than \
                   knowledge.",
    responses(
        (status = 200, description = "This account's links.", body = IdentityLinksResponse),
        (
            status = 401,
            description = "No identity, or an anonymous one: the anonymous \
                           viewer has no account.",
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
) -> Result<Json<IdentityLinksResponse>, ApiError> {
    let user = require_own_account(&identity)?;
    let links = state
        .auth
        .identity_links(&user.name)
        .await
        .map_err(store_error)?;
    let has_password = state
        .auth
        .has_password(&user.name)
        .await
        .map_err(store_error)?;
    Ok(Json(IdentityLinksResponse {
        links,
        has_password,
    }))
}

/// `DELETE /me/identity-links/{issuer}` - give up one of them.
///
/// Keyed on the issuer rather than on the subject, because an account holds at
/// most one identity per provider and the person unlinking knows which
/// provider they mean rather than which opaque string it calls them.
#[utoipa::path(
    delete,
    path = "/api/v1/me/identity-links/{issuer}",
    tag = "settings",
    operation_id = "unlink_my_identity",
    params(
        ("issuer" = String, Path, description = "The provider's issuer url, \
         percent-encoded as one path segment."),
    ),
    summary = "Unlink one single sign-on identity from the caller's account.",
    description = "Removes the identity this account holds at that issuer. A \
                   later sign-on from it then provisions a new account rather \
                   than reaching this one. Refused with 409 when it is this \
                   account's last way in - no password and no other identity - \
                   because unlinking would leave an account nobody can sign \
                   in to; the refusal names `crystalline users passwd`, which \
                   is what gives the account a password first. An \
                   administrator can force it from the command line, for the \
                   repair where a provider re-issued its subjects.",
    responses(
        (status = 204, description = "The link is gone."),
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
            description = "This account holds no identity at that issuer.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "That link is the account's last way in.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn unlink(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(issuer): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let user = require_own_account(&identity)?;
    let removed = state
        .auth
        .unlink_identity(&issuer, &user.name)
        .await
        .map_err(store_error)?;
    if !removed {
        return Err(ApiError::not_found(
            "this account holds no single sign-on identity at that issuer",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}
