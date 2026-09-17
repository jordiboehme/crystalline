//! One account's own MCP tokens: the credentials its agents authenticate with
//! when `auth.mcp` makes every HTTP MCP connection identify itself.
//!
//! Self-service, like [`super::github_identity`] and unlike
//! [`super::users_api`]: no account name rides the path, because the session
//! already names the account, so a caller can only ever see and revoke its
//! own. Two rules are this surface's own rather than the guard's:
//!
//! - **Every account may issue, viewers included.** An agent acts as the user
//!   that issued its token (Task 5), so a viewer's agent is read-only by
//!   construction and needs no separate gate. Refusing a viewer here would
//!   only mean an instance with `auth.mcp` on had readers who could not
//!   connect an agent at all.
//! - **A read-only instance serves all four**, which is the one place on this
//!   API where an unsafe method is not refused there. `service.read_only`
//!   protects the knowledge, and a token is not knowledge: it is account
//!   state, in the accounts database, beside the password that logs the same
//!   person in. A read-only team server is exactly where agents need tokens,
//!   because with `auth.mcp` on a reading agent cannot connect at all without
//!   one - refusing here would make such an instance impossible to onboard an
//!   agent onto over HTTP.
//! - **The anonymous viewer is refused, 401.** It passes
//!   [`Identity::require_viewer`] where `auth.anonymous` is on, but it has no
//!   account, and a token is issued to an account. Logging in is what changes
//!   that, so it is told to log in rather than that it is forbidden.
//!
//! The token itself exists in the clear exactly once, in the reply to the
//! issuing (or rotating) request: the store keeps only its sha256. Nothing
//! here logs one, [`IssuedTokenResponse`]'s `Debug` redacts it, and the
//! listing carries the label and the timestamps and no token material at all,
//! because there is none left to carry.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::auth::{Identity, NoStore, no_store};
use super::auth_store::{McpTokenInfo, RefusalKind, StoreRefusal};
use super::{ApiError, ApiJson, ApiPath, ProblemDetail, RestState};

/// What `POST /me/mcp-tokens` takes: what the token is for, so a row in the
/// listing is recognizable months later when it comes time to revoke one.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "A new MCP token. The label is what the listing shows: \
                        name the machine or the agent it is for, since the \
                        token itself is never shown again.")]
pub struct IssueBody {
    /// What this token is for. Required and non-empty: an unlabeled row is
    /// one nobody dares revoke.
    #[schema(example = "laptop")]
    pub label: String,
}

/// What issuing and rotating answer with: the one look anybody ever gets at
/// the token.
///
/// `Debug` is written by hand rather than derived, exactly as
/// [`super::auth_store::IssuedMcpToken`]'s is and for the same reason: this
/// type is one `tracing::debug!` or one failed assertion away from putting a
/// live credential in a log file, while `id` and `label` are worth printing.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[schema(description = "A freshly issued MCP token. `token` is shown here and \
                        nowhere else, ever: only its hash is stored. Send it \
                        as `Authorization: Bearer <token>` from the agent's \
                        MCP registration.")]
pub struct IssuedTokenResponse {
    /// The row id, used to rotate or revoke this token later.
    #[schema(example = 3)]
    pub id: i64,
    /// The token itself, this once.
    #[schema(example = "cmt_1f3c...")]
    pub token: String,
    /// The label it was issued under, echoed so the reply is self-describing.
    #[schema(example = "laptop")]
    pub label: String,
}

/// The two responses that carry one are marked `Cache-Control: no-store`, the
/// convention [`super::auth`] applies to every response carrying a CSRF token
/// or a `Set-Cookie`. A 200 to a POST is not heuristically cacheable, so this
/// is defence in depth rather than a hole being closed - but what these carry
/// is a live bearer credential, which is the strongest case on this API for
/// saying so out loud rather than relying on a caching rule holding.
impl std::fmt::Debug for IssuedTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedTokenResponse")
            .field("id", &self.id)
            .field("token", &"cmt_[redacted]")
            .field("label", &self.label)
            .finish()
    }
}

