//! One account's own GitHub identity: the self-service half of the settings
//! surface. Where [`super::github_settings`] manages the MACHINE's credential
//! and is admin-only, everything here manages the CALLER's own and is open to
//! whoever can share - the session already names the account, so no path
//! segment does.
//!
//! Token-material-free in both directions, like its instance sibling: the two
//! connect routes take a token and never echo one, and the status carries only
//! whether an identity is on file, the login it authenticated as, since when
//! and where it lives.
//!
//! Two rules differ from the instance surface, both deliberate:
//!
//! - a viewer is refused. Viewers cannot share, so an identity of theirs would
//!   have nothing to do; the refusal says that rather than reciting roles.
//! - connecting does NOT turn `github.enabled` on. Enabling collaboration is an
//!   instance-wide decision an admin makes on the settings screen; an editor
//!   connecting their own identity is not that intent, and this surface is only
//!   reachable on an instance where sharing already exists.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::auth::Identity;
use super::github_settings::{GithubPendingView, TokenBody};
use super::{ApiError, ApiJson, Caller, ProblemDetail, RestState, refuse_read_only};

/// One account's GitHub identity as its profile card renders and polls it.
/// Never carries token material: the engine's own status struct, passed
/// through with the timestamp rendered.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "One account's own GitHub identity: whose it is, \
                        whether a credential is on file, the login it \
                        authenticated as, since when and where it lives. No \
                        token material, ever.")]
pub struct GithubIdentityResponse {
    /// The account this identity belongs to: always the caller's own.
    #[schema(example = "ada")]
    pub account: String,
    /// Whether a personal credential is on file for that account.
    #[schema(example = true)]
    pub connected: bool,
    /// The GitHub login it authenticated as, when connected.
    #[schema(example = "octo")]
    pub login: Option<String>,
    /// When the credential was stored, RFC 3339. The card's "connected since".
    #[schema(example = "2026-08-29T09:12:44Z")]
    pub connected_at: Option<String>,
    /// keyring | file; null when disconnected. Never `environment`: the
    /// environment supplies the machine's credential and never a personal one.
    #[schema(example = "keyring")]
    pub token_store: Option<String>,
    /// This account's device flow waiting for the browser side. Poll this
    /// route until it goes null; the flow either connected or set `error`.
    pub pending: Option<GithubPendingView>,
    /// This account's device flow's failure (expired, denied), reported on
    /// exactly one status read after the flow ends, then cleared.
    pub error: Option<String>,
}

fn view(c: crate::engine::GithubIdentity) -> GithubIdentityResponse {
    GithubIdentityResponse {
        account: c.account,
        connected: c.connected,
        login: c.login,
        connected_at: c.connected_at.map(|at| at.to_rfc3339()),
        token_store: c.token_store,
        pending: c.pending.map(|p| GithubPendingView {
            user_code: p.user_code,
            verification_url: p.verification_url,
            expires_in_secs: p.expires_in_secs,
        }),
        error: c.error,
    }
}

/// The caller, when they may manage a GitHub identity of their own: an editor
/// or an admin.
///
/// Editor rather than admin because this is the caller's OWN credential, not
/// the instance's - the same role that may write knowledge may say who it
/// writes it as. A viewer is refused in this surface's own words: the generic
/// role message would name a permission, where what a viewer needs to hear is
/// that there is nothing here for them because they cannot share at all.
fn require_own_identity(identity: &Identity) -> Result<Caller, ApiError> {
    identity.require_editor().map_err(|e| {
        if e.status == StatusCode::FORBIDDEN {
            return ApiError::forbidden(
                "only editors and admins can share, so a viewer account has no \
                 GitHub identity to connect",
            );
        }
        e
    })
}

/// `GET /me/github-identity` - the caller's own GitHub identity, and their
/// device flow's poll.
///
/// A pure read, so a read-only instance serves it. It doubles as the device
/// flow's poll the same way the instance status does: while a flow of this
/// account's runs, `pending` carries the code; once it ends, `pending` goes
/// null and a failure is reported in `error` on exactly that one read.
#[utoipa::path(
    get,
    path = "/api/v1/me/github-identity",
    tag = "settings",
    operation_id = "get_my_github_identity",
    summary = "The caller's own GitHub identity, and their device flow's poll.",
    description = "Editors and admins only - a viewer cannot share, so there \
                   is no identity of theirs to manage. A pure read, served \
                   even on a read-only instance. Also the device flow's poll: \
                   while one of this account's runs, `pending` carries the \
                   short code and its URL; once it ends, `pending` goes null \
                   and a failed flow's reason is reported in `error` on \
                   exactly one read, then cleared.",
    responses(
        (status = 200, description = "The identity. No token material.", body = GithubIdentityResponse),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is a viewer, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "This account's name cannot address a credential \
                           (account names for sharing use lowercase letters, \
                           digits, dots, hyphens and underscores). The detail \
                           names the fix.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn status(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<GithubIdentityResponse>, ApiError> {
    let caller = require_own_identity(&identity)?;
    // A pure read: served under read_only, like the instance status.
    Ok(Json(view(
        state.engine.github_identity_status(caller.name()).await?,
    )))
}