impl From<super::auth_store::IssuedMcpToken> for IssuedTokenResponse {
    fn from(issued: super::auth_store::IssuedMcpToken) -> IssuedTokenResponse {
        IssuedTokenResponse {
            id: issued.id,
            token: issued.token,
            label: issued.label,
        }
    }
}

/// Turn a store failure into a status. Two of them are worth naming, and they
/// are deliberately kept apart:
///
/// - a token id that is not one of this account's is a plain 404 - the row is
///   either gone or was never the caller's, and the two are indistinguishable
///   so an id cannot be probed for.
/// - `no such user` is the caller's OWN account having been removed between
///   the session resolving and this statement. That is a 401, not the 404
///   above: telling somebody their token was revoked when what actually
///   happened is that their account is gone would send them to fix the wrong
///   thing, the same reasoning `mcp_gate`'s `MCP_AUTH_UNAVAILABLE` rests on.
///
/// Deliberately not [`super::users_api`]'s `store_error`: that one classifies
/// the phrases account editing produces and documents a branch order that
/// exists for a collision between two of them. Nothing is gained by teaching
/// it a second vocabulary.
///
/// Classified by [`RefusalKind`] rather than by substring, so a reworded
/// message cannot turn either of these into a 500.
fn store_error(e: anyhow::Error) -> ApiError {
    match StoreRefusal::kind_of(&e) {
        Some(RefusalKind::NoSuchToken) => ApiError::not_found(TOKEN_NOT_FOUND),
        Some(RefusalKind::NoSuchAccount) => ApiError::unauthorized(
            "the account this request was made as no longer exists: log in again",
        ),
        _ => ApiError::internal(format!("{e:#}")),
    }
}

/// What naming an id that is not one of the caller's tokens is told, in one
/// place because the revoke path produces it without the store failing at all.
const TOKEN_NOT_FOUND: &str = "no such MCP token: it may already have been revoked or rotated";

/// `GET /me/mcp-tokens` - the caller's own MCP tokens, newest first.
///
/// A pure read, so a read-only instance serves it. Carries no token material:
/// the store keeps only hashes, so there is nothing to show after issuance.
#[utoipa::path(
    get,
    path = "/api/v1/me/mcp-tokens",
    tag = "settings",
    operation_id = "list_my_mcp_tokens",
    summary = "The caller's own MCP tokens, newest first.",
    description = "Every signed-in account has this, viewers included: an \
                   agent acts as the account that issued its token, so a \
                   viewer's agent is read-only by construction. The rows \
                   carry the label, when the token was issued and when it was \
                   last presented - never the token, which exists in the clear \
                   only in the reply that issued it. Served on a read-only \
                   instance like the rest of this surface: a token is account \
                   state rather than knowledge.",
    responses(
        (status = 200, description = "This account's tokens.", body = Vec<McpTokenInfo>),
        (
            status = 401,
            description = "No identity, or an anonymous one: the anonymous \
                           viewer has no account and so holds no tokens.",
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
) -> Result<Json<Vec<McpTokenInfo>>, ApiError> {
    let user = identity.require_account()?;
    Ok(Json(
        state
            .auth
            .list_mcp_tokens(&user.name)
            .await
            .map_err(store_error)?,
    ))
}

/// `POST /me/mcp-tokens` - issue one, answering with the token itself.
///
/// The one moment the token exists outside the database in readable form. A
/// caller that loses it revokes the row and issues another; there is no way
/// to read it back, by design.
#[utoipa::path(
    post,
    path = "/api/v1/me/mcp-tokens",
    tag = "settings",
    operation_id = "issue_my_mcp_token",
    summary = "Issue an MCP token for the caller's own account.",
    description = "Every signed-in account may issue one, viewers included. \
                   The reply is the only place the token is ever readable: \
                   only its hash is stored, so a lost token is revoked and \
                   replaced rather than looked up. Send it from the agent's \
                   MCP registration as `Authorization: Bearer <token>`. \
                   Served on a read-only instance too: that setting protects \
                   the knowledge, and a token is account state rather than \
                   knowledge - a read-only server is where an agent most \
                   needs one.",
    request_body = IssueBody,
    responses(
        (status = 200, description = "The token, this once.", body = IssuedTokenResponse),
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
            description = "A cookie session did not echo its CSRF token, or \
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
            description = "The label is empty.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn issue(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<IssueBody>,
) -> Result<(NoStore, Json<IssuedTokenResponse>), ApiError> {
    let user = identity.require_account()?;
    let label = check_label(&body.label)?;
    let issued = state
        .auth
        .issue_mcp_token(&user.name, &label)
        .await
        .map_err(store_error)?;
    Ok((no_store(), Json(issued.into())))
}

/// `POST /me/mcp-tokens/{id}/rotate` - replace one of the caller's tokens with
/// a fresh one carrying the same label, answering with the new token.
///
/// One step rather than revoke-then-issue: the old secret stops working and
/// the new one exists together, so a token that may have leaked is replaced
/// without a window in which the account holds none. The reply carries a NEW
/// `id`; the old one is gone.
#[utoipa::path(
    post,
    path = "/api/v1/me/mcp-tokens/{id}/rotate",
    tag = "settings",
    operation_id = "rotate_my_mcp_token",
    summary = "Replace one of the caller's MCP tokens with a fresh one.",
    description = "The old secret stops working and the new one exists in the \
                   same step, so a token that may have leaked is replaced \
                   without a window in which the agent holds none. The label \
                   rides along and the reply carries a new `id`. Only the \
                   caller's own tokens can be named: any other id is 404, so \
                   another account's cannot be probed for.",
    params(("id" = i64, Path, description = "One of the caller's own token ids.")),
    responses(
        (status = 200, description = "The new token, this once.", body = IssuedTokenResponse),
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
            description = "No token of the caller's carries that id.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn rotate(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(id): ApiPath<i64>,
) -> Result<(NoStore, Json<IssuedTokenResponse>), ApiError> {
    let user = identity.require_account()?;
    let issued = state
        .auth
        .rotate_mcp_token(&user.name, id)
        .await
        .map_err(store_error)?;
    Ok((no_store(), Json(issued.into())))
}

/// `DELETE /me/mcp-tokens/{id}` - revoke one of the caller's tokens, 204.
///
/// 404 rather than 204 for an id that names nothing of the caller's, which is
/// the one place this surface is deliberately not idempotent: revoking is the
/// answer to a token that got out, and a caller told "done" about a row it did
/// not actually reach would stop looking.
#[utoipa::path(
    delete,
    path = "/api/v1/me/mcp-tokens/{id}",
    tag = "settings",
    operation_id = "revoke_my_mcp_token",
    summary = "Revoke one of the caller's own MCP tokens.",
    description = "The token stops resolving at once: the next request \
                   carrying it is refused at the door. Only the caller's own \
                   tokens can be named - any other id is 404, the same answer \
                   an unknown one gets, so another account's tokens cannot be \
                   probed for.",
    params(("id" = i64, Path, description = "One of the caller's own token ids.")),
    responses(
        (status = 204, description = "The token is gone."),
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
            description = "No token of the caller's carries that id.",
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
        .revoke_mcp_token(&user.name, id)
        .await
        .map_err(store_error)?;
    if !revoked {
        return Err(ApiError::not_found(TOKEN_NOT_FOUND));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The label as it will be stored: trimmed, and never empty. A row nobody can
/// tell apart from its neighbors is a row nobody dares revoke, which is the
/// whole reason the listing exists.
fn check_label(label: &str) -> Result<String, ApiError> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err(ApiError::unprocessable(
            "the label is empty: name the machine or the agent this token is \
             for, since the token itself is never shown again",
        ));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live credential must never be one `tracing::debug!` away.
    #[test]
    fn a_debugged_issued_token_redacts_the_secret() {
        let issued = IssuedTokenResponse {
            id: 7,
            token: "cmt_c0ffee".repeat(8),
            label: "laptop".to_string(),
        };
        let text = format!("{issued:?}");
        assert!(!text.contains("c0ffee"), "{text}");
        assert!(text.contains("redacted"), "{text}");
        assert!(text.contains('7') && text.contains("laptop"), "{text}");
    }

    #[test]
    fn a_label_is_trimmed_and_never_empty() {
        assert_eq!(check_label("  laptop ").unwrap(), "laptop");
        assert_eq!(
            check_label(" \t ").unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