/// `POST /me/github-identity/connect` - start a device-code sign-in for the
/// caller's own identity, answering 202 with the code to confirm in a browser.
///
/// 202 rather than 200 for the same reason the instance connect answers 202:
/// the connection does not exist yet when this answers. The flow runs in the
/// background and its outcome shows up on [`status`], which the card polls.
///
/// One sign-in at a time across the whole instance. A second call from this
/// same account reports the code already outstanding rather than stranding a
/// new one, so a double click is safe; a call while ANOTHER identity's flow is
/// in flight - another person's, or the machine's - is answered 409, because
/// there is one flow slot and two sign-ins must never complete into each
/// other's credential.
#[utoipa::path(
    post,
    path = "/api/v1/me/github-identity/connect",
    tag = "settings",
    operation_id = "connect_my_github_identity_device",
    summary = "Start a device-code sign-in for the caller's own identity.",
    description = "Editors and admins only, and refused on a read-only \
                   instance. Answers 202 with the short code to confirm in a \
                   browser; the flow runs in the background and its outcome is \
                   read from `GET /me/github-identity`. A second call from the \
                   same account reports that same flow; one made while another \
                   identity's sign-in is in flight is refused 409. Unlike the \
                   instance connect, this does not turn `github.enabled` on: \
                   enabling collaboration is an admin's instance-wide \
                   decision.",
    responses(
        (
            status = 202,
            description = "The flow started (or this account's was already \
                           running): `pending` carries the code to confirm. \
                           Poll `GET /me/github-identity` for the outcome.",
            body = GithubIdentityResponse,
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is a viewer, a cookie session did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "Another identity's device sign-in is in flight. \
                           There is one flow at a time per instance; wait for \
                           it to finish and start again.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "This account's name cannot address a credential, or \
                           GitHub refused to start the flow.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn connect(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Response, ApiError> {
    let caller = require_own_identity(&identity)?;
    refuse_read_only(&state)?;
    let started = state
        .engine
        .start_github_identity_device_flow(caller.name())
        .await?;
    Ok((StatusCode::ACCEPTED, Json(view(started))).into_response())
}

/// `PUT /me/github-identity/token` - connect the caller's own identity with a
/// personal access token, answering with the identity as it now stands.
///
/// The token is validated against GitHub before it is stored, so the login the
/// card shows is the one the token actually belongs to rather than one the
/// caller claimed. Write-only in both directions: the response is the same
/// token-material-free shape every other route here answers with.
///
/// `PUT` rather than the instance surface's `POST`: this replaces the caller's
/// one identity rather than adding to a collection, so re-pasting a token is
/// the same request twice with the same result.
#[utoipa::path(
    put,
    path = "/api/v1/me/github-identity/token",
    tag = "settings",
    operation_id = "connect_my_github_identity_token",
    summary = "Connect the caller's own identity with a personal access token.",
    description = "Editors and admins only, and refused on a read-only \
                   instance. The token is validated against GitHub before it \
                   is stored, and is never echoed: the answer is the same \
                   token-material-free status the GET returns. Replaces \
                   whatever this account had connected before.",
    request_body = TokenBody,
    responses(
        (status = 200, description = "The identity as it now stands.", body = GithubIdentityResponse),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is a viewer, a cookie session did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
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
            description = "The body is JSON but not a token, the token is \
                           empty, GitHub refused it, or this account's name \
                           cannot address a credential.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn token(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<TokenBody>,
) -> Result<Json<GithubIdentityResponse>, ApiError> {
    let caller = require_own_identity(&identity)?;
    refuse_read_only(&state)?;
    let token = body.token.trim();
    if token.is_empty() {
        return Err(ApiError::unprocessable("the token is empty"));
    }
    Ok(Json(view(
        state
            .engine
            .connect_github_identity_token(caller.name(), token)
            .await?,
    )))
}

/// `DELETE /me/github-identity` - forget the caller's own credential,
/// answering with the identity as it now stands.
///
/// Idempotent, like the method it is spelled with: disconnecting an account
/// that holds no credential succeeds and answers `connected: false` rather
/// than 404, so the card can offer the button without first deciding whether
/// pressing it would be legal.
///
/// Only this account's credential is touched. The instance connection, and
/// everybody else's identity, are untouched by construction: each lives in its
/// own store entry.
#[utoipa::path(
    delete,
    path = "/api/v1/me/github-identity",
    tag = "settings",
    operation_id = "disconnect_my_github_identity",
    summary = "Forget the caller's own GitHub credential.",
    description = "Editors and admins only, and refused on a read-only \
                   instance. Idempotent: disconnecting an account that holds \
                   no credential succeeds and answers `connected: false` \
                   rather than 404. The instance connection is untouched.",
    responses(
        (status = 200, description = "The identity as it now stands.", body = GithubIdentityResponse),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is a viewer, a cookie session did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "This account's name cannot address a credential.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn disconnect(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<GithubIdentityResponse>, ApiError> {
    let caller = require_own_identity(&identity)?;
    refuse_read_only(&state)?;
    Ok(Json(view(
        state
            .engine
            .disconnect_github_identity(caller.name())
            .await?,
    )))
}
